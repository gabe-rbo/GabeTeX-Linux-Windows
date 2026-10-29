//! Directory watcher: port of `DirectoryWatcher.swift` on top of the `notify` crate.
//!
//! Semantics kept from the Swift (FSEvents) version:
//!
//! * the whole tree below the root is watched (recursively);
//! * events are **coalesced with a 200 ms latency, `NoDefer` style**: the first event after a quiet period is
//!   delivered immediately, events that follow are batched per 200 ms window until a window stays quiet;
//! * paths with a hidden component below the root (`.git`, `.DS_Store`, editor swap files …) are ignored;
//! * `callback` receives the unique changed paths of one batch (files and directories; created, modified,
//!   removed or renamed — check the file system for the current state), spelled under the root as given to
//!   [`start`]. When the watched directory itself is moved or deleted the callback receives the root;
//! * after [`WatchHandle::stop`] returns no further callback is delivered (one already running may finish).
//!
//! On Linux (inotify has no recursive mode) one non-recursive watch is placed per non-hidden directory, new
//! directories are added as they appear, and hidden trees such as `.git` are never watched. macOS and
//! Windows use the native recursive watchers.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use notify::event::{AccessKind, AccessMode, EventKind, ModifyKind, RenameMode};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

/// FSEvents latency of the Swift watcher.
pub const LATENCY: Duration = Duration::from_millis(200);

enum Msg {
    Event(Event),
    Stop,
}

/// A running watcher. Dropping it stops watching.
pub struct WatchHandle {
    root: String,
    stopped: Arc<AtomicBool>,
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

impl WatchHandle {
    /// The watched root, as given to [`start`].
    pub fn root(&self) -> &str {
        &self.root
    }

    /// True until [`WatchHandle::stop`] (or drop).
    pub fn is_running(&self) -> bool {
        !self.stopped.load(Ordering::SeqCst)
    }

    /// Stops watching and waits for the worker (called from the callback itself it does not wait).
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Msg::Stop);
        if let Some(t) = self.thread.take() {
            if t.thread().id() != thread::current().id() {
                let _ = t.join();
            }
        }
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Starts watching `root`. `callback` runs on a worker thread with the unique changed absolute paths of each
/// coalesced batch.
pub fn start<F>(root: &str, callback: F) -> Result<WatchHandle, String>
where
    F: Fn(Vec<String>) + Send + 'static,
{
    let root_path = PathBuf::from(root);
    if !root_path.is_dir() {
        return Err(format!("“{root}” is not a folder."));
    }
    let (tx, rx) = mpsc::channel::<Msg>();
    let event_tx = tx.clone();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            let _ = event_tx.send(Msg::Event(event));
        }
    })
    .map_err(|e| format!("Could not watch “{root}”: {e}"))?;

    let mapper = PathMapper::new(root);
    if cfg!(target_os = "linux") {
        watcher
            .watch(&root_path, RecursiveMode::NonRecursive)
            .map_err(|e| format!("Could not watch “{root}”: {e}"))?;
        add_subdirectories(&mut watcher, &root_path, &mapper);
    } else {
        watcher
            .watch(&root_path, RecursiveMode::Recursive)
            .map_err(|e| format!("Could not watch “{root}”: {e}"))?;
    }

    let stopped = Arc::new(AtomicBool::new(false));
    let worker_stopped = stopped.clone();
    let thread = thread::Builder::new()
        .name("gabetex-watcher".into())
        .spawn(move || worker(rx, watcher, mapper, worker_stopped, callback))
        .map_err(|e| e.to_string())?;
    Ok(WatchHandle { root: root.to_string(), stopped, tx, thread: Some(thread) })
}

// ---------------------------------------------------------------------------------------------------
// Path filtering (EventSink.handle / relativePath)
// ---------------------------------------------------------------------------------------------------

/// Maps event paths (which may use the resolved spelling of the root, e.g. through a symlink) back to the
/// spelling the caller gave, and drops hidden paths.
#[derive(Debug, Clone)]
pub struct PathMapper {
    root: String,
    resolved: String,
}

fn trim_seps(p: &str) -> &str {
    if p.len() > 1 { p.trim_end_matches(['/', '\\']) } else { p }
}

impl PathMapper {
    pub fn new(root: &str) -> Self {
        let resolved = std::fs::canonicalize(root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| root.to_string());
        PathMapper { root: trim_seps(root).to_string(), resolved: strip_verbatim(trim_seps(&resolved)).to_string() }
    }

    /// Path relative to the watched root in either spelling (`Some("")` for the root itself).
    pub fn relative<'a>(&self, path: &'a str) -> Option<&'a str> {
        let path = trim_seps(path);
        for root in [&self.root, &self.resolved] {
            if eq_path(path, root) {
                return Some("");
            }
            if path.len() > root.len() && eq_path(&path[..root.len().min(path.len())], root) {
                let rest = &path[root.len()..];
                if rest.starts_with('/') || rest.starts_with('\\') {
                    return Some(&rest[1..]);
                }
                // The root itself ends with a separator ("/" or "C:\").
                if root.ends_with('/') || root.ends_with('\\') {
                    return Some(rest);
                }
            }
        }
        None
    }

    /// `rootPath + "/" + relative` in the caller's spelling, or `None` for hidden / foreign paths.
    /// An event on the root itself (moved or deleted) reports the root.
    pub fn map(&self, path: &str) -> Option<String> {
        let relative = self.relative(path)?;
        if relative.is_empty() {
            return Some(self.root.clone());
        }
        if relative.split(['/', '\\']).any(|c| c.starts_with('.')) {
            return None;
        }
        let sep = if self.root.contains('\\') && !self.root.contains('/') { '\\' } else { '/' };
        let tail: String = if sep == '\\' { relative.replace('/', "\\") } else { relative.to_string() };
        if self.root.ends_with('/') || self.root.ends_with('\\') {
            Some(format!("{}{}", self.root, tail))
        } else {
            Some(format!("{}{}{}", self.root, sep, tail))
        }
    }
}

fn strip_verbatim(p: &str) -> &str {
    p.strip_prefix("\\\\?\\").unwrap_or(p)
}

fn eq_path(a: &str, b: &str) -> bool {
    if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }
}

/// Events worth reporting. `Access` events are reads (ignored) except a completed write
/// (`Close(Write)`, which is how inotify reports the end of a save).
fn is_relevant(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

// ---------------------------------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------------------------------

#[derive(Default)]
struct Batch {
    seen: HashSet<String>,
    paths: Vec<String>,
}

impl Batch {
    fn push(&mut self, path: String) {
        if self.seen.insert(path.clone()) {
            self.paths.push(path);
        }
    }

    fn take(&mut self) -> Vec<String> {
        self.seen.clear();
        std::mem::take(&mut self.paths)
    }
}

fn worker<F: Fn(Vec<String>)>(
    rx: mpsc::Receiver<Msg>,
    mut watcher: RecommendedWatcher,
    mapper: PathMapper,
    stopped: Arc<AtomicBool>,
    callback: F,
) {
    let mut batch = Batch::default();
    let flush = |batch: &mut Batch| {
        if batch.paths.is_empty() {
            return;
        }
        let paths = batch.take();
        if !stopped.load(Ordering::SeqCst) {
            callback(paths);
        }
    };
    let mut ingest = |event: Event, batch: &mut Batch| {
        if !is_relevant(&event.kind) {
            return;
        }
        let created = matches!(event.kind, EventKind::Create(_)) || matches!(event.kind, EventKind::Modify(ModifyKind::Name(RenameMode::To | RenameMode::Both)));
        for path in &event.paths {
            let text = path.to_string_lossy();
            let Some(mapped) = mapper.map(&text) else { continue };
            if cfg!(target_os = "linux") && created && path.is_dir() {
                // A new directory: watch it and report what was already put into it.
                for found in add_subdirectories_reporting(&mut watcher, path, &mapper) {
                    batch.push(found);
                }
            }
            batch.push(mapped);
        }
    };
    loop {
        // Quiet: wait for the first event, deliver it at once (NoDefer).
        let first = match rx.recv() {
            Ok(Msg::Event(e)) => e,
            Ok(Msg::Stop) | Err(_) => return,
        };
        ingest(first, &mut batch);
        loop {
            match rx.try_recv() {
                Ok(Msg::Event(e)) => ingest(e, &mut batch),
                Ok(Msg::Stop) | Err(mpsc::TryRecvError::Disconnected) => return,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        flush(&mut batch);
        // Busy: batch per latency window until a window stays quiet.
        loop {
            let deadline = Instant::now() + LATENCY;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                match rx.recv_timeout(deadline - now) {
                    Ok(Msg::Event(e)) => ingest(e, &mut batch),
                    Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => break,
                }
            }
            if batch.paths.is_empty() {
                break;
            }
            flush(&mut batch);
        }
    }
}

/// Linux: places a watch on every non-hidden directory below `dir` (the folders already watched are skipped by inotify).
fn add_subdirectories(watcher: &mut RecommendedWatcher, dir: &Path, mapper: &PathMapper) {
    let _ = add_subdirectories_reporting(watcher, dir, mapper);
}

/// Like [`add_subdirectories`] but also returns the mapped paths of everything found below `dir`: files that
/// were put into a new directory before its watch existed would otherwise never be reported.
fn add_subdirectories_reporting(watcher: &mut RecommendedWatcher, dir: &Path, mapper: &PathMapper) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        // Hidden directories are never watched (`.git` alone can hold thousands of folders).
        if mapper.map(&current.to_string_lossy()).is_none() {
            continue;
        }
        let _ = watcher.watch(&current, RecursiveMode::NonRecursive);
        let Ok(entries) = std::fs::read_dir(&current) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(mapped) = mapper.map(&path.to_string_lossy()) else { continue };
            found.push(mapped);
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(path);
            }
        }
    }
    found
}

// ---------------------------------------------------------------------------------------------------
// Registry for the watch_project / unwatch_project commands
// ---------------------------------------------------------------------------------------------------

fn registry() -> &'static Mutex<HashMap<String, WatchHandle>> {
    static CELL: OnceLock<Mutex<HashMap<String, WatchHandle>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Watches `root` (replacing a previous watcher of the same root).
pub fn watch<F>(root: &str, callback: F) -> Result<(), String>
where
    F: Fn(Vec<String>) + Send + 'static,
{
    let handle = start(root, callback)?;
    let old = registry().lock().unwrap_or_else(|e| e.into_inner()).insert(root.to_string(), handle);
    drop(old);
    Ok(())
}

/// Stops the watcher of `root` (no-op when there is none).
pub fn unwatch(root: &str) {
    let old = registry().lock().unwrap_or_else(|e| e.into_inner()).remove(root);
    drop(old);
}

/// Stops every watcher (app quit).
pub fn unwatch_all() {
    let all: Vec<WatchHandle> = registry().lock().unwrap_or_else(|e| e.into_inner()).drain().map(|(_, h)| h).collect();
    drop(all);
}

// ---------------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gabetex-watch-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Same spelling the watcher will report on every platform.
        dir
    }

    fn collect(root: &Path) -> (WatchHandle, Arc<Mutex<Vec<Vec<String>>>>) {
        let batches = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
        let sink = batches.clone();
        let handle = start(root.to_str().unwrap(), move |paths| sink.lock().unwrap().push(paths)).unwrap();
        (handle, batches)
    }

    fn all_names(b: &Arc<Mutex<Vec<Vec<String>>>>) -> HashSet<String> {
        b.lock().unwrap().iter().flatten().map(|p| Path::new(p).file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    fn wait_for(b: &Arc<Mutex<Vec<Vec<String>>>>, want: &[&str]) -> HashSet<String> {
        let mut names = HashSet::new();
        for _ in 0..60 {
            names = all_names(b);
            if want.iter().all(|w| names.contains(*w)) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        names
    }

    #[test]
    fn path_mapper_filters_hidden_and_foreign_paths() {
        let m = PathMapper { root: "/p/proj".into(), resolved: "/private/p/proj".into() };
        assert_eq!(m.map("/p/proj/a.tex").as_deref(), Some("/p/proj/a.tex"));
        assert_eq!(m.map("/private/p/proj/sub/b.tex").as_deref(), Some("/p/proj/sub/b.tex"), "resolved spelling is mapped back");
        assert_eq!(m.map("/p/proj").as_deref(), Some("/p/proj"));
        assert_eq!(m.map("/p/proj/").as_deref(), Some("/p/proj"));
        assert_eq!(m.map("/p/proj/.git/HEAD"), None);
        assert_eq!(m.map("/p/proj/sub/.a.tex.swp"), None);
        assert_eq!(m.map("/p/proj/.DS_Store"), None);
        assert_eq!(m.map("/p/project/a.tex"), None, "sibling with the same prefix");
        assert_eq!(m.map("/elsewhere/a.tex"), None);
        // A hidden ancestor *above* the root does not matter.
        let hidden_root = PathMapper { root: "/home/u/.work/proj".into(), resolved: "/home/u/.work/proj".into() };
        assert_eq!(hidden_root.map("/home/u/.work/proj/a.tex").as_deref(), Some("/home/u/.work/proj/a.tex"));
        let win = PathMapper { root: "C:\\Users\\u\\proj".into(), resolved: "C:\\Users\\u\\proj".into() };
        assert_eq!(win.map("C:\\Users\\u\\proj\\sub\\a.tex").as_deref(), Some("C:\\Users\\u\\proj\\sub\\a.tex"));
        assert_eq!(win.map("C:\\Users\\u\\proj\\.git\\x"), None);
        let top = PathMapper { root: "/".into(), resolved: "/".into() };
        assert_eq!(top.map("/a/b").as_deref(), Some("/a/b"));
    }

    #[test]
    fn relevance_of_event_kinds() {
        assert!(is_relevant(&EventKind::Create(notify::event::CreateKind::File)));
        assert!(is_relevant(&EventKind::Access(AccessKind::Close(AccessMode::Write))));
        assert!(!is_relevant(&EventKind::Access(AccessKind::Open(AccessMode::Any))));
        assert!(!is_relevant(&EventKind::Access(AccessKind::Close(AccessMode::Read))));
        assert!(is_relevant(&EventKind::Remove(notify::event::RemoveKind::Any)));
    }

    #[test]
    fn delivers_changes_ignores_hidden_and_stops() {
        let dir = scratch("basic");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        let (handle, batches) = collect(&dir);
        assert!(handle.is_running());
        thread::sleep(Duration::from_millis(300));
        fs::write(dir.join("a.tex"), "x").unwrap();
        fs::write(dir.join(".hidden"), "x").unwrap();
        fs::write(dir.join(".git").join("HEAD"), "x").unwrap();
        fs::write(dir.join("sub").join("b.tex"), "x").unwrap();
        let names = wait_for(&batches, &["a.tex", "b.tex"]);
        assert!(names.contains("a.tex") && names.contains("b.tex"), "changes delivered: {names:?}");
        assert!(!names.contains(".hidden") && !names.contains("HEAD") && !names.contains(".git"), "hidden paths ignored: {names:?}");
        let root = dir.to_str().unwrap();
        assert!(batches.lock().unwrap().iter().flatten().all(|p| p.starts_with(root)), "watched spelling");
        // Within one batch every path is unique.
        for b in batches.lock().unwrap().iter() {
            assert_eq!(b.len(), b.iter().collect::<HashSet<_>>().len());
        }
        handle.stop();
        thread::sleep(Duration::from_millis(300));
        let count = batches.lock().unwrap().len();
        fs::write(dir.join("c.tex"), "y").unwrap();
        thread::sleep(Duration::from_millis(600));
        assert_eq!(batches.lock().unwrap().len(), count, "no events after stop()");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn watches_new_directories() {
        let dir = scratch("newdir");
        let (handle, batches) = collect(&dir);
        thread::sleep(Duration::from_millis(300));
        fs::create_dir_all(dir.join("fresh").join("deeper")).unwrap();
        thread::sleep(Duration::from_millis(500));
        fs::write(dir.join("fresh").join("deeper").join("n.tex"), "x").unwrap();
        let names = wait_for(&batches, &["fresh", "n.tex"]);
        assert!(names.contains("fresh") && names.contains("n.tex"), "new directories are watched too: {names:?}");
        // A hidden directory created later is not watched.
        fs::create_dir_all(dir.join(".cache")).unwrap();
        thread::sleep(Duration::from_millis(500));
        fs::write(dir.join(".cache").join("z.tex"), "x").unwrap();
        thread::sleep(Duration::from_millis(600));
        assert!(!all_names(&batches).contains("z.tex"));
        drop(handle);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn coalesces_bursts_and_reports_removals() {
        let dir = scratch("burst");
        fs::write(dir.join("old.tex"), "x").unwrap();
        let (handle, batches) = collect(&dir);
        thread::sleep(Duration::from_millis(300));
        for i in 0..30 {
            fs::write(dir.join(format!("f{i}.tex")), "x").unwrap();
        }
        fs::remove_file(dir.join("old.tex")).unwrap();
        let names = wait_for(&batches, &["f29.tex", "old.tex"]);
        assert!(names.contains("f0.tex") && names.contains("f29.tex") && names.contains("old.tex"), "{names:?}");
        thread::sleep(Duration::from_millis(500));
        let n = batches.lock().unwrap().len();
        assert!(n <= 6, "a 31-file burst is coalesced into a few batches, got {n}");
        drop(handle);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn registry_replaces_and_unwatches() {
        let dir = scratch("registry");
        let hits = Arc::new(Mutex::new(0usize));
        let h1 = hits.clone();
        watch(dir.to_str().unwrap(), move |_| *h1.lock().unwrap() += 1).unwrap();
        let h2 = hits.clone();
        watch(dir.to_str().unwrap(), move |_| *h2.lock().unwrap() += 1).unwrap();
        thread::sleep(Duration::from_millis(300));
        fs::write(dir.join("a.tex"), "x").unwrap();
        thread::sleep(Duration::from_millis(700));
        assert!(*hits.lock().unwrap() >= 1);
        unwatch(dir.to_str().unwrap());
        thread::sleep(Duration::from_millis(300));
        let n = *hits.lock().unwrap();
        fs::write(dir.join("b.tex"), "x").unwrap();
        thread::sleep(Duration::from_millis(600));
        assert_eq!(*hits.lock().unwrap(), n);
        unwatch("/never/watched");
        assert!(start("/definitely/not/a/folder", |_| {}).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}

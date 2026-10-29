//! LaTeX compiler: port of `ShadowWorkspace.swift`, `LaTeXCompiler.swift`, `Compiler.swift`,
//! `AsyncLock.swift`, `ChkTeX.swift` and the pure part of `SectionPageLocator.swift`.
//!
//! The compile runs latexmk in a **shadow workspace** (a private mirror of the project, see
//! [`ShadowWorkspace`]) so unsaved editor buffers can be compiled and TeX never writes into the user's
//! folders. One compile per project at a time: a newer request for the same project bumps a generation
//! counter, which terminates the process tree of the compile in flight (and makes waiters give up).
//!
//! All paths handled here are `String`s with `/` separators (drive letters kept on Windows), i.e. the form
//! `log_parser::lexically_standardized` produces. Links: symlinks on Unix; on Windows symlink, then
//! hard link (files) / junction (directories), then copy (project files only).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::core::distribution::{self, Distribution, RunOptions, Tool};
use crate::core::log_parser::{
    self, lexically_standardized, LatexmkOutputSummary, LogDiagnostic, ProjectFiles, Severity, ShadowPathMapper,
    Source,
};

// ---------------------------------------------------------------------------------------------------
// Small path helpers (strings with `/`)
// ---------------------------------------------------------------------------------------------------

/// Key for comparing file names: case-folded on Windows (case-insensitive file system), exact elsewhere.
fn key(s: &str) -> String {
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

fn eq_path(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase()
    } else {
        a == b
    }
}

/// `path` without `prefix` (which must end at a component boundary the caller included, e.g. "/proj/").
fn strip_prefix_os<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if cfg!(windows) {
        let (p, q) = (path.as_bytes(), prefix.as_bytes());
        if p.len() >= q.len() && p[..q.len()].eq_ignore_ascii_case(q) && path.is_char_boundary(q.len()) {
            Some(&path[q.len()..])
        } else {
            None
        }
    } else {
        path.strip_prefix(prefix)
    }
}

fn join(base: &str, rel: &str) -> String {
    if rel.is_empty() {
        base.to_string()
    } else if base.is_empty() || base.ends_with('/') {
        format!("{base}{rel}")
    } else {
        format!("{base}/{rel}")
    }
}

fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => "",
    }
}

fn last_component(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `NSString.pathExtension`: the text after the last dot of the last component (none for dot-files).
fn path_extension(name: &str) -> &str {
    let last = last_component(name);
    match last.rfind('.') {
        Some(i) if i > 0 => &last[i + 1..],
        _ => "",
    }
}

fn delete_extension(name: &str) -> &str {
    let ext = path_extension(name);
    if ext.is_empty() {
        name
    } else {
        &name[..name.len() - ext.len() - 1]
    }
}

/// `realpath(3)` as a `/` path without Windows' `\\?\` prefix; `None` if it does not exist.
pub fn real_path(path: &str) -> Option<String> {
    let c = fs::canonicalize(path).ok()?;
    Some(distribution::strip_verbatim(&c.to_string_lossy()).replace('\\', "/"))
}

fn norm_target(t: &str) -> String {
    let s = t.replace('\\', "/");
    let s = s.strip_prefix("//?/").or_else(|| s.strip_prefix("/??/")).unwrap_or(&s).to_string();
    key(s.trim_end_matches('/'))
}

fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let h = distribution::sha256(format!("{}-{n}-{t}", std::process::id()).as_bytes());
    distribution::hex(&h[..4])
}

// ---------------------------------------------------------------------------------------------------
// Shadow workspace
// ---------------------------------------------------------------------------------------------------

/// The private mirror of a project in which latexmk runs.
///
/// ```text
/// <root>/tree/...             mirrors the project's ancestor folder A (`ANCESTOR_LEVELS` above the project):
///                             each entry of A and of every folder between A and the project is a link to
///                             the original, except the folders on the way down to the project (real folders)
/// <root>/tree/<a>/<b>/<proj>  the project mirror (`source_directory`): every project file is a link to the
///                             original, except overlays (unsaved buffers), which are real files
/// <root>/out/...              latexmk's -outdir, with the same directory tree as the project
/// ```
/// A symlinked directory of the project is mirrored as one link unless an overlay or the root document lies
/// below it: then it is *materialized* as a real directory of per-entry links, so overlays are written into
/// the shadow and never through a link into the user's files. Every directory, link and overlay is created
/// only after checking that the real path of its parent lies inside the shadow tree.
#[derive(Debug, Clone)]
pub struct ShadowWorkspace {
    pub project_directory: String,
    pub root: String,
}

/// Upper bound on mirrored project entries.
pub const MAXIMUM_ENTRIES: usize = 20_000;
/// How many folders above the project the ancestor layer mirrors.
pub const ANCESTOR_LEVELS: usize = 3;
/// An ancestor folder with more entries than this contributes no sibling links.
pub const MAXIMUM_SIBLINGS_PER_LEVEL: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    TooManyFiles(usize),
    Io(String),
}

impl Failure {
    pub fn message(&self) -> String {
        match self {
            Failure::TooManyFiles(limit) => format!(
                "The project folder contains more than {limit} files; open the folder that contains the document instead."
            ),
            Failure::Io(detail) => format!("Could not prepare the build folder: {detail}"),
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// What `refresh` found.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// `key(project-relative path)` -> (on-disk relative spelling, absolute project path).
    pub project_files: HashMap<String, (String, String)>,
    /// Project-relative directories (also created in `out/`), sorted.
    pub directories: Vec<String>,
}

#[derive(Clone)]
enum Desired {
    Dir,
    Link { target: String, copy_ok: bool },
    Overlay(Vec<u8>),
}

#[derive(Clone)]
struct Entry {
    spelling: String,
    kind: Desired,
}

enum Actual {
    Dir,
    Link(String),
    File,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryType {
    Directory,
    Symlink,
    Regular,
    Other,
}

impl ShadowWorkspace {
    pub fn new(project_directory: &str, root: &str) -> Self {
        ShadowWorkspace { project_directory: lexically_standardized(project_directory), root: lexically_standardized(root) }
    }

    pub fn tree_directory(&self) -> String {
        join(&self.root, "tree")
    }

    pub fn output_directory(&self) -> String {
        join(&self.root, "out")
    }

    /// The real folder mirrored at `tree_directory`, and the path components leading from it to the project.
    pub fn layout(&self) -> (String, Vec<String>) {
        Self::layout_for_project_path(&self.project_directory)
    }

    pub fn ancestor_directory(&self) -> String {
        self.layout().0
    }

    /// The project mirror: the shadow counterpart of `project_directory`.
    pub fn source_directory(&self) -> String {
        let (_, components) = self.layout();
        components.iter().fold(self.tree_directory(), |acc, c| join(&acc, c))
    }

    pub fn path_mapper(&self) -> ShadowPathMapper {
        ShadowPathMapper::with_ancestor(
            &self.project_directory,
            &self.source_directory(),
            &self.ancestor_directory(),
            &self.tree_directory(),
        )
    }

    pub fn layout_for_project_path(path: &str) -> (String, Vec<String>) {
        let (prefix, rest) = if path.len() >= 2 && path.as_bytes()[1] == b':' && path.as_bytes()[0].is_ascii_alphabetic() {
            (&path[..2], &path[2..])
        } else {
            ("", path)
        };
        let all: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
        let levels = ANCESTOR_LEVELS.min(all.len());
        let cut = all.len() - levels;
        let ancestor = format!("{prefix}/{}", all[..cut].join("/"));
        (ancestor, all[cut..].iter().map(|s| s.to_string()).collect())
    }

    /// `<16 hex digits of SHA-256(standardized project path)>`.
    pub fn identifier(project_directory: &str) -> String {
        let mut p = lexically_standardized(project_directory);
        if cfg!(windows) {
            p = p.to_lowercase();
        }
        distribution::hex(&distribution::sha256(p.as_bytes())[..8])
    }

    /// Names that are never mirrored: VCS metadata, Finder metadata and editor swap/backup files.
    pub fn is_excluded(name: &str) -> bool {
        match name {
            ".git" | ".svn" | ".hg" | ".DS_Store" => return true,
            _ => {}
        }
        if name.starts_with(".#") || name.ends_with('~') {
            return true; // Emacs lock, backups
        }
        if name.chars().count() > 2 && name.starts_with('#') && name.ends_with('#') {
            return true; // Emacs auto-save
        }
        if name.starts_with('.') {
            let ext = path_extension(name); // Vim .x.swp/.swo/.swn...
            if ext.chars().count() == 3 && ext.starts_with("sw") {
                return true;
            }
        }
        false
    }

    /// Brings the mirror (and the directory tree of `out/`) up to date with the project and the overlays.
    ///
    /// * `overlays`: unsaved buffer contents keyed by project file path (files outside the project are ignored);
    /// * `encodings`: "utf-8" / "latin-1" per overlay path (missing or unrepresentable: UTF-8);
    /// * `pinned`: further project files (the root document) whose directory must be a real shadow directory.
    pub fn refresh(
        &self,
        overlays: &HashMap<String, String>,
        encodings: &HashMap<String, String>,
        pinned: &[String],
    ) -> Result<Snapshot, Failure> {
        let project_path = self.project_directory.as_str();
        let project_base: &str = if project_path == "/" { "" } else { project_path };

        // 1. The project, keyed by project-relative path; values keep the on-disk spelling.
        let mut project: HashMap<String, Entry> = HashMap::new();
        let mut project_files: HashMap<String, (String, String)> = HashMap::new();
        let mut count = 0usize;
        let mut walk_error: Option<Failure> = None;
        walk(project_path, "", &Self::is_excluded, &mut |rel, ty| {
            count += 1;
            if count > MAXIMUM_ENTRIES {
                walk_error = Some(Failure::TooManyFiles(MAXIMUM_ENTRIES));
                return false;
            }
            if ty == EntryType::Directory {
                project.insert(key(rel), Entry { spelling: rel.to_string(), kind: Desired::Dir });
                return true;
            }
            let abs = format!("{project_base}/{rel}");
            project.insert(
                key(rel),
                Entry { spelling: rel.to_string(), kind: Desired::Link { target: abs.clone(), copy_ok: true } },
            );
            project_files.insert(key(rel), (rel.to_string(), abs));
            true
        });
        if let Some(e) = walk_error {
            return Err(e);
        }

        let mut b = Builder { project, project_files, base: project_base.to_string() };
        for url in pinned {
            if let Some(rel) = project_relative(url, project_base) {
                let _ = b.prepare_parents(&rel);
            }
        }

        let encoding_by_path: HashMap<String, &String> =
            encodings.iter().map(|(k, v)| (lexically_standardized(k), v)).collect();
        let mut overlay_list: Vec<(&String, &String)> = overlays.iter().collect();
        overlay_list.sort();
        for (url, text) in overlay_list {
            let std_url = lexically_standardized(url);
            let Some(rel) = project_relative(&std_url, project_base) else { continue };
            let Some(parent) = b.prepare_parents(&rel) else { continue };
            let name = last_component(&rel).to_string();
            let existing = b.project.get(&key(&rel)).cloned();
            if matches!(existing.as_ref().map(|e| &e.kind), Some(Desired::Dir)) {
                continue;
            }
            let spelling = existing
                .map(|e| e.spelling)
                .unwrap_or_else(|| if parent.is_empty() { name.clone() } else { format!("{parent}/{name}") });
            let encoding = encoding_by_path.get(&std_url).map(|s| s.as_str());
            b.project.insert(key(&rel), Entry { spelling: spelling.clone(), kind: Desired::Overlay(encode(text, encoding)) });
            b.project_files
                .entry(key(&rel))
                .or_insert_with(|| (spelling.clone(), format!("{project_base}/{spelling}")));
        }
        let Builder { project, project_files, .. } = b;

        // 2. Desired state of tree/, keyed by tree-relative path: ancestor layer + project mirror.
        let mut desired: HashMap<String, Entry> = HashMap::new();
        let (ancestor, components) = self.layout();
        let mut level = ancestor;
        let mut prefix = String::new();
        for component in &components {
            if let Ok(rd) = fs::read_dir(&level) {
                let names: Vec<String> = rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().into_string().ok()).collect();
                if names.len() <= MAXIMUM_SIBLINGS_PER_LEVEL {
                    let component_key = key(component);
                    for name in names.iter().filter(|n| !Self::is_excluded(n)) {
                        // On a case-insensitive volume never put a link next to the real path.
                        if key(name) == component_key {
                            continue;
                        }
                        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
                        let base = if level == "/" { String::new() } else { level.clone() };
                        desired.insert(
                            key(&rel),
                            Entry { spelling: rel, kind: Desired::Link { target: format!("{base}/{name}"), copy_ok: false } },
                        );
                    }
                }
            }
            prefix = if prefix.is_empty() { component.clone() } else { format!("{prefix}/{component}") };
            desired.insert(key(&prefix), Entry { spelling: prefix.clone(), kind: Desired::Dir });
            level = join(&level, component);
        }
        let mut project_directories: Vec<String> = Vec::new();
        for entry in project.values() {
            let rel = if prefix.is_empty() { entry.spelling.clone() } else { format!("{prefix}/{}", entry.spelling) };
            if matches!(entry.kind, Desired::Dir) {
                project_directories.push(entry.spelling.clone());
            }
            desired.insert(key(&rel), Entry { spelling: rel, kind: entry.kind.clone() });
        }

        // 3. What is in tree/ now (no exclusions: stale hidden entries must be removed too).
        let tree = self.tree_directory();
        let out = self.output_directory();
        for dir in [&tree, &out] {
            fs::create_dir_all(dir).map_err(|e| Failure::Io(e.to_string()))?;
        }
        // Workspaces of earlier versions mirrored the project at <root>/src.
        let legacy = join(&self.root, "src");
        if Path::new(&legacy).exists() {
            let _ = fs::remove_dir_all(&legacy);
        }
        let real_tree = real_path(&tree).ok_or_else(|| Failure::Io(format!("cannot resolve {tree}")))?;

        let mut actual: HashMap<String, (String, Actual)> = HashMap::new();
        walk(&tree, "", &|_| false, &mut |rel, ty| {
            let kind = match ty {
                EntryType::Directory => Actual::Dir,
                EntryType::Symlink => Actual::Link(read_link(&join(&tree, rel)).unwrap_or_default()),
                EntryType::Regular => Actual::File,
                EntryType::Other => Actual::Other,
            };
            actual.insert(key(rel), (rel.to_string(), kind));
            true
        });

        // 4. Remove stale or mismatching entries (a removed directory takes its subtree with it). The walk
        //    never descends into links, so every path here is physically inside tree/.
        let mut removed_prefixes: Vec<String> = Vec::new();
        let mut actual_keys: Vec<String> = actual.keys().cloned().collect();
        actual_keys.sort();
        for k in &actual_keys {
            if removed_prefixes.iter().any(|p| k.starts_with(p.as_str())) {
                continue;
            }
            let Some((spelling, kind)) = actual.get(k) else { continue };
            let path = join(&tree, spelling);
            let keep = match (kind, desired.get(k).map(|d| &d.kind)) {
                (Actual::Dir, Some(Desired::Dir)) => true,
                (Actual::Link(a), Some(Desired::Link { target, .. })) => norm_target(a) == norm_target(target),
                (Actual::File, Some(Desired::Overlay(_))) => true, // contents compared below
                // Windows fallback: a hard link / copy of a project file is up to date when size + mtime match.
                (Actual::File, Some(Desired::Link { target, .. })) => cfg!(windows) && same_file_metadata(&path, target),
                _ => false,
            };
            if keep {
                continue;
            }
            if matches!(kind, Actual::Dir) {
                removed_prefixes.push(format!("{k}/"));
                fs::remove_dir_all(&path).map_err(|e| Failure::Io(e.to_string()))?;
            } else if let Err(e) = remove_entry(&path) {
                if e.kind() != io::ErrorKind::NotFound {
                    return Err(Failure::Io(format!("{e} ({path})")));
                }
            }
            actual.remove(k);
        }
        for removed in &removed_prefixes {
            actual.retain(|k, _| !k.starts_with(removed.as_str()));
        }

        // 5. Create what is missing: directories first (parents before children), then links and overlays.
        let mut verified_parents: HashSet<String> = HashSet::new();
        let mut check_parent = |path: &str| -> Result<(), Failure> {
            let parent = parent_of(path).to_string();
            if verified_parents.contains(&parent) {
                return Ok(());
            }
            require_inside(&parent, &real_tree)?;
            verified_parents.insert(parent);
            Ok(())
        };
        let mut ordered: Vec<(&String, &Entry)> = desired.iter().collect();
        ordered.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(b.0)));
        for (k, entry) in &ordered {
            if !matches!(entry.kind, Desired::Dir) || actual.contains_key(*k) {
                continue;
            }
            let path = join(&tree, &entry.spelling);
            check_parent(&path)?;
            match fs::create_dir(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(Failure::Io(format!("{e} ({})", entry.spelling))),
            }
        }
        let mut dirs_sorted = project_directories.clone();
        dirs_sorted.sort_by_key(|d| d.len());
        for directory in &dirs_sorted {
            match fs::create_dir(join(&out, directory)) {
                Ok(()) => {}
                Err(e) if matches!(e.kind(), io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound) => {}
                Err(e) => return Err(Failure::Io(format!("{e} (out/{directory})"))),
            }
        }
        let source_path = self.source_directory();
        for (k, entry) in &ordered {
            let path = join(&tree, &entry.spelling);
            match &entry.kind {
                Desired::Dir => {}
                Desired::Link { target, copy_ok } => {
                    if actual.contains_key(*k) {
                        continue;
                    }
                    check_parent(&path)?;
                    match make_link(target, &path, *copy_ok) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                        // Ancestor-layer siblings are best effort when links are not allowed (Windows).
                        Err(_) if !copy_ok => {}
                        Err(e) => return Err(Failure::Io(format!("{e} ({})", entry.spelling))),
                    }
                }
                Desired::Overlay(data) => {
                    if actual.contains_key(*k) {
                        if let Ok(existing) = fs::read(&path) {
                            if &existing == data {
                                continue;
                            }
                        }
                    }
                    write_atomically(data, &path, &source_path)?;
                }
            }
        }
        let mut directories = project_directories;
        directories.sort();
        Ok(Snapshot { project_files, directories })
    }

    /// Removes the whole workspace.
    pub fn remove(&self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Mutable state of step 1 of `refresh` (project listing + materialization of linked folders).
struct Builder {
    project: HashMap<String, Entry>,
    project_files: HashMap<String, (String, String)>,
    base: String,
}

impl Builder {
    /// Replaces the link `spelling` (a directory of the project reached through a link) by a real directory
    /// whose entries are links; false if the link does not lead to a directory.
    fn materialize(&mut self, spelling: &str) -> bool {
        let real = format!("{}/{}", self.base, spelling);
        if let Ok(meta) = fs::metadata(&real) {
            if !meta.is_dir() {
                return false;
            }
        }
        self.project.insert(key(spelling), Entry { spelling: spelling.to_string(), kind: Desired::Dir });
        // Reading through the user's link is fine; only writes must never go through it.
        let mut names: Vec<String> = fs::read_dir(&real)
            .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().into_string().ok()).collect())
            .unwrap_or_default();
        names.sort();
        for name in names.iter().filter(|n| !ShadowWorkspace::is_excluded(n)) {
            let child = format!("{spelling}/{name}");
            if self.project.contains_key(&key(&child)) {
                continue;
            }
            let abs = format!("{}/{}", self.base, child);
            self.project.insert(
                key(&child),
                Entry { spelling: child.clone(), kind: Desired::Link { target: abs.clone(), copy_ok: true } },
            );
            if fs::metadata(&abs).map(|m| !m.is_dir()).unwrap_or(false) {
                self.project_files.insert(key(&child), (child, abs));
            }
        }
        true
    }

    /// Makes every parent of `relative` a real directory of the mirror; returns the on-disk spelling of the
    /// parent, or `None` when a parent is a file (nothing can live below it).
    fn prepare_parents(&mut self, relative: &str) -> Option<String> {
        let comps: Vec<&str> = relative.split('/').filter(|s| !s.is_empty()).collect();
        let mut parent = String::new();
        for component in comps.iter().take(comps.len().saturating_sub(1)) {
            let candidate = if parent.is_empty() { component.to_string() } else { format!("{parent}/{component}") };
            let kind = self.project.get(&key(&candidate)).map(|e| e.kind.clone());
            match kind {
                Some(Desired::Dir) => {}
                Some(Desired::Link { .. }) => {
                    let spelling = self.project.get(&key(&candidate))?.spelling.clone();
                    if !self.materialize(&spelling) {
                        return None;
                    }
                }
                Some(Desired::Overlay(_)) => return None,
                None => {
                    // A folder that does not exist on disk yet.
                    self.project.insert(key(&candidate), Entry { spelling: candidate.clone(), kind: Desired::Dir });
                }
            }
            parent = self.project.get(&key(&candidate)).map(|e| e.spelling.clone()).unwrap_or(candidate);
        }
        Some(parent)
    }
}

fn project_relative(path: &str, project_base: &str) -> Option<String> {
    let std = lexically_standardized(path);
    let rel = strip_prefix_os(&std, &format!("{project_base}/"))?;
    (!rel.is_empty()).then(|| rel.to_string())
}

/// `text` in `encoding` ("latin-1"/"iso-8859-1"); UTF-8 when nil, unknown, or when the text is not
/// representable in it.
pub fn encode(text: &str, encoding: Option<&str>) -> Vec<u8> {
    if let Some(enc) = encoding {
        let e = enc.to_ascii_lowercase();
        if (e == "latin-1" || e == "latin1" || e == "iso-8859-1" || e == "isolatin1")
            && text.chars().all(|c| (c as u32) <= 0xFF) {
                return text.chars().map(|c| c as u32 as u8).collect();
            }
    }
    text.as_bytes().to_vec()
}

// -- file-system helpers ---------------------------------------------------------------------------

/// Depth-first walk that does not follow links and skips names for which `skipping` returns true. `visit`
/// returns false to stop the walk.
pub fn walk(
    directory: &str,
    relative: &str,
    skipping: &dyn Fn(&str) -> bool,
    visit: &mut dyn FnMut(&str, EntryType) -> bool,
) -> bool {
    let Ok(rd) = fs::read_dir(directory) else { return true };
    let mut names: Vec<String> = rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().into_string().ok()).collect();
    names.sort();
    for name in names.iter().filter(|n| !skipping(n)) {
        let full = format!("{directory}/{name}");
        let rel = if relative.is_empty() { name.clone() } else { format!("{relative}/{name}") };
        let Ok(meta) = fs::symlink_metadata(&full) else { continue };
        let ft = meta.file_type();
        let ty = if ft.is_symlink() {
            EntryType::Symlink
        } else if ft.is_dir() {
            EntryType::Directory
        } else if ft.is_file() {
            EntryType::Regular
        } else {
            EntryType::Other
        };
        if !visit(&rel, ty) {
            return false;
        }
        if ty == EntryType::Directory && !walk(&full, &rel, skipping, visit) {
            return false;
        }
    }
    true
}

pub fn read_link(path: &str) -> Option<String> {
    fs::read_link(path).ok().map(|p| p.to_string_lossy().replace('\\', "/"))
}

/// Throws unless the real path of `directory` is `root` (already a real path) or lies below it.
pub fn require_inside(directory: &str, root: &str) -> Result<(), Failure> {
    let real = real_path(directory).ok_or_else(|| Failure::Io(format!("cannot resolve {directory}")))?;
    let inside = if root == "/" {
        true
    } else {
        eq_path(&real, root) || strip_prefix_os(&real, &format!("{root}/")).is_some()
    };
    if inside {
        Ok(())
    } else {
        Err(Failure::Io(format!("refusing to write outside the build folder: {directory} leads to {real}")))
    }
}

/// Writes to a new temporary sibling and renames it over `path` (replacing a link, never following it).
/// Hard guard: fails, without touching anything, unless the real path of `path`'s parent lies inside `root`.
pub fn write_atomically(data: &[u8], path: &str, root: &str) -> Result<(), Failure> {
    let real_root = real_path(root).ok_or_else(|| Failure::Io(format!("cannot resolve {root}")))?;
    require_inside(parent_of(path), &real_root)?;
    let temporary = format!("{path}.gabetex-{}.tmp", unique_suffix());
    let name = last_component(path).to_string();
    let result = (|| -> io::Result<()> {
        use std::io::Write;
        // create_new = O_CREAT|O_EXCL: never follows a (dangling) link at the temporary name.
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        f.write_all(data)?;
        f.flush()?;
        drop(f);
        fs::rename(&temporary, path)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&temporary);
        return Err(Failure::Io(format!("{e} ({name})")));
    }
    Ok(())
}

/// Removes a file or a link (also a directory link on Windows).
fn remove_entry(path: &str) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) => {
            if fs::remove_dir(path).is_ok() {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

fn same_file_metadata(a: &str, b: &str) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.len() == y.len() && x.modified().ok() == y.modified().ok(),
        _ => false,
    }
}

/// Unix: symlink. Windows: symlink, then hard link (files) / junction (directories), then (if `copy_ok`) a copy.
fn make_link(target: &str, path: &str, copy_ok: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        let _ = copy_ok;
        std::os::unix::fs::symlink(target, path)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::{symlink_dir, symlink_file};
        let t = target.replace('/', "\\");
        let p = path.replace('/', "\\");
        let is_dir = fs::metadata(target).map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            symlink_dir(&t, &p).or_else(|_| make_junction(&t, &p))
        } else {
            symlink_file(&t, &p).or_else(|e| {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    return Err(e);
                }
                fs::hard_link(&t, &p).or_else(|_| if copy_ok { fs::copy(&t, &p).map(|_| ()) } else { Err(e) })
            })
        }
    }
}

#[cfg(windows)]
fn make_junction(target: &str, path: &str) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    let status = distribution::command("cmd")
        .arg("/C")
        .raw_arg(format!("mklink /J \"{path}\" \"{target}\""))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "mklink /J failed"))
    }
}

// ---------------------------------------------------------------------------------------------------
// Per-workspace lock + generation counter (AsyncLock.swift / task cancellation)
// ---------------------------------------------------------------------------------------------------

/// A mutex whose waiters can give up: `acquire` returns `None` when `cancelled()` became true while
/// waiting, so a superseded compile request does not wait for the run in progress.
#[derive(Default)]
pub struct WorkspaceLock {
    locked: Mutex<bool>,
    cv: Condvar,
}

pub struct LockGuard<'a>(&'a WorkspaceLock);

impl WorkspaceLock {
    pub fn acquire(&self, cancelled: &dyn Fn() -> bool) -> Option<LockGuard<'_>> {
        let mut locked = self.locked.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if cancelled() {
                return None;
            }
            if !*locked {
                *locked = true;
                return Some(LockGuard(self));
            }
            let (guard, _) = self.cv.wait_timeout(locked, Duration::from_millis(20)).unwrap_or_else(|e| e.into_inner());
            locked = guard;
        }
    }
}

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        *self.0.locked.lock().unwrap_or_else(|e| e.into_inner()) = false;
        self.0.cv.notify_one();
    }
}

#[derive(Default)]
struct Slot {
    generation: AtomicU64,
    lock: WorkspaceLock,
}

/// One slot per shadow workspace, shared by every `LaTeXCompiler` in the process.
fn slot_for(workspace_root: &str) -> Arc<Slot> {
    static SLOTS: OnceLock<Mutex<HashMap<String, Arc<Slot>>>> = OnceLock::new();
    let map = SLOTS.get_or_init(|| Mutex::new(HashMap::new()));
    map.lock().unwrap_or_else(|e| e.into_inner()).entry(key(workspace_root)).or_default().clone()
}

// ---------------------------------------------------------------------------------------------------
// Request / result types (Compiler.swift)
// ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Engine {
    Pdflatex,
    Xelatex,
    Lualatex,
}

impl Engine {
    pub fn latexmk_flag(self) -> &'static str {
        match self {
            Engine::Pdflatex => "-pdf",
            Engine::Xelatex => "-xelatex",
            Engine::Lualatex => "-lualatex",
        }
    }

    pub fn tool(self) -> Tool {
        match self {
            Engine::Pdflatex => Tool::Pdflatex,
            Engine::Xelatex => Tool::Xelatex,
            Engine::Lualatex => Tool::Lualatex,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Engine::Pdflatex => "pdfLaTeX",
            Engine::Xelatex => "XeLaTeX",
            Engine::Lualatex => "LuaLaTeX",
        }
    }

    /// "pdflatex" | "xelatex" | "lualatex" (IPC value).
    pub fn parse(s: &str) -> Option<Engine> {
        match s.to_ascii_lowercase().as_str() {
            "pdflatex" => Some(Engine::Pdflatex),
            "xelatex" => Some(Engine::Xelatex),
            "lualatex" => Some(Engine::Lualatex),
            _ => None,
        }
    }

    /// Parses `% !TEX program = ...` values ("xelatex", "XeLaTeX", "lualatex", "latexmk -xelatex"...).
    pub fn from_magic_program(program: &str) -> Option<Engine> {
        let p = program.to_lowercase();
        if p.contains("xelatex") || p.contains("xetex") {
            Some(Engine::Xelatex)
        } else if p.contains("lualatex") || p.contains("luatex") {
            Some(Engine::Lualatex)
        } else if p.contains("pdflatex") || p.contains("pdftex") || p == "latex" {
            Some(Engine::Pdflatex)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Debounced background compile while typing: uses unsaved buffers, never touches project files.
    Live,
    /// Explicit build: additionally copies the PDF atomically next to the root file.
    Build,
}

#[derive(Debug, Clone)]
pub struct CompileRequest {
    pub project_directory: String,
    pub root_file: String,
    pub engine: Engine,
    /// Unsaved editor contents keyed by project file path.
    pub overlays: HashMap<String, String>,
    /// "utf-8" | "latin-1" per overlay path.
    pub overlay_encodings: HashMap<String, String>,
    pub purpose: Purpose,
    pub shell_escape: bool,
    pub extra_arguments: Vec<String>,
    /// Wall-clock limit of the whole run; `None` = 5 minutes for live, 30 minutes for build.
    pub timeout: Option<Duration>,
}

impl CompileRequest {
    pub fn new(project_directory: &str, root_file: &str, engine: Engine, purpose: Purpose) -> Self {
        CompileRequest {
            project_directory: project_directory.to_string(),
            root_file: root_file.to_string(),
            engine,
            overlays: HashMap::new(),
            overlay_encodings: HashMap::new(),
            purpose,
            shell_escape: false,
            extra_arguments: Vec::new(),
            timeout: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileStatus {
    Succeeded,
    /// TeX finished but reported errors (a PDF may still exist).
    Failed,
    Cancelled,
    /// Could not run at all (no distribution, latexmk missing, ...). Message is user-facing.
    ToolError(String),
}

#[derive(Debug, Clone)]
pub struct CompileOutcome {
    pub status: CompileStatus,
    /// PDF produced in the shadow output directory (stable per project; what the preview displays).
    pub pdf_path: Option<String>,
    /// For `Purpose::Build`: the PDF copied next to the root file.
    pub exported_pdf_path: Option<String>,
    pub synctex_path: Option<String>,
    pub log_path: Option<String>,
    /// Diagnostics mapped to project paths, errors first then by file/line.
    pub diagnostics: Vec<LogDiagnostic>,
    /// Raw TeX log.
    pub log: String,
    /// latexmk's stdout/stderr.
    pub tool_output: String,
    pub duration: Duration,
    /// Files TeX reported as missing (e.g. ["tikz-cd.sty"]).
    pub missing_files: Vec<String>,
    pub path_mapper: ShadowPathMapper,
}

impl CompileOutcome {
    pub fn ok(&self) -> bool {
        self.status == CompileStatus::Succeeded
    }

    /// IPC shape. A tool error becomes one error diagnostic on the root file so the UI shows it.
    pub fn to_ipc(&self, root_file: &str) -> crate::commands::CompileResult {
        let mut diagnostics: Vec<crate::commands::Diagnostic> = self.diagnostics.iter().map(|d| d.to_ipc()).collect();
        if let CompileStatus::ToolError(message) = &self.status {
            diagnostics.insert(
                0,
                crate::commands::Diagnostic {
                    severity: "error".into(),
                    message: message.clone(),
                    path: root_file.to_string(),
                    line: 1,
                    column: None,
                    source: "latex".into(),
                    detail: None,
                    length: None,
                    missing_file: None,
                },
            );
        }
        crate::commands::CompileResult {
            ok: self.ok(),
            pdf_path: self.pdf_path.clone(),
            diagnostics,
            missing_packages: self.missing_files.clone(),
            duration_ms: self.duration.as_millis() as u64,
            log_path: self.log_path.clone(),
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// LaTeXCompiler
// ---------------------------------------------------------------------------------------------------

pub struct LaTeXCompiler {
    pub distribution: Distribution,
    /// Folder that holds one shadow workspace per project.
    pub shadow_root: String,
}

/// `<cache dir>/io.github.gabe-rbo.GabeTeX/Shadow` (XDG cache on Linux, `%LOCALAPPDATA%` on Windows).
pub fn default_shadow_root() -> String {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)
    } else if cfg!(target_os = "macos") {
        distribution::home_dir().map(|h| h.join("Library").join("Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| distribution::home_dir().map(|h| h.join(".cache")))
    }
    .unwrap_or_else(std::env::temp_dir);
    lexically_standardized(&base.join("io.github.gabe-rbo.GabeTeX").join("Shadow").to_string_lossy())
}

/// Compile the IPC way: detect TeX (cached), compile live with the given overrides.
pub fn compile_ipc(
    root: &str,
    root_file: &str,
    engine: Option<&str>,
    overrides: &HashMap<String, String>,
    purpose: Purpose,
) -> Result<crate::commands::CompileResult, String> {
    let dist = distribution::detect_cached()
        .ok_or_else(|| "No TeX distribution was found. Install TeX Live (or MiKTeX on Windows).".to_string())?;
    let engine = engine.and_then(Engine::parse).unwrap_or(Engine::Pdflatex);
    let mut request = CompileRequest::new(root, root_file, engine, purpose);
    request.overlays = overrides.clone();
    let outcome = LaTeXCompiler::new(dist).compile(&request);
    Ok(outcome.to_ipc(root_file))
}

/// Cancels the compile in flight (and queued ones) of a project. Returns immediately.
pub fn cancel_compile(project_directory: &str, shadow_root: Option<&str>) {
    let root = shadow_root.map(lexically_standardized).unwrap_or_else(default_shadow_root);
    let workspace_root = join(&root, &ShadowWorkspace::identifier(project_directory));
    slot_for(&workspace_root).generation.fetch_add(1, Ordering::SeqCst);
}

/// Synchronously kills every TeX process tree started by this module still running (never a tlmgr
/// install). Call when the app quits.
pub fn terminate_all_running_processes() -> usize {
    distribution::terminate_all()
}

struct Run {
    text: String,
    exit_code: Option<i32>,
    timed_out: bool,
    cancelled: bool,
}

impl LaTeXCompiler {
    pub fn new(distribution: Distribution) -> Self {
        LaTeXCompiler { distribution, shadow_root: default_shadow_root() }
    }

    pub fn with_shadow_root(distribution: Distribution, shadow_root: &str) -> Self {
        LaTeXCompiler { distribution, shadow_root: lexically_standardized(shadow_root) }
    }

    /// The shadow workspace of a project (stable per project path): contains `tree/` and `out/`.
    pub fn shadow_directory(&self, project_directory: &str) -> String {
        join(&self.shadow_root, &ShadowWorkspace::identifier(project_directory))
    }

    pub fn workspace(&self, project_directory: &str) -> ShadowWorkspace {
        ShadowWorkspace::new(project_directory, &self.shadow_directory(project_directory))
    }

    /// The mapper between project paths and the shadow mirror (the same value every outcome carries).
    pub fn path_mapper(&self, project_directory: &str) -> ShadowPathMapper {
        self.workspace(project_directory).path_mapper()
    }

    /// Cancels the compile in flight of this project.
    pub fn cancel(&self, project_directory: &str) {
        let ws = self.workspace(project_directory);
        slot_for(&ws.root).generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Removes the shadow workspace of a project. Waits for a running compile.
    pub fn clean(&self, project_directory: &str) {
        let ws = self.workspace(project_directory);
        let slot = slot_for(&ws.root);
        let never = || false;
        let _guard = slot.lock.acquire(&never);
        ws.remove();
    }

    /// Compiles. Blocks the calling thread (run it from a worker); a newer `compile` call for the same
    /// project supersedes this one (it returns `Cancelled`).
    pub fn compile(&self, request: &CompileRequest) -> CompileOutcome {
        let started = Instant::now();
        let dist = &self.distribution;
        let project = lexically_standardized(&request.project_directory);
        let workspace = ShadowWorkspace::new(&project, &self.shadow_directory(&project));
        let mapper = workspace.path_mapper();
        let early = |status: CompileStatus, tool_output: String| CompileOutcome {
            status,
            pdf_path: None,
            exported_pdf_path: None,
            synctex_path: None,
            log_path: None,
            diagnostics: Vec::new(),
            log: String::new(),
            tool_output,
            duration: started.elapsed(),
            missing_files: Vec::new(),
            path_mapper: mapper.clone(),
        };
        let tool_error = |m: String| early(CompileStatus::ToolError(m), String::new());

        let slot = slot_for(&workspace.root);
        let my_generation = slot.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let cancelled = || slot.generation.load(Ordering::SeqCst) != my_generation;
        let Some(_guard) = slot.lock.acquire(&cancelled) else {
            return early(CompileStatus::Cancelled, String::new());
        };
        if cancelled() {
            return early(CompileStatus::Cancelled, String::new());
        }

        let latexmk = dist.tool(Tool::Latexmk).map(|s| s.to_string());
        let direct_engine = dist.tool(request.engine.tool()).map(|s| s.to_string());
        if latexmk.is_none() && direct_engine.is_none() {
            return tool_error(format!("latexmk was not found in {}.", dist.name));
        }
        let project_base = if project == "/" { "" } else { project.as_str() };
        let root_path = lexically_standardized(&request.root_file);
        let Some(root_relative) = strip_prefix_os(&root_path, &format!("{project_base}/")).map(|s| s.to_string()) else {
            return tool_error("The root document is not inside the project folder.".to_string());
        };
        let root_relative_directory = parent_of(&root_relative).to_string();
        let root_name = last_component(&root_relative).to_string();
        let jobname = delete_extension(&root_name).to_string();

        let snapshot = match workspace.refresh(&request.overlays, &request.overlay_encodings, std::slice::from_ref(&request.root_file)) {
            Ok(s) => s,
            Err(e) => return tool_error(e.message()),
        };
        let Some((_, root_url_known)) = snapshot.project_files.get(&key(&root_relative)).cloned() else {
            return tool_error(format!("The root document {root_name} does not exist."));
        };
        if cancelled() {
            return early(CompileStatus::Cancelled, String::new());
        }

        let source_directory = workspace.source_directory();
        let cwd = join(&source_directory, &root_relative_directory);
        // TeX may write next to its input (minted, \write18 ...): never run it inside the user's folders.
        let tree = workspace.tree_directory();
        let real_tree = real_path(&tree).unwrap_or_else(|| tree.clone());
        let cwd_ok = real_path(&cwd)
            .map(|c| eq_path(&c, &real_tree) || strip_prefix_os(&c, &format!("{real_tree}/")).is_some())
            .unwrap_or(false);
        if !cwd_ok {
            return tool_error(format!("Could not prepare the build folder for {root_name}."));
        }
        let out_root = workspace.output_directory();
        let out_directory = join(&out_root, &root_relative_directory);
        let timeout = request.timeout.unwrap_or(if request.purpose == Purpose::Live {
            Duration::from_secs(300)
        } else {
            Duration::from_secs(1800)
        });
        let mut env = dist.process_environment();
        // bibtex runs from the compile folder and writes .bbl/.blg to the absolute out/ path; older bibtex
        // versions only allow that below TEXMFOUTPUT (openout_any = p).
        env.push(("TEXMFOUTPUT".into(), out_root.clone()));

        let run = if let Some(latexmk) = &latexmk {
            let arguments = latexmk_arguments(
                request.engine,
                &out_directory,
                request.shell_escape,
                &request.extra_arguments,
                &root_name,
            );
            let opts = RunOptions { env: env.clone(), cwd: Some(&cwd), timeout: Some(timeout), cancel: Some(&cancelled), ..Default::default() };
            match distribution::run(latexmk, &arguments, &opts) {
                Ok(o) => Run { text: o.combined_text(), exit_code: o.exit_code, timed_out: o.timed_out, cancelled: o.cancelled },
                Err(e) => return tool_error(e),
            }
        } else {
            let engine_exe = direct_engine.unwrap_or_default();
            match run_engine_directly(
                dist,
                &engine_exe,
                request,
                &env,
                &cwd,
                &out_directory,
                &jobname,
                &root_name,
                timeout,
                &cancelled,
            ) {
                Ok(r) => r,
                Err(e) => return tool_error(e),
            }
        };
        let tool_output = run.text.clone();
        if run.cancelled || cancelled() {
            return early(CompileStatus::Cancelled, tool_output);
        }

        let log_path = join(&out_directory, &format!("{jobname}.log"));
        let Ok(log_data) = fs::read(&log_path) else {
            let reason = last_lines(&tool_output, 12);
            let tool = if latexmk.is_some() { "latexmk" } else { "TeX" };
            let message = if reason.is_empty() {
                format!("{tool} did not run TeX (exit status {}).", run.exit_code.unwrap_or(-1))
            } else {
                format!("{tool} did not run TeX: {reason}")
            };
            return early(CompileStatus::ToolError(message), tool_output);
        };
        let log = String::from_utf8_lossy(&log_data).into_owned();

        // Diagnostics ------------------------------------------------------------------------------
        let project_files = ProjectFiles::new(snapshot.project_files.values().map(|(rel, abs)| (rel.clone(), abs.clone())));
        let map_log_path = |raw: &str| log_parser::project_file_for_log_path(raw, &cwd, &mapper, &project_files);
        let root_url = root_url_known;
        let mut diagnostics = log_parser::parse(&log, map_log_path);

        let summary = log_parser::parse_latexmk_output(&tool_output);
        let fdb = fs::read(join(&out_directory, &format!("{jobname}.fdb_latexmk")))
            .map(|d| bibliography_rules(&String::from_utf8_lossy(&d)))
            .unwrap_or_default();
        let overlay_for = |path: &str| -> Option<&String> {
            let std = lexically_standardized(path);
            request.overlays.iter().find(|(k, _)| eq_path(&lexically_standardized(k), &std)).map(|(_, v)| v)
        };
        let root_text = overlay_for(&root_path).cloned().or_else(|| read_text(&root_path));
        diagnostics.extend(self.bibliography_diagnostics(
            &fdb,
            &summary,
            &root_url,
            root_text.as_deref(),
            &map_log_path,
            &snapshot,
        ));
        diagnostics.extend(
            summary
                .diagnostics
                .iter()
                .filter(|d| d.source != Source::Biber && d.source != Source::Bibtex)
                .cloned(),
        );

        // "! LaTeX Error: File `x.sty' not found." has no l.N context: point at the line that requests it.
        for d in diagnostics.iter_mut() {
            if d.line.is_some() {
                continue;
            }
            let (Some(name), Some(path)) = (d.missing_file.clone(), d.path.clone()) else { continue };
            let text = overlay_for(&path).cloned().or_else(|| read_text(&path));
            if let Some(line) = text.and_then(|t| log_parser::missing_file_line(&t, &name)) {
                d.line = Some(line);
            }
        }
        let real_root_directory = join(project_base, &root_relative_directory);
        let extra = unreachable_input_diagnostics(&diagnostics, &real_root_directory, &root_url);
        diagnostics.extend(extra);

        if run.timed_out {
            let mut d = LogDiagnostic::new(
                Severity::Error,
                Source::Latexmk,
                format!("Compilation was stopped after {} seconds. The document may contain an endless loop.", timeout.as_secs()),
            );
            d.path = Some(root_url.clone());
            diagnostics.push(d);
        }
        if !diagnostics.iter().any(|d| d.severity == Severity::Error) {
            if let Some(code) = run.exit_code.filter(|c| *c != 0) {
                let mut d = LogDiagnostic::new(
                    Severity::Error,
                    Source::Latexmk,
                    format!("latexmk reported a problem (exit status {code})."),
                );
                d.detail = Some(last_lines(&tool_output, 15));
                d.path = Some(root_url.clone());
                diagnostics.push(d);
            }
        }
        diagnostics = log_parser::normalized(diagnostics);
        let status = if diagnostics.iter().any(|d| d.severity == Severity::Error) {
            CompileStatus::Failed
        } else {
            CompileStatus::Succeeded
        };

        // Outputs ----------------------------------------------------------------------------------
        let default_pdf = join(&out_directory, &format!("{jobname}.pdf"));
        let pdf_path = pdf_location(&tool_output, &default_pdf);
        let synctex = join(&out_directory, &format!("{jobname}.synctex.gz"));
        let synctex_path = Path::new(&synctex).exists().then_some(synctex);
        let mut exported: Option<String> = None;
        if request.purpose == Purpose::Build && !log.contains("\nNo pages of output.") {
            if let Some(pdf) = &pdf_path {
                let destination = join(&join(project_base, &root_relative_directory), &format!("{jobname}.pdf"));
                match copy_atomically(pdf, &destination) {
                    Ok(()) => exported = Some(destination),
                    Err(e) => {
                        let mut d = LogDiagnostic::new(
                            Severity::Warning,
                            Source::Latexmk,
                            format!("Could not copy the PDF to {destination}: {e}"),
                        );
                        d.path = Some(root_url.clone());
                        diagnostics.push(d);
                    }
                }
            }
        }
        let missing = log_parser::missing_package_files(&diagnostics, &summary);

        CompileOutcome {
            status,
            pdf_path,
            exported_pdf_path: exported,
            synctex_path,
            log_path: Some(log_path),
            diagnostics,
            log,
            tool_output,
            duration: started.elapsed(),
            missing_files: missing,
            path_mapper: mapper,
        }
    }

    fn bibliography_diagnostics(
        &self,
        rules: &[BibliographyRule],
        summary: &LatexmkOutputSummary,
        root_url: &str,
        root_text: Option<&str>,
        map_path: &dyn Fn(&str) -> Option<String>,
        snapshot: &Snapshot,
    ) -> Vec<LogDiagnostic> {
        let mut result = Vec::new();
        let map_bib = |name: &str| -> Option<String> {
            if let Some(p) = map_path(name) {
                return Some(p);
            }
            // bibtex/biber find databases through BIBINPUTS too: fall back to a unique file-name match.
            let file = key(last_component(name));
            let mut matches = snapshot.project_files.iter().filter(|(k, _)| last_component(k) == file);
            match (matches.next(), matches.next()) {
                (Some((_, (_, abs))), None) => Some(abs.clone()),
                _ => None,
            }
        };
        let mut failed: Vec<Source> = Vec::new();
        for d in &summary.diagnostics {
            if d.severity == Severity::Error && matches!(d.source, Source::Biber | Source::Bibtex) && !failed.contains(&d.source) {
                failed.push(d.source);
            }
        }
        let mut parsed: Vec<Source> = Vec::new();
        for rule in rules {
            let blg = format!("{}.blg", rule.base);
            if let Ok(data) = fs::read(&blg) {
                let text = String::from_utf8_lossy(&data);
                result.extend(log_parser::parse_bibliography_log(&text, rule.tool, map_bib));
                if !parsed.contains(&rule.tool) {
                    parsed.push(rule.tool);
                }
            } else if rule.last_exit_code.is_some_and(|c| c != 0) && !failed.contains(&rule.tool) {
                // The tool ran (in this or an earlier run) and could not even write its log; latexmk does
                // not retry it until its inputs change, so this keeps the explanation visible later.
                failed.push(rule.tool);
            }
        }
        failed.retain(|t| !parsed.contains(t));
        let biblatex_line = root_text.and_then(biblatex_line);
        failed.sort_by_key(|t| if *t == Source::Biber { 0 } else { 1 }); // "biber" < "bibtex"
        for tool in failed {
            let from_summary = summary.diagnostics.iter().find(|d| d.source == tool && d.severity == Severity::Error);
            let mut detail: Vec<String> = Vec::new();
            if tool == Source::Biber {
                if let Some(problem) = self.distribution.biber_problem() {
                    detail.push(problem);
                }
            }
            if let Some(d) = from_summary.and_then(|d| d.detail.clone()).filter(|d| !d.is_empty()) {
                detail.push(d);
            }
            let message = if tool == Source::Biber {
                log_parser::BIBER_FAILURE_MESSAGE.to_string()
            } else {
                from_summary
                    .map(|d| d.message.clone())
                    .unwrap_or_else(|| "BibTeX failed to run; the bibliography was not generated.".to_string())
            };
            let mut d = LogDiagnostic::new(Severity::Error, tool, message);
            d.detail = (!detail.is_empty()).then(|| detail.join("\n"));
            d.path = Some(root_url.to_string());
            d.line = biblatex_line;
            result.push(d);
        }
        result
    }
}

/// Fallback when latexmk is missing (not in the Swift app, which reports a tool error): runs the engine
/// directly, bibtex when the .aux requests a bibliography, and reruns while TeX asks for it (max 4 passes).
#[allow(clippy::too_many_arguments)]
fn run_engine_directly(
    dist: &Distribution,
    engine_exe: &str,
    request: &CompileRequest,
    env: &[(String, String)],
    cwd: &str,
    out_directory: &str,
    jobname: &str,
    root_name: &str,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<Run, String> {
    let started = Instant::now();
    let mut args: Vec<String> = vec![
        "-interaction=nonstopmode".into(),
        "-file-line-error".into(),
        "-synctex=1".into(),
        format!("-output-directory={out_directory}"),
    ];
    if request.shell_escape {
        args.push("-shell-escape".into());
    }
    args.extend(request.extra_arguments.iter().cloned());
    args.push(root_name.to_string());
    let mut text = String::new();
    let mut last = Run { text: String::new(), exit_code: Some(0), timed_out: false, cancelled: false };
    let mut ran_bibtex = false;
    for pass in 1..=4 {
        let left = timeout.saturating_sub(started.elapsed());
        let opts = RunOptions { env: env.to_vec(), cwd: Some(cwd), timeout: Some(left), cancel: Some(cancelled), ..Default::default() };
        let out = distribution::run(engine_exe, &args, &opts)?;
        text.push_str(&out.combined_text());
        last.exit_code = out.exit_code;
        last.timed_out = out.timed_out;
        last.cancelled = out.cancelled;
        if out.timed_out || out.cancelled {
            break;
        }
        let aux = fs::read_to_string(join(out_directory, &format!("{jobname}.aux"))).unwrap_or_default();
        let mut rerun = false;
        if pass == 1 && !ran_bibtex && aux.contains("\\bibdata") {
            if let Some(bibtex) = dist.tool(Tool::Bibtex) {
                ran_bibtex = true;
                let o = RunOptions { env: env.to_vec(), cwd: Some(cwd), timeout: Some(left), cancel: Some(cancelled), ..Default::default() };
                let b = distribution::run(bibtex, &[join(out_directory, jobname)], &o)?;
                text.push_str(&b.combined_text());
                rerun = true;
            }
        }
        let log = fs::read_to_string(join(out_directory, &format!("{jobname}.log"))).unwrap_or_default();
        if log.contains("Rerun to get") || log.contains("Label(s) may have changed") || log.contains("Please rerun") {
            rerun = true;
        }
        if !rerun {
            break;
        }
    }
    last.text = text;
    Ok(last)
}

// ---------------------------------------------------------------------------------------------------
// latexmk helpers (pure, unit-tested)
// ---------------------------------------------------------------------------------------------------

/// `-e '$bibtex_fudge = 0'`: latexmk's default runs bibtex *inside* the out directory, where
/// `\bibliography{../refs}` can never be found. Without the fudge bibtex runs from the compile folder like
/// TeX itself; it writes to out/ (see TEXMFOUTPUT).
pub fn latexmk_arguments(
    engine: Engine,
    output_directory: &str,
    shell_escape: bool,
    extra: &[String],
    root_file_name: &str,
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        engine.latexmk_flag().into(),
        "-interaction=nonstopmode".into(),
        "-file-line-error".into(),
        "-synctex=1".into(),
        "-f".into(),
        "-e".into(),
        "$bibtex_fudge = 0;".into(),
        format!("-outdir={output_directory}"),
    ];
    if shell_escape {
        a.push("-shell-escape".into());
    }
    a.extend(extra.iter().cloned());
    a.push(root_file_name.to_string());
    a
}

/// A `["bibtex <base>"]` / `["biber <base>"]` rule of latexmk's `.fdb_latexmk` and its last exit status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibliographyRule {
    /// `Source::Bibtex` or `Source::Biber`.
    pub tool: Source,
    /// Absolute path without extension.
    pub base: String,
    pub last_exit_code: Option<i32>,
}

pub fn bibliography_rules(text: &str) -> Vec<BibliographyRule> {
    let mut rules = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if !(line.starts_with("[\"bibtex ") || line.starts_with("[\"biber ")) {
            continue;
        }
        let Some(close) = line.find("\"]") else { continue };
        let name = &line[2..close];
        let Some(space) = name.find(' ') else { continue };
        let tool = if &name[..space] == "biber" { Source::Biber } else { Source::Bibtex };
        let base = name[space + 1..].to_string();
        let last_token = line.split(' ').rfind(|s| !s.is_empty());
        rules.push(BibliographyRule { tool, base, last_exit_code: last_token.and_then(|t| t.parse().ok()) });
    }
    rules
}

/// 1-based line of `\usepackage[...]{biblatex}` (or the first `\addbibresource`/`\bibliography`).
pub fn biblatex_line(text: &str) -> Option<u32> {
    let mut fallback = None;
    for (i, line) in text.split('\n').enumerate() {
        let number = i as u32 + 1;
        let code = line.split('%').next().unwrap_or("");
        if code.contains("\\usepackage") && code.contains("biblatex") {
            return Some(number);
        }
        if fallback.is_none() && (code.contains("\\addbibresource") || code.contains("\\bibliography{")) {
            fallback = Some(number);
        }
    }
    fallback
}

/// The PDF latexmk reports ("Log file says output to '...pdf'"), else `default`; `None` if it does not exist.
pub fn pdf_location(tool_output: &str, default: &str) -> Option<String> {
    let marker = "Latexmk: Log file says output to '";
    if let Some(i) = tool_output.rfind(marker) {
        let rest = &tool_output[i + marker.len()..];
        if let Some(end) = rest.find('\'') {
            let path = &rest[..end];
            if path.ends_with(".pdf") && Path::new(path).exists() {
                return Some(lexically_standardized(path));
            }
        }
    }
    Path::new(default).exists().then(|| default.to_string())
}

/// File contents as text: UTF-8, else ISO Latin-1 (legacy `inputenc{latin1}` documents).
pub fn read_text(path: &str) -> Option<String> {
    let data = fs::read(path).ok()?;
    Some(match String::from_utf8(data) {
        Ok(s) => s,
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    })
}

/// Last `count` non-empty lines, without separator-only lines ("-----").
pub fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty())
        .filter(|l| !l.chars().all(|c| c == ' ' || c == '-'))
        .collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

/// Copies to a temporary file in the destination directory, then renames it over the destination.
pub fn copy_atomically(source: &str, destination: &str) -> io::Result<()> {
    let temporary = format!(
        "{}/.{}.gabetex-{}",
        parent_of(destination),
        last_component(destination),
        unique_suffix()
    );
    fs::copy(source, &temporary)?;
    if let Err(e) = fs::rename(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(e);
    }
    Ok(())
}

/// Explains inputs like `\input{../../../../x}` that TeX could not find although they exist relative to the
/// real root folder: they are beyond the folders mirrored by the shadow workspace.
pub fn unreachable_input_diagnostics(
    diagnostics: &[LogDiagnostic],
    root_directory: &str,
    root_url: &str,
) -> Vec<LogDiagnostic> {
    let mut result = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for d in diagnostics {
        let Some(name) = d.missing_file.as_deref() else { continue };
        if !(name.starts_with("../") || name.contains("/../")) {
            continue;
        }
        let base = lexically_standardized(&format!("{root_directory}/{name}"));
        let mut candidates = vec![base.clone()];
        if path_extension(&base).is_empty() {
            for ext in ["tex", "bib", "pdf", "png", "jpg", "eps"] {
                candidates.push(format!("{base}.{ext}"));
            }
        }
        let Some(found) = candidates.into_iter().find(|c| Path::new(c).is_file()) else { continue };
        if !seen.insert(found.clone()) {
            continue;
        }
        let mut diag = LogDiagnostic::new(
            Severity::Error,
            Source::Latexmk,
            format!(
                "{found} exists, but the background compiler cannot reach it: it only mirrors the project folder and the {ANCESTOR_LEVELS} folders above it. Open a folder that contains this file as the project."
            ),
        );
        diag.path = Some(d.path.clone().unwrap_or_else(|| root_url.to_string()));
        diag.line = d.line;
        result.push(diag);
    }
    result
}

// ---------------------------------------------------------------------------------------------------
// ChkTeX (ChkTeX.swift)
// ---------------------------------------------------------------------------------------------------

/// Warnings that are mostly noise for a mathematician's documents (see the Swift file for the reasons).
pub const CHKTEX_DEFAULT_SUPPRESSIONS: [u32; 19] =
    [1, 3, 8, 9, 10, 12, 13, 15, 17, 24, 30, 36, 37, 38, 39, 40, 41, 44, 46];

/// `chktex -q -I0 -v0 -f '%l:%c:%d:%k:%n:%m<newline>' -n<k>...` (reads stdin). The format must end with a
/// real newline character: chktex does not interpret a backslash-n escape.
pub fn chktex_arguments(suppressing: &[u32]) -> Vec<String> {
    let mut sorted: Vec<u32> = suppressing.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut a: Vec<String> = ["-q", "-I0", "-v0", "-f", "%l:%c:%d:%k:%n:%m\n"].iter().map(|s| s.to_string()).collect();
    a.extend(sorted.iter().map(|n| format!("-n{n}")));
    a
}

/// Runs chktex on `text` via stdin. Returns `[]` when chktex is missing or fails (linting is best effort).
pub fn chktex_lint(dist: &Distribution, text: &str, file_path: &str, suppressed: &[u32]) -> Vec<LogDiagnostic> {
    let Some(chktex) = dist.tool(Tool::Chktex) else { return Vec::new() };
    let opts = RunOptions {
        env: dist.process_environment(),
        stdin: Some(text.as_bytes().to_vec()),
        timeout: Some(Duration::from_secs(20)),
        ..Default::default()
    };
    match distribution::run(chktex, &chktex_arguments(suppressed), &opts) {
        Ok(o) if !o.cancelled && !o.timed_out => chktex_parse(&o.stdout_text(), text, file_path, suppressed),
        _ => Vec::new(),
    }
}

/// Parses `line:column:length:kind:number:message` lines. chktex counts columns and lengths in bytes (it is
/// not UTF-8 aware); they are converted to UTF-16 units using `text`.
pub fn chktex_parse(output: &str, text: &str, file_path: &str, suppressed: &[u32]) -> Vec<LogDiagnostic> {
    let source_lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let path = lexically_standardized(file_path);
    let mut result = Vec::new();
    for raw in output.split('\n') {
        let raw = raw.trim_end_matches('\r');
        if raw.is_empty() {
            continue;
        }
        let fields: Vec<&str> = raw.splitn(6, ':').collect();
        if fields.len() != 6 {
            continue;
        }
        let (Ok(line), Ok(column), Ok(number)) = (fields[0].parse::<i64>(), fields[1].parse::<i64>(), fields[4].parse::<i64>())
        else {
            continue;
        };
        if suppressed.iter().any(|s| *s as i64 == number) {
            continue;
        }
        let length = fields[2].parse::<i64>().unwrap_or(1);
        let kind = trim_blank(fields[3]);
        let message = trim_blank(fields[5]);
        let severity = if kind.eq_ignore_ascii_case("error") { Severity::Warning } else { Severity::Info };
        let mut span = (column, length);
        if line >= 1 && (line as usize) <= source_lines.len() {
            span = utf16_span(column, length, source_lines[line as usize - 1]);
        }
        let mut d = LogDiagnostic::new(
            severity,
            Source::Chktex,
            if message.is_empty() { format!("chktex {number}") } else { message.to_string() },
        );
        d.detail = Some(format!("chktex {} {number}", kind.to_lowercase()));
        d.path = Some(path.clone());
        d.line = Some(line.max(0) as u32);
        d.column = Some(span.0.max(0) as u32);
        d.length = Some(span.1.max(1) as u32);
        result.push(d);
    }
    result
}

fn trim_blank(s: &str) -> &str {
    s.trim_matches(|c| c == ' ' || c == '\t')
}

/// Converts a 1-based byte column + byte length on `line` to a 1-based UTF-16 column + length.
pub fn utf16_span(byte_column: i64, byte_length: i64, line: &str) -> (i64, i64) {
    // UTF-16 units of the scalars that start before `byte_offset`.
    let utf16_offset = |byte_offset: i64| -> i64 {
        let (mut bytes, mut units) = (0i64, 0i64);
        for c in line.chars() {
            if bytes >= byte_offset {
                break;
            }
            bytes += c.len_utf8() as i64;
            units += c.len_utf16() as i64;
        }
        units
    };
    let start = utf16_offset(byte_column - 1);
    let end = utf16_offset(byte_column - 1 + byte_length.max(0));
    (start + 1, (end - start).max(1))
}

// ---------------------------------------------------------------------------------------------------
// SectionPageLocator (pure part)
// ---------------------------------------------------------------------------------------------------

/// Finds the PDF page of every section heading with SyncTeX forward search. The PDF page count, the
/// forward lookup and the page-top test are supplied by the caller (the SyncTeX module and a PDF reader own
/// them); this keeps the Swift algorithm - which of the answered pages is the heading's - in one place.
pub mod section_page {
    use std::collections::HashMap;
    use std::path::Path;

    /// More headings than this and the lookup is skipped (a thesis has a few hundred).
    pub const HEADING_LIMIT: usize = 1500;

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct Heading {
        /// Echoed in the result map.
        pub key: String,
        pub file: String,
        /// 1-based line the heading command starts on.
        pub line: u32,
        /// 1-based line its title ends on (SyncTeX records the line a command finished on).
        pub end_line: u32,
    }

    impl Heading {
        pub fn new(key: &str, file: &str, line: u32, end_line: u32) -> Self {
            Heading { key: key.to_string(), file: file.to_string(), line, end_line: end_line.max(line) }
        }
    }

    /// One box of a forward-search answer.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct PageBox {
        pub page: u32,
        /// Top edge (PDF points, origin bottom-left).
        pub max_y: f64,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Placement {
        pub page: u32,
        /// The heading's boxes on `page` start in the top part of the page.
        pub is_at_top: bool,
    }

    /// Whether the compile left SyncTeX data next to the PDF.
    pub fn has_synctex_data(pdf: &str) -> bool {
        let stem = pdf.strip_suffix(".pdf").unwrap_or(pdf);
        Path::new(&format!("{stem}.synctex.gz")).exists() || Path::new(&format!("{stem}.synctex")).exists()
    }

    /// The heading is on the **last** page SyncTeX answered (earlier pages are the running header and
    /// footer shipped out while TeX read the heading line); `top` is the highest box edge on that page,
    /// `at_top` the page-top test (`PageAttribution.isAtPageTop`).
    pub fn placement(boxes: &[PageBox], total_pages: u32, at_top: impl Fn(u32, f64) -> bool) -> Option<Placement> {
        let page = boxes.iter().map(|b| b.page).max()?;
        if page < 1 || page > total_pages {
            return None;
        }
        let top = boxes.iter().filter(|b| b.page == page).map(|b| b.max_y).fold(f64::NEG_INFINITY, f64::max);
        Some(Placement { page, is_at_top: at_top(page, top) })
    }

    /// Looks every heading up with `forward(file, line)`: its last line first, then the line it started on.
    /// `None` when there are too many headings or no SyncTeX data. The front matter (key
    /// `front_matter_key`) is always on page 1.
    pub fn locate(
        headings: &[Heading],
        pdf: &str,
        total_pages: u32,
        front_matter_key: &str,
        forward: impl Fn(&str, u32) -> Vec<PageBox> + Sync,
        at_top: impl Fn(u32, f64) -> bool + Sync,
    ) -> Option<HashMap<String, Placement>> {
        if headings.len() > HEADING_LIMIT || total_pages == 0 || !has_synctex_data(pdf) {
            return None;
        }
        let mut placements: HashMap<String, Placement> = HashMap::new();
        placements.insert(front_matter_key.to_string(), Placement { page: 1, is_at_top: true });
        let next = std::sync::atomic::AtomicUsize::new(0);
        let results = std::sync::Mutex::new(Vec::<(usize, Option<Placement>)>::new());
        // SyncTeX answers one question per process: at most four at once.
        std::thread::scope(|scope| {
            for _ in 0..4.min(headings.len().max(1)) {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let Some(h) = headings.get(i) else { break };
                    let mut lines = vec![h.end_line];
                    if h.line != h.end_line {
                        lines.push(h.line);
                    }
                    let mut found = Vec::new();
                    for line in lines {
                        found = forward(&h.file, line);
                        if !found.is_empty() {
                            break;
                        }
                    }
                    let p = placement(&found, total_pages, &at_top);
                    results.lock().unwrap_or_else(|e| e.into_inner()).push((i, p));
                });
            }
        });
        for (i, p) in results.into_inner().unwrap_or_else(|e| e.into_inner()) {
            if let Some(p) = p {
                placements.insert(headings[i].key.clone(), p);
            }
        }
        Some(placements)
    }
}

// ---------------------------------------------------------------------------------------------------
// Tests (ports of TeXToolchainChecks UnitChecks / ShadowSafetyChecks that need no TeX, plus a compile
// pipeline test against a fake latexmk on Unix)
// ---------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> String {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-scratch"))
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        lexically_standardized(&dir.to_string_lossy())
    }

    fn write(path: &str, text: &str) {
        fs::create_dir_all(parent_of(path)).unwrap();
        fs::write(path, text).unwrap();
    }

    #[allow(dead_code)]
    fn no_overlays() -> HashMap<String, String> {
        HashMap::new()
    }

    #[test]
    fn engine_parsing() {
        assert_eq!(Engine::from_magic_program("XeLaTeX"), Some(Engine::Xelatex));
        assert_eq!(Engine::from_magic_program("latexmk -lualatex"), Some(Engine::Lualatex));
        assert_eq!(Engine::from_magic_program("pdflatex"), Some(Engine::Pdflatex));
        assert_eq!(Engine::from_magic_program("latex"), Some(Engine::Pdflatex));
        assert_eq!(Engine::from_magic_program("context"), None);
        assert_eq!(Engine::parse("xelatex"), Some(Engine::Xelatex));
        assert_eq!(Engine::parse("nope"), None);
        assert_eq!(Engine::Lualatex.latexmk_flag(), "-lualatex");
    }

    #[test]
    fn latexmk_fdb_rules_package_files_arguments() {
        let fdb = "# Fdb version 4\n[\"biber /o/main\"] 1793221244.29773 \"/o/main.bcf\" \"/o/main.bbl\" \"/o/main\" 1793221248.66568 2\n  \"/o/main.bcf\" 1 2 3 \"pdflatex\"\n[\"pdflatex\"] 1793221247.92909 \"main.tex\" \"/o/main.pdf\" \"main\" 1793221248.66597 0\n";
        assert_eq!(
            bibliography_rules(fdb),
            vec![BibliographyRule { tool: Source::Biber, base: "/o/main".into(), last_exit_code: Some(2) }]
        );
        let spaces = "[\"bibtex C:/Users/John Doe/out/main\"] 1.0 \"x\" 0\r\n";
        assert_eq!(
            bibliography_rules(spaces),
            vec![BibliographyRule { tool: Source::Bibtex, base: "C:/Users/John Doe/out/main".into(), last_exit_code: Some(0) }]
        );
        assert!(log_parser::is_package_file("tikz-cd.sty") && log_parser::is_package_file("ecrm1000.tfm"));
        assert!(!log_parser::is_package_file("figs/plot"));
        assert!(!log_parser::is_package_file("chapters/nothere.tex"));
        assert!(!log_parser::is_package_file("nothere.tex"));
        let args = latexmk_arguments(Engine::Xelatex, "/s/out/thesis", true, &["-silent".to_string()], "main file.tex");
        assert_eq!(
            args,
            [
                "-xelatex",
                "-interaction=nonstopmode",
                "-file-line-error",
                "-synctex=1",
                "-f",
                "-e",
                "$bibtex_fudge = 0;",
                "-outdir=/s/out/thesis",
                "-shell-escape",
                "-silent",
                "main file.tex"
            ]
        );
        assert_eq!(biblatex_line("\\documentclass{article}\n% \\usepackage{biblatex}\n\\usepackage[backend=biber]{biblatex}\n"), Some(3));
        assert_eq!(biblatex_line("a\n\\addbibresource{x.bib}\n"), Some(2));
        assert_eq!(biblatex_line("a\n"), None);
    }

    #[test]
    fn missing_file_lines() {
        use log_parser::missing_file_line as f;
        assert_eq!(f("\\documentclass{article}\n\\usepackage{amsmath}\n\\usepackage{thispackagedoesnotexist}\n", "thispackagedoesnotexist.sty"), Some(3));
        assert_eq!(f("a\n\\input{chapters/nothere}\n", "chapters/nothere.tex"), Some(2));
        assert_eq!(f("\\usepackage{amsmath}\n", "amsmath-other.sty"), None);
    }

    #[test]
    fn helpers() {
        assert_eq!(last_lines("a\n\n-----\nb\r\nc\n", 2), "b\nc");
        assert_eq!(path_extension("a/b.tar.gz"), "gz");
        assert_eq!(path_extension(".bashrc"), "");
        assert_eq!(delete_extension("main.tex"), "main");
        assert_eq!(delete_extension("a.b.tex"), "a.b");
        assert_eq!(delete_extension("Makefile"), "Makefile");
        assert_eq!(parent_of("/a/b"), "/a");
        assert_eq!(parent_of("/a"), "/");
        assert_eq!(parent_of("a"), "");
        assert_eq!(encode("ação", Some("latin-1")), vec![b'a', 0xE7, 0xE3, b'o']);
        assert_eq!(encode("5 €", Some("latin-1")), "5 €".as_bytes().to_vec(), "not representable -> UTF-8");
        assert_eq!(encode("ação", None), "ação".as_bytes().to_vec());
        let dir = scratch("pdfloc");
        let pdf = format!("{dir}/x.pdf");
        write(&pdf, "%PDF");
        assert_eq!(pdf_location(&format!("noise\nLatexmk: Log file says output to '{pdf}'\n"), "/nonexistent/a.pdf"), Some(pdf.clone()));
        assert_eq!(pdf_location("nothing", &pdf), Some(pdf.clone()));
        assert_eq!(pdf_location("nothing", "/nonexistent/a.pdf"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn excluded_names_and_layout() {
        for n in [".git", ".svn", ".hg", ".DS_Store", ".#main.tex", "main.tex~", "#main.tex#", ".main.tex.swp", ".x.swo"] {
            assert!(ShadowWorkspace::is_excluded(n), "{n}");
        }
        for n in [".latexmkrc", ".hidden.tex", "main.tex", "#", "##", ".swp", "a.swap"] {
            assert!(!ShadowWorkspace::is_excluded(n), "{n}");
        }
        let top = ShadowWorkspace::layout_for_project_path("/Users/g/Math/papers/p1");
        assert_eq!(top, ("/Users/g".to_string(), vec!["Math".into(), "papers".into(), "p1".into()]));
        let shallow = ShadowWorkspace::layout_for_project_path("/Proj");
        assert_eq!(shallow, ("/".to_string(), vec!["Proj".into()]));
        let win = ShadowWorkspace::layout_for_project_path("C:/Users/g/Math/p1");
        assert_eq!(win, ("C:/Users".to_string(), vec!["g".into(), "Math".into(), "p1".into()]));
        let win_shallow = ShadowWorkspace::layout_for_project_path("D:/p");
        assert_eq!(win_shallow, ("D:/".to_string(), vec!["p".into()]));
        let id = ShadowWorkspace::identifier("/Users/g/Proj");
        assert_eq!(id.len(), 16);
        assert_eq!(id, ShadowWorkspace::identifier("/Users/g/Proj/"), "stable per project path");
        assert_ne!(id, ShadowWorkspace::identifier("/Users/g/Proj2"));
    }

    #[test]
    fn path_mapper_with_ancestor_layer() {
        let ws = ShadowWorkspace::new("/x/papers/paper1", "/s/shadow");
        let mapper = ws.path_mapper();
        let src = ws.source_directory();
        assert_eq!(src, "/s/shadow/tree/x/papers/paper1");
        assert_eq!(mapper.project_path(&format!("{src}/../macros.tex")), Some("/x/papers/macros.tex".to_string()));
        assert_eq!(mapper.project_path(&format!("{src}/./main.tex")), Some("/x/papers/paper1/main.tex".to_string()));
        assert_eq!(mapper.project_path(&format!("{}/main.aux", ws.output_directory())), None, "out/ is not mapped");
        assert_eq!(mapper.shadow_path("/x/common/defs.tex"), "/s/shadow/tree/x/common/defs.tex");
    }

    #[test]
    fn chktex_arguments_and_parsing() {
        let args = chktex_arguments(&[2, 1]);
        assert_eq!(&args[..5], ["-q", "-I0", "-v0", "-f", "%l:%c:%d:%k:%n:%m\n"]);
        assert_eq!(&args[5..], ["-n1", "-n2"]);
        // Real chktex 1.7 output for "Olá ação $x^10$ fim." - columns/lengths are bytes.
        let text = "Olá ação $x^10$ fim.\nSee section \\ref{a}\n";
        let out = "1:16:2:Warning:25:You might wish to put this between a pair of `{}'\n2:12:1:Warning:2:Non-breaking space (`~') should have been used.\n3:1:4:Error:9:`itemize' expected, found `enumerate'.\n";
        let ds = chktex_parse(out, text, "/p/a.tex", &[9]);
        assert_eq!(ds.len(), 2);
        assert_eq!(ds[0].line, Some(1));
        assert_eq!(ds[0].column, Some(13), "byte column 16 -> UTF-16 column 13 (a-acute, c-cedilla, a-tilde are 2 bytes)");
        assert_eq!(ds[0].length, Some(2));
        assert_eq!(ds[0].severity, Severity::Info, "chktex Warning -> info");
        assert_eq!(ds[0].source, Source::Chktex);
        assert_eq!(ds[1].message, "Non-breaking space (`~') should have been used.");
        assert_eq!(ds[1].column, Some(12));
        let err = chktex_parse("1:1:1:Error:99:Message with: colons\r\n", "x\n", "/p/a.tex", &[]);
        assert_eq!(err[0].severity, Severity::Warning, "chktex Error -> warning");
        assert_eq!(err[0].message, "Message with: colons");
        assert_eq!(chktex_parse("garbage\n1:2:3\n", "x", "/p/a.tex", &[]).len(), 0);
        assert_eq!(utf16_span(2, 4, "a😀b"), (2, 2), "4-byte scalar = 2 UTF-16 units");
        assert_eq!(utf16_span(6, 1, "a😀b").0, 4, "after a 4-byte emoji (2 UTF-16 units)");
        assert!(CHKTEX_DEFAULT_SUPPRESSIONS.contains(&9) && !CHKTEX_DEFAULT_SUPPRESSIONS.contains(&2));
        // Without chktex installed, linting is a no-op.
        let d = Distribution {
            name: "x".into(),
            year: None,
            kind: distribution::Kind::TeXLive,
            bin_dir: "/nonexistent".into(),
            texmf_root: None,
            user_writable: false,
            tools: Default::default(),
            version_description: None,
        };
        assert!(chktex_lint(&d, "x", "/p/a.tex", &[]).is_empty());
    }

    #[test]
    fn section_page_algorithm() {
        use section_page::*;
        let boxes = [PageBox { page: 3, max_y: 700.0 }, PageBox { page: 4, max_y: 100.0 }, PageBox { page: 4, max_y: 780.0 }];
        // The heading is on the last answered page; its top is the highest box edge there.
        let p = placement(&boxes, 10, |page, top| page == 4 && top > 750.0).unwrap();
        assert_eq!(p, Placement { page: 4, is_at_top: true });
        assert_eq!(placement(&[], 10, |_, _| true), None);
        assert_eq!(placement(&[PageBox { page: 11, max_y: 1.0 }], 10, |_, _| true), None);
        let h = Heading::new("k", "/p/a.tex", 5, 3);
        assert_eq!(h.end_line, 5, "end line is never before the start line");
        assert!(!has_synctex_data("/nonexistent/x.pdf"));
        let dir = scratch("section");
        let pdf = format!("{dir}/main.pdf");
        write(&pdf, "%PDF");
        assert!(!has_synctex_data(&pdf));
        write(&format!("{dir}/main.synctex.gz"), "x");
        assert!(has_synctex_data(&pdf));
        let headings = vec![Heading::new("a", "/p/a.tex", 3, 3), Heading::new("b", "/p/a.tex", 10, 11), Heading::new("ghost", "/p/g.tex", 1, 1)];
        let result = locate(
            &headings,
            &pdf,
            9,
            "front",
            |file, line| match (file, line) {
                ("/p/a.tex", 3) => vec![PageBox { page: 1, max_y: 10.0 }, PageBox { page: 2, max_y: 800.0 }],
                ("/p/a.tex", 10) => vec![PageBox { page: 5, max_y: 400.0 }], // endLine 11 unknown, falls back to 10
                _ => vec![],
            },
            |_, top| top > 700.0,
        )
        .unwrap();
        assert_eq!(result["front"], Placement { page: 1, is_at_top: true });
        assert_eq!(result["a"], Placement { page: 2, is_at_top: true });
        assert_eq!(result["b"], Placement { page: 5, is_at_top: false });
        assert!(!result.contains_key("ghost"));
        let many: Vec<Heading> = (0..=HEADING_LIMIT).map(|i| Heading::new(&i.to_string(), "/p/a.tex", 1, 1)).collect();
        assert!(locate(&many, &pdf, 9, "front", |_, _| vec![], |_, _| false).is_none(), "too many headings");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreachable_inputs() {
        let base = scratch("unreach");
        write(&format!("{base}/shared/macros.tex"), "x");
        write(&format!("{base}/a/b/c/d/proj/main.tex"), "x");
        let mut d = LogDiagnostic::new(Severity::Error, Source::Tex, "File not found");
        d.missing_file = Some("../../../../../shared/macros".into());
        d.path = Some(format!("{base}/a/b/c/d/proj/main.tex"));
        d.line = Some(4);
        let out = unreachable_input_diagnostics(&[d.clone(), d], &format!("{base}/a/b/c/d/proj"), "/root.tex");
        assert_eq!(out.len(), 1, "deduplicated");
        assert!(out[0].message.contains("macros.tex exists, but the background compiler cannot reach it"));
        assert_eq!(out[0].line, Some(4));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn write_atomically_refuses_to_escape() {
        let base = scratch("guard");
        let src = format!("{base}/src");
        let outside = format!("{base}/outside");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&outside).unwrap();
        // ../ escape
        let r = write_atomically(b"x", &format!("{src}/../outside/y.tex"), &src);
        assert!(matches!(&r, Err(Failure::Io(m)) if m.contains("refusing") || m.contains("cannot resolve")), "{r:?}");
        let r = write_atomically(b"x", &format!("{src}/missing/deeper/z.tex"), &src);
        assert!(r.is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0, "nothing created outside");
        write_atomically(b"ok", &format!("{src}/ok.tex"), &src).unwrap();
        assert_eq!(fs::read_to_string(format!("{src}/ok.tex")).unwrap(), "ok");
        write_atomically(b"over", &format!("{src}/ok.tex"), &src).unwrap();
        assert_eq!(fs::read_to_string(format!("{src}/ok.tex")).unwrap(), "over");
        let leftovers = fs::read_dir(&src).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).count();
        assert_eq!(leftovers, 0, "no temporary files left behind");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn lock_waiters_can_give_up() {
        let lock = Arc::new(WorkspaceLock::default());
        let never = || false;
        let guard = lock.acquire(&never).unwrap();
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (l2, f2) = (lock.clone(), flag.clone());
        let waiter = std::thread::spawn(move || {
            let cancelled = || f2.load(Ordering::SeqCst);
            l2.acquire(&cancelled).is_some()
        });
        std::thread::sleep(Duration::from_millis(100));
        flag.store(true, Ordering::SeqCst);
        assert!(!waiter.join().unwrap(), "a cancelled waiter returns without the lock");
        drop(guard);
        assert!(lock.acquire(&never).is_some(), "the lock is free again");
        // A waiter that is not cancelled gets the lock once released.
        let g = lock.acquire(&never).unwrap();
        let l3 = lock.clone();
        let waiter = std::thread::spawn(move || l3.acquire(&|| false).is_some());
        std::thread::sleep(Duration::from_millis(60));
        drop(g);
        assert!(waiter.join().unwrap());
    }

    // -- shadow workspace on a real file system (links need symlink support) ----------------------------

    #[cfg(unix)]
    mod fs_tests {
        use super::*;

        fn symlink(target: &str, path: &str) {
            std::os::unix::fs::symlink(target, path).unwrap();
        }

        /// Path, type, mode, mtime, contents of everything below `dir`, not following links.
        fn fingerprint(dir: &str) -> Vec<String> {
            use std::os::unix::fs::MetadataExt;
            let mut lines = Vec::new();
            walk(dir, "", &|_| false, &mut |rel, ty| {
                let p = format!("{dir}/{rel}");
                let m = fs::symlink_metadata(&p).unwrap();
                let content = if ty == EntryType::Regular { fs::read(&p).unwrap() } else { vec![] };
                lines.push(format!("{rel}|{ty:?}|{}|{:o}|{}.{}|{content:?}", m.ino(), m.mode(), m.mtime(), m.mtime_nsec()));
                true
            });
            lines
        }

        #[test]
        fn mirror_overlays_and_stale_entries() {
            let base = scratch("mirror");
            let project = format!("{base}/proj");
            write(&format!("{project}/main.tex"), "main\n");
            write(&format!("{project}/sections/intro.tex"), "intro\n");
            write(&format!("{project}/.hidden.tex"), "h\n");
            write(&format!("{project}/.latexmkrc"), "rc\n");
            for skipped in [".git/HEAD", ".DS_Store", "main.tex~", ".main.tex.swp"] {
                write(&format!("{project}/{skipped}"), "x");
            }
            let ws = ShadowWorkspace::new(&project, &format!("{base}/shadow"));
            let src = ws.source_directory();
            let overlays = HashMap::from([(format!("{project}/sections/intro.tex"), "UNSAVED\n".to_string())]);
            let snap = ws.refresh(&overlays, &no_overlays(), &[]).unwrap();
            assert_eq!(read_link(&format!("{src}/main.tex")), Some(format!("{project}/main.tex")), "symlink to the original");
            assert_eq!(read_link(&format!("{src}/sections/intro.tex")), None, "overlay is a real file");
            assert_eq!(fs::read_to_string(format!("{src}/sections/intro.tex")).unwrap(), "UNSAVED\n");
            assert_eq!(fs::read_to_string(format!("{project}/sections/intro.tex")).unwrap(), "intro\n", "project untouched");
            for skipped in [".git", ".DS_Store", "main.tex~", ".main.tex.swp"] {
                assert!(fs::symlink_metadata(format!("{src}/{skipped}")).is_err(), "{skipped} is not mirrored");
            }
            assert_eq!(read_link(&format!("{src}/.latexmkrc")), Some(format!("{project}/.latexmkrc")), ".latexmkrc is mirrored");
            assert_eq!(read_link(&format!("{src}/.hidden.tex")), Some(format!("{project}/.hidden.tex")));
            let mut keys: Vec<&String> = snap.project_files.keys().collect();
            keys.sort();
            assert_eq!(keys, [".hidden.tex", ".latexmkrc", "main.tex", "sections/intro.tex"]);
            assert_eq!(snap.directories, ["sections"]);
            assert!(Path::new(&format!("{}/sections", ws.output_directory())).is_dir(), "out/ mirrors the directory tree");

            // Project changes: stale entries disappear, overlays go away, new files appear.
            fs::remove_file(format!("{project}/main.tex")).unwrap();
            fs::remove_file(format!("{project}/.hidden.tex")).unwrap();
            write(&format!("{project}/new.tex"), "new\n");
            ws.refresh(&no_overlays(), &no_overlays(), &[]).unwrap();
            assert_eq!(read_link(&format!("{src}/sections/intro.tex")), Some(format!("{project}/sections/intro.tex")));
            assert!(fs::symlink_metadata(format!("{src}/main.tex")).is_err(), "stale entry removed");
            assert!(fs::symlink_metadata(format!("{src}/.hidden.tex")).is_err(), "stale hidden link removed");
            assert_eq!(read_link(&format!("{src}/new.tex")), Some(format!("{project}/new.tex")));
            let _ = fs::remove_dir_all(&base);
        }

        #[test]
        fn overlays_below_a_symlinked_folder_never_reach_the_real_files() {
            let base = scratch("symlink-unit");
            let shared = format!("{base}/shared/chapters");
            let project = format!("{base}/proj");
            write(&format!("{shared}/one.tex"), "REAL ONE\n");
            write(&format!("{shared}/sub/two.tex"), "REAL TWO\n");
            write(&format!("{project}/main.tex"), "main\n");
            symlink("../shared/chapters", &format!("{project}/chapters"));
            let before = fingerprint(&format!("{base}/shared"));
            let ws = ShadowWorkspace::new(&project, &format!("{base}/shadow"));
            let src = ws.source_directory();
            let overlays = HashMap::from([
                (format!("{project}/chapters/one.tex"), "OVERLAY ONE\n".to_string()),
                (format!("{project}/chapters/sub/two.tex"), "OVERLAY TWO\n".to_string()),
                (format!("{project}/chapters/new/three.tex"), "NEW\n".to_string()),
            ]);
            let snap = ws.refresh(&overlays, &no_overlays(), &[]).unwrap();
            assert_eq!(read_link(&format!("{src}/chapters")), None, "the linked folder is materialized as a real directory");
            assert_eq!(read_link(&format!("{src}/chapters/sub")), None, "... recursively along the overlay's path");
            assert_eq!(fs::read_to_string(format!("{src}/chapters/one.tex")).unwrap(), "OVERLAY ONE\n");
            assert_eq!(read_link(&format!("{src}/chapters/one.tex")), None, "the overlay is a real file in the shadow");
            assert_eq!(fs::read_to_string(format!("{src}/chapters/sub/two.tex")).unwrap(), "OVERLAY TWO\n");
            assert_eq!(fs::read_to_string(format!("{src}/chapters/new/three.tex")).unwrap(), "NEW\n");
            assert!(snap.project_files.contains_key("chapters/one.tex") && snap.project_files.contains_key("chapters/sub/two.tex"));
            assert_eq!(fingerprint(&format!("{base}/shared")), before, "real files byte-identical and no stray files next to them");

            // Overlays gone: the folder is a single link again; nothing was ever written into shared/.
            ws.refresh(&no_overlays(), &no_overlays(), &[]).unwrap();
            assert_eq!(read_link(&format!("{src}/chapters")), Some(format!("{project}/chapters")), "back to one symlink");
            assert_eq!(fingerprint(&format!("{base}/shared")), before, "still untouched");

            // The root document below a symlinked folder: its directory is materialized too (pinned).
            let pinned = ws.refresh(&no_overlays(), &no_overlays(), &[format!("{project}/chapters/sub/two.tex")]).unwrap();
            assert_eq!(read_link(&format!("{src}/chapters/sub")), None, "cwd of a pinned root is a real shadow folder");
            assert!(pinned.project_files.contains_key("chapters/sub/two.tex"), "pinned root is a known project file");
            let _ = fs::remove_dir_all(&base);
        }

        #[test]
        fn write_atomically_replaces_links_and_refuses_link_escapes() {
            let base = scratch("guard-links");
            let src = format!("{base}/src");
            let outside = format!("{base}/outside");
            fs::create_dir_all(&src).unwrap();
            fs::create_dir_all(&outside).unwrap();
            symlink(&outside, &format!("{src}/escape"));
            for (path, label) in [
                (format!("{src}/escape/x.tex"), "through a symlinked folder"),
                (format!("{src}/escape/deeper/z.tex"), "below a missing folder behind a link"),
            ] {
                let r = write_atomically(b"x", &path, &src);
                assert!(matches!(&r, Err(Failure::Io(m)) if m.contains("refusing") || m.contains("cannot resolve")), "{label}: {r:?}");
            }
            assert_eq!(fs::read_dir(&outside).unwrap().count(), 0, "nothing created outside");
            symlink(&format!("{outside}/target.tex"), &format!("{src}/link.tex"));
            write_atomically(b"over", &format!("{src}/link.tex"), &src).unwrap();
            assert_eq!(read_link(&format!("{src}/link.tex")), None, "the symlink itself was replaced");
            assert!(!Path::new(&format!("{outside}/target.tex")).exists(), "its target was not created");
            let _ = fs::remove_dir_all(&base);
        }

        #[test]
        fn ancestor_layer_mirrors_the_folders_above_the_project() {
            let base = scratch("ancestor-unit");
            let project = format!("{base}/papers/paper1");
            write(&format!("{project}/main.tex"), "main\n");
            write(&format!("{base}/papers/macros.tex"), "macros\n");
            write(&format!("{base}/common/defs.tex"), "defs\n");
            write(&format!("{base}/papers/.git/HEAD"), "x\n");
            let ws = ShadowWorkspace::new(&project, &format!("{base}/shadow"));
            let (_, components) = ws.layout();
            assert_eq!(components.len(), ANCESTOR_LEVELS);
            assert_eq!(components.last().map(|s| s.as_str()), Some("paper1"));
            ws.refresh(&no_overlays(), &no_overlays(), &[]).unwrap();
            let src = ws.source_directory();
            let parent = parent_of(&src).to_string(); // mirror of base/papers
            let grandparent = parent_of(&parent).to_string(); // mirror of base
            assert_eq!(read_link(&format!("{parent}/macros.tex")), Some(format!("{base}/papers/macros.tex")), "sibling file linked");
            assert_eq!(read_link(&format!("{grandparent}/common")), Some(format!("{base}/common")), "sibling folder linked (2 levels up)");
            assert_eq!(read_link(&format!("{grandparent}/papers")), None, "folders on the way down are real");
            assert!(fs::symlink_metadata(format!("{parent}/.git")).is_err(), "VCS folders are not mirrored in the ancestor layer");
            assert_eq!(fs::read_to_string(format!("{src}/../../common/defs.tex")).unwrap(), "defs\n", "../../common/defs.tex resolves in the shadow");

            let mapper = ws.path_mapper();
            assert_eq!(mapper.project_path(&format!("{src}/../macros.tex")), Some(format!("{base}/papers/macros.tex")), "ancestor-layer path maps back");
            assert_eq!(mapper.project_path(&format!("{src}/./main.tex")), Some(format!("{project}/main.tex")));
            let pf = ProjectFiles::default();
            assert_eq!(
                log_parser::project_file_for_log_path("../macros.tex", &src, &mapper, &pf),
                Some(format!("{base}/papers/macros.tex")),
                "log paths with ../ map to the real file"
            );
            assert_eq!(log_parser::project_file_for_log_path("../nothere.tex", &src, &mapper, &pf), None);
            let _ = fs::remove_dir_all(&base);
        }

        #[test]
        fn overlays_are_written_in_their_encoding() {
            let base = scratch("encoding-unit");
            let project = format!("{base}/proj");
            let latin = format!("{project}/latin.tex");
            let euro = format!("{project}/euro.tex");
            let plain = format!("{project}/plain.tex");
            write(&latin, "x");
            let ws = ShadowWorkspace::new(&project, &format!("{base}/shadow"));
            let overlays = HashMap::from([
                (latin.clone(), "ação AÇÃO\n".to_string()),
                (euro.clone(), "5 €\n".to_string()),
                (plain.clone(), "ação\n".to_string()),
            ]);
            let encodings = HashMap::from([
                (latin.clone(), "latin-1".to_string()),
                (format!("{project}/../proj/euro.tex"), "latin-1".to_string()), // non-standardized key
            ]);
            ws.refresh(&overlays, &encodings, &[]).unwrap();
            let src = ws.source_directory();
            let expected: Vec<u8> = "ação AÇÃO\n".chars().map(|c| c as u32 as u8).collect();
            assert_eq!(fs::read(format!("{src}/latin.tex")).unwrap(), expected, "Latin-1 bytes");
            assert_eq!(fs::read(format!("{src}/euro.tex")).unwrap(), "5 €\n".as_bytes(), "the euro sign is not Latin-1: UTF-8 fallback");
            assert_eq!(fs::read(format!("{src}/plain.tex")).unwrap(), "ação\n".as_bytes(), "no encoding: UTF-8");
            assert_eq!(fs::read_to_string(&latin).unwrap(), "x", "project file untouched");
            let _ = fs::remove_dir_all(&base);
        }

        #[test]
        fn too_many_files_is_reported() {
            let base = scratch("toomany");
            let project = format!("{base}/proj");
            fs::create_dir_all(&project).unwrap();
            for i in 0..(MAXIMUM_ENTRIES + 1) {
                fs::write(format!("{project}/f{i}.tex"), "").unwrap();
            }
            let ws = ShadowWorkspace::new(&project, &format!("{base}/shadow"));
            assert_eq!(ws.refresh(&no_overlays(), &no_overlays(), &[]).unwrap_err(), Failure::TooManyFiles(MAXIMUM_ENTRIES));
            let _ = fs::remove_dir_all(&base);
        }

        // -- compile pipeline against fake TeX tools ---------------------------------------------------

        fn fake_distribution(base: &str, latexmk_body: &str) -> Distribution {
            use std::os::unix::fs::PermissionsExt;
            let bin = format!("{base}/bin");
            fs::create_dir_all(&bin).unwrap();
            let p = format!("{bin}/latexmk");
            fs::write(&p, format!("#!/bin/sh\n{latexmk_body}\n")).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
            Distribution {
                name: "Fake TeX".into(),
                year: None,
                kind: distribution::Kind::TeXLive,
                bin_dir: bin,
                texmf_root: None,
                user_writable: true,
                tools: [(Tool::Latexmk, p)].into_iter().collect(),
                version_description: None,
            }
        }

        /// A latexmk stand-in: finds -outdir and the root file, copies the root source into the log (so the
        /// test can see which text TeX "saw"), writes a PDF and a .synctex.gz, and reports an error.
        const FAKE_LATEXMK: &str = r#"
for a in "$@"; do case "$a" in -outdir=*) out="${a#-outdir=}";; *) root="$a";; esac; done
job="${root%.tex}"
mkdir -p "$out"
{
  echo "This is pdfTeX, Version 3.14159265-2.6-1.40.21 (TeX Live 2020)"
  echo "(./$root"
  echo "./$root:3: Undefined control sequence."
  echo "l.3 \\foo"
  echo "        "
  echo "? "
  echo "! Emergency stop."
  echo "SEEN-BY-TEX: $(cat "$root" | tr '\n' '|')"
  echo ")"
} > "$out/$job.log"
echo "%PDF-fake" > "$out/$job.pdf"
echo sync > "$out/$job.synctex.gz"
echo "Latexmk: Log file says output to '$out/$job.pdf'"
pwd
exit 12
"#;

        #[test]
        fn compile_pipeline_with_fake_latexmk() {
            let base = scratch("compile-fake");
            let project = format!("{base}/proj");
            write(&format!("{project}/main.tex"), "\\documentclass{article}\n\\begin{document}\n\\foo\n\\end{document}\n");
            let dist = fake_distribution(&base, FAKE_LATEXMK);
            let compiler = LaTeXCompiler::with_shadow_root(dist, &format!("{base}/shadow"));
            let mut req = CompileRequest::new(&project, &format!("{project}/main.tex"), Engine::Pdflatex, Purpose::Build);
            req.overlays.insert(format!("{project}/main.tex"), "UNSAVED\n\\foo\n\\foo\n".into());
            let out = compiler.compile(&req);
            assert_eq!(out.status, CompileStatus::Failed, "{:?}", out.tool_output);
            assert!(out.tool_output.contains("/tree/") && out.tool_output.contains("/proj"), "ran in the shadow tree: {}", out.tool_output);
            assert!(out.log.contains("SEEN-BY-TEX: UNSAVED|"), "TeX saw the unsaved buffer: {}", out.log);
            assert_eq!(fs::read_to_string(format!("{project}/main.tex")).unwrap().lines().next(), Some("\\documentclass{article}"), "project untouched");
            let err = out.diagnostics.iter().find(|d| d.severity == Severity::Error).expect("an error diagnostic");
            assert_eq!(err.path.as_deref(), Some(format!("{project}/main.tex").as_str()), "mapped back to the project file");
            assert_eq!(err.line, Some(3));
            let pdf = out.pdf_path.clone().expect("pdf");
            assert!(pdf.starts_with(&compiler.shadow_directory(&project)) && pdf.ends_with("/out/main.pdf"), "{pdf}");
            assert_eq!(out.exported_pdf_path.as_deref(), Some(format!("{project}/main.pdf").as_str()));
            assert_eq!(fs::read_to_string(format!("{project}/main.pdf")).unwrap(), "%PDF-fake\n");
            assert!(out.synctex_path.as_deref().is_some_and(|p| p.ends_with("main.synctex.gz")));
            assert!(out.log_path.as_deref().is_some_and(|p| p.ends_with("/out/main.log")));
            let ipc = out.to_ipc(&format!("{project}/main.tex"));
            assert!(!ipc.ok);
            assert_eq!(ipc.pdf_path.as_deref(), Some(pdf.as_str()));
            assert!(ipc.diagnostics.iter().any(|d| d.severity == "error" && d.line == 3 && d.source == "latex"));

            // A live compile does not export; the same compiler is reusable.
            let mut live = CompileRequest::new(&project, &format!("{project}/main.tex"), Engine::Xelatex, Purpose::Live);
            live.overlays.insert(format!("{project}/main.tex"), "second\n".into());
            fs::remove_file(format!("{project}/main.pdf")).unwrap();
            let out2 = compiler.compile(&live);
            assert!(out2.log.contains("SEEN-BY-TEX: second|"));
            assert_eq!(out2.exported_pdf_path, None);
            assert!(!Path::new(&format!("{project}/main.pdf")).exists());

            // The root outside the project / missing is a tool error with a message.
            let bad = compiler.compile(&CompileRequest::new(&project, &format!("{base}/elsewhere.tex"), Engine::Pdflatex, Purpose::Live));
            assert!(matches!(&bad.status, CompileStatus::ToolError(m) if m.contains("not inside the project folder")), "{:?}", bad.status);
            let missing = compiler.compile(&CompileRequest::new(&project, &format!("{project}/nothere.tex"), Engine::Pdflatex, Purpose::Live));
            assert!(matches!(&missing.status, CompileStatus::ToolError(m) if m.contains("does not exist")), "{:?}", missing.status);
            let ipc = missing.to_ipc(&format!("{project}/nothere.tex"));
            assert!(!ipc.ok && ipc.diagnostics.len() == 1 && ipc.diagnostics[0].severity == "error");

            compiler.clean(&project);
            assert!(!Path::new(&compiler.shadow_directory(&project)).exists(), "clean removes the workspace");
            let _ = fs::remove_dir_all(&base);
        }

        #[test]
        fn missing_latexmk_and_engine_is_a_tool_error() {
            let base = scratch("compile-notools");
            let project = format!("{base}/proj");
            write(&format!("{project}/main.tex"), "x\n");
            let mut dist = fake_distribution(&base, "exit 0");
            dist.tools.clear();
            let compiler = LaTeXCompiler::with_shadow_root(dist, &format!("{base}/shadow"));
            let out = compiler.compile(&CompileRequest::new(&project, &format!("{project}/main.tex"), Engine::Pdflatex, Purpose::Live));
            assert_eq!(out.status, CompileStatus::ToolError("latexmk was not found in Fake TeX.".into()));
            let _ = fs::remove_dir_all(&base);
        }

        #[test]
        fn tool_that_does_not_run_tex_is_reported() {
            let base = scratch("compile-nolog");
            let project = format!("{base}/proj");
            write(&format!("{project}/main.tex"), "x\n");
            let compiler = LaTeXCompiler::with_shadow_root(fake_distribution(&base, "echo 'perl: command not found'; exit 2"), &format!("{base}/shadow"));
            let out = compiler.compile(&CompileRequest::new(&project, &format!("{project}/main.tex"), Engine::Pdflatex, Purpose::Live));
            assert!(matches!(&out.status, CompileStatus::ToolError(m) if m.contains("latexmk did not run TeX: perl: command not found")), "{:?}", out.status);
            let _ = fs::remove_dir_all(&base);
        }

        #[test]
        fn direct_engine_fallback_without_latexmk() {
            use std::os::unix::fs::PermissionsExt;
            let base = scratch("compile-direct");
            let project = format!("{base}/proj");
            write(&format!("{project}/main.tex"), "x\n");
            let mut dist = fake_distribution(&base, "exit 0");
            // A pdflatex stand-in that asks for one rerun, then is satisfied.
            let engine = format!("{}/pdflatex", dist.bin_dir);
            let body = r#"#!/bin/sh
for a in "$@"; do case "$a" in -output-directory=*) out="${a#-output-directory=}";; *) root="$a";; esac; done
job="${root%.tex}"
mkdir -p "$out"
n=0; [ -f "$out/count" ] && n=$(cat "$out/count"); n=$((n+1)); echo $n > "$out/count"
if [ $n -lt 2 ]; then echo "LaTeX Warning: Label(s) may have changed. Rerun to get cross-references right." > "$out/$job.log"; else echo "(./$root)" > "$out/$job.log"; fi
echo "%PDF-direct" > "$out/$job.pdf"
"#;
            fs::write(&engine, body).unwrap();
            fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).unwrap();
            dist.tools.clear();
            dist.tools.insert(Tool::Pdflatex, engine);
            let compiler = LaTeXCompiler::with_shadow_root(dist, &format!("{base}/shadow"));
            let out = compiler.compile(&CompileRequest::new(&project, &format!("{project}/main.tex"), Engine::Pdflatex, Purpose::Live));
            assert_eq!(out.status, CompileStatus::Succeeded, "{:?} {}", out.diagnostics, out.tool_output);
            let count = fs::read_to_string(format!("{}/count", parent_of(out.pdf_path.as_deref().unwrap()))).unwrap();
            assert_eq!(count.trim(), "2", "reran once");
            assert!(out.pdf_path.is_some());
            let _ = fs::remove_dir_all(&base);
        }

        #[test]
        fn a_newer_compile_cancels_the_one_in_flight() {
            let base = scratch("compile-cancel");
            let project = format!("{base}/proj");
            write(&format!("{project}/main.tex"), "x\n");
            let slow = "echo $$ > \"$PWD/../pid\"; sleep 30";
            let dist = fake_distribution(&base, slow);
            let compiler = Arc::new(LaTeXCompiler::with_shadow_root(dist, &format!("{base}/shadow")));
            let req = CompileRequest::new(&project, &format!("{project}/main.tex"), Engine::Pdflatex, Purpose::Live);
            let started = Instant::now();
            let (c1, r1) = (compiler.clone(), req.clone());
            let first = std::thread::spawn(move || c1.compile(&r1));
            std::thread::sleep(Duration::from_millis(500)); // let the first one start its process
            let (c2, mut r2) = (compiler.clone(), req.clone());
            r2.timeout = Some(Duration::from_millis(400));
            let second = std::thread::spawn(move || c2.compile(&r2));
            let out1 = first.join().unwrap();
            assert_eq!(out1.status, CompileStatus::Cancelled, "the superseded compile is cancelled");
            let out2 = second.join().unwrap();
            assert!(matches!(out2.status, CompileStatus::ToolError(_) | CompileStatus::Failed), "{:?}", out2.status);
            assert!(started.elapsed() < Duration::from_secs(15), "nothing waited for sleep 30");
            // cancel_compile / compiler.cancel supersede too.
            compiler.cancel(&project);
            let _ = fs::remove_dir_all(&base);
        }

        #[test]
        fn timeout_adds_an_endless_loop_diagnostic() {
            let base = scratch("compile-timeout");
            let project = format!("{base}/proj");
            write(&format!("{project}/main.tex"), "x\n");
            let body = r#"
for a in "$@"; do case "$a" in -outdir=*) out="${a#-outdir=}";; esac; done
mkdir -p "$out"; echo "(./main.tex" > "$out/main.log"; sleep 30"#;
            let compiler = LaTeXCompiler::with_shadow_root(fake_distribution(&base, body), &format!("{base}/shadow"));
            let mut req = CompileRequest::new(&project, &format!("{project}/main.tex"), Engine::Pdflatex, Purpose::Live);
            req.timeout = Some(Duration::from_millis(1000));
            let out = compiler.compile(&req);
            assert_eq!(out.status, CompileStatus::Failed);
            assert!(out.diagnostics.iter().any(|d| d.message.contains("Compilation was stopped after 1 seconds")), "{:?}", out.diagnostics);
            let _ = fs::remove_dir_all(&base);
        }
    }

    /// Integration test against a real TeX installation; skipped when none is installed.
    #[test]
    fn real_latexmk_compile_when_available() {
        let Some(dist) = distribution::locate(None) else {
            eprintln!("skipped: no TeX distribution found");
            return;
        };
        if !dist.has(Tool::Latexmk) {
            eprintln!("skipped: no latexmk");
            return;
        }
        let base = scratch("compile-real");
        let project = format!("{base}/proj");
        write(&format!("{project}/main.tex"), "\\documentclass{article}\n\\begin{document}\nHello \\input{sec}\n\\end{document}\n");
        write(&format!("{project}/sec.tex"), "world\n");
        let compiler = LaTeXCompiler::with_shadow_root(dist, &format!("{base}/shadow"));
        let mut req = CompileRequest::new(&project, &format!("{project}/main.tex"), Engine::Pdflatex, Purpose::Live);
        let ok = compiler.compile(&req);
        assert_eq!(ok.status, CompileStatus::Succeeded, "{:?}\n{}", ok.diagnostics, ok.tool_output);
        assert!(ok.pdf_path.is_some() && ok.synctex_path.is_some());
        req.overlays.insert(format!("{project}/sec.tex"), "\\undefinedcommandhere\n".into());
        let bad = compiler.compile(&req);
        assert_eq!(bad.status, CompileStatus::Failed);
        let e = bad.diagnostics.iter().find(|d| d.severity == Severity::Error).unwrap();
        assert_eq!(e.path.as_deref(), Some(format!("{project}/sec.tex").as_str()));
        let _ = fs::remove_dir_all(&base);
    }
}

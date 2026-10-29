//! File operations of the sidebar (port of `ProjectWorkspace+Files.swift`): create, rename, duplicate, move to
//! the trash, open in the default app, clean auxiliary files. All paths are absolute strings; every function
//! returns a user-presentable error message.
use std::fs;
use std::io;
use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;

/// Extensions of the auxiliary files TeX runs leave behind (`FileTreeScanner.hiddenExtensions` on the Mac).
pub const AUXILIARY_EXTENSIONS: [&str; 23] = [
    "aux", "log", "out", "toc", "lof", "lot", "fls", "fdb_latexmk", "bbl", "blg", "bcf", "nav", "snm", "vrb", "idx", "ilg", "ind",
    "run.xml", "synctex", "gz", "xdv", "dvi", "listing",
];

/// `validatedName`: trims, refuses empty names, “/” (and “\” on Windows), “.” and “..”.
pub fn validated_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    let bad_sep = name.contains('/') || (cfg!(windows) && name.contains('\\'));
    if name.is_empty() || bad_sep || name == "." || name == ".." || name.contains('\0') {
        return Err("Names cannot be empty or contain “/”.".into());
    }
    Ok(name.to_string())
}

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

fn already_exists(name: &str) -> String {
    format!("“{name}” already exists")
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Creates an empty file in `dir` (adds “.tex” when the name has no extension). Returns its path.
pub fn create_file(dir: &str, raw_name: &str) -> Result<String, String> {
    let mut name = validated_name(raw_name)?;
    if Path::new(&name).extension().is_none() {
        name.push_str(".tex");
    }
    let path = Path::new(dir).join(&name);
    if exists(&path) {
        return Err(already_exists(&name));
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("Could not create “{name}”: {e}"))?;
    Ok(path_string(&path))
}

pub fn create_folder(dir: &str, raw_name: &str) -> Result<String, String> {
    let name = validated_name(raw_name)?;
    let path = Path::new(dir).join(&name);
    if exists(&path) {
        return Err(already_exists(&name));
    }
    fs::create_dir_all(&path).map_err(|e| format!("Could not create “{name}”: {e}"))?;
    Ok(path_string(&path))
}

/// Renames in place. Returns the new path (the old one when the name did not change).
pub fn rename(path: &str, raw_name: &str) -> Result<String, String> {
    let from = Path::new(path);
    let name = validated_name(raw_name)?;
    let old_name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name == old_name {
        return Ok(path.to_string());
    }
    let parent = from.parent().ok_or_else(|| "This item cannot be renamed.".to_string())?;
    let to = parent.join(&name);
    // A case-only rename on a case-insensitive volume "exists" as the source itself.
    if exists(&to) && !(name.to_lowercase() == old_name.to_lowercase() && same_file(from, &to)) {
        return Err(already_exists(&name));
    }
    fs::rename(from, &to).map_err(|e| format!("Could not rename “{old_name}”: {e}"))?;
    Ok(path_string(&to))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// `duplicate`: “name copy.ext”, “name copy 2.ext”, … next to the original. Returns the copy's path.
pub fn duplicate(path: &str) -> Result<String, String> {
    let from = Path::new(path);
    let dir = from.parent().ok_or_else(|| "This item cannot be duplicated.".to_string())?;
    let base = from.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = from.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut attempt = 1u32;
    let dest = loop {
        let suffix = if attempt == 1 { "copy".to_string() } else { format!("copy {attempt}") };
        let name = if ext.is_empty() { format!("{base} {suffix}") } else { format!("{base} {suffix}.{ext}") };
        let candidate = dir.join(name);
        if !exists(&candidate) {
            break candidate;
        }
        attempt += 1;
    };
    copy_recursive(from, &dest).map_err(|e| {
        let _ = remove_any(&dest);
        format!("Could not duplicate “{}”: {e}", from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
    })?;
    Ok(path_string(&dest))
}

fn copy_recursive(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else if meta.file_type().is_symlink() {
        // Copy what the link points to (a duplicate is a real file); a dangling link is an error.
        let target = fs::metadata(from)?;
        if target.is_dir() {
            copy_recursive(&fs::canonicalize(from)?, to)
        } else {
            fs::copy(from, to).map(|_| ())
        }
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

fn remove_any(p: &Path) -> io::Result<()> {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------------------------------
// Trash
// ---------------------------------------------------------------------------------------------------

/// Moves a file or folder to the system trash (`FileManager.trashItem`).
pub fn move_to_trash(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if !exists(p) {
        return Err(format!("“{path}” does not exist."));
    }
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
    trash_impl(p).map_err(|e| format!("Could not move “{name}” to the Trash: {e}"))
}

#[cfg(not(windows))]
fn trash_impl(p: &Path) -> Result<(), String> {
    let data_home = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from) {
        Some(d) if d.is_absolute() => d,
        _ => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?).join(".local/share"),
    };
    trash_into(p, &data_home.join("Trash"), &local_timestamp())
}

/// Freedesktop trash specification: `files/<name>` plus `info/<name>.trashinfo`, unique names, info first.
#[cfg_attr(windows, allow(dead_code))]
pub fn trash_into(p: &Path, trash: &Path, deletion_date: &str) -> Result<(), String> {
    let files = trash.join("files");
    let info = trash.join("info");
    fs::create_dir_all(&files).map_err(|e| e.to_string())?;
    fs::create_dir_all(&info).map_err(|e| e.to_string())?;
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map_err(|e| e.to_string())?.join(p) };
    let original = path_string(&abs);
    let name = abs.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or("This item cannot be trashed.")?;
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };
    let mut n = 0u32;
    let (unique, info_path) = loop {
        let candidate = if n == 0 { name.clone() } else { format!("{stem}.{n}{ext}") };
        let info_path = info.join(format!("{candidate}.trashinfo"));
        // create_new makes claiming the name atomic against another process trashing at the same time.
        match fs::OpenOptions::new().write(true).create_new(true).open(&info_path) {
            Ok(_) if !exists(&files.join(&candidate)) => break (candidate, info_path),
            Ok(_) => {
                let _ = fs::remove_file(&info_path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
        n += 1;
    };
    let body = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", percent_encode(&original), deletion_date);
    if let Err(e) = fs::write(&info_path, body) {
        let _ = fs::remove_file(&info_path);
        return Err(e.to_string());
    }
    let dest = files.join(&unique);
    let moved = fs::rename(p, &dest).or_else(|_| {
        // Another filesystem: copy then delete (a failed copy leaves the original untouched).
        copy_recursive(p, &dest).and_then(|_| remove_any(p)).inspect_err(|_| {
            let _ = remove_any(&dest);
        })
    });
    if let Err(e) = moved {
        let _ = fs::remove_file(&info_path);
        return Err(e.to_string());
    }
    Ok(())
}

/// RFC 3986 percent-encoding that keeps “/” (what `.trashinfo` `Path=` expects).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Local time `YYYY-MM-DDThh:mm:ss` (via `date`; falls back to UTC computed by hand).
#[cfg(not(windows))]
fn local_timestamp() -> String {
    if let Ok(out) = std::process::Command::new("date").arg("+%Y-%m-%dT%H:%M:%S").output() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() && s.len() == 19 {
            return s;
        }
    }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    utc_timestamp(secs)
}

#[cfg_attr(windows, allow(dead_code))]
pub fn utc_timestamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(windows)]
fn trash_impl(p: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE, SHFILEOPSTRUCTW};
    let abs = fs::canonicalize(p).map_err(|e| e.to_string())?;
    // SHFileOperation refuses the “\\?\” prefix canonicalize adds.
    let text = path_string(&abs);
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_string();
    // The path list is double-NUL terminated.
    let mut wide: Vec<u16> = std::ffi::OsStr::new(&text).encode_wide().collect();
    wide.push(0);
    wide.push(0);
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE as _;
    op.pFrom = wide.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as _;
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 {
        return Err(format!("the shell returned error {code:#x}"));
    }
    let aborted = op.fAnyOperationsAborted;
    if aborted != 0 || exists(p) {
        return Err("the operation was cancelled".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------
// Opening, cleaning
// ---------------------------------------------------------------------------------------------------

/// Opens a file or folder with the system's default application.
pub fn open_in_default_app(path: &str) -> Result<(), String> {
    use std::process::{Command, Stdio};
    if !exists(Path::new(path)) {
        return Err(format!("“{path}” does not exist."));
    }
    let mut cmd;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd = Command::new("explorer.exe");
        cmd.arg(path);
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(target_os = "macos")]
    {
        cmd = Command::new("open");
        cmd.arg(path);
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        cmd = Command::new("xdg-open");
        cmd.arg(path);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(not(windows))]
    {
        let spawned = cmd.spawn();
        match spawned {
            Ok(mut child) => {
                // Reap in the background so no zombie stays behind.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                Ok(())
            }
            Err(e) => Err(format!("Could not open “{path}”: {e}")),
        }
    }
    #[cfg(windows)]
    cmd.spawn().map(|_| ()).map_err(|e| format!("Could not open “{path}”: {e}"))
}

/// Is `name` an auxiliary file the Mac app hides (`run.xml` is a double extension; `.gz` only counts for
/// `.synctex.gz`, so a user's own archives are never touched)?
pub fn is_auxiliary_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.starts_with('.') && !lower[1..].contains('.') {
        return false;
    }
    AUXILIARY_EXTENSIONS.iter().any(|ext| {
        if *ext == "gz" {
            lower.ends_with(".synctex.gz")
        } else {
            lower.len() > ext.len() + 1 && lower.ends_with(&format!(".{ext}"))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gabetex-fileops-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names_are_validated() {
        assert!(validated_name("  ").is_err());
        assert!(validated_name("a/b").is_err());
        assert!(validated_name("..").is_err());
        assert_eq!(validated_name(" ok.tex ").unwrap(), "ok.tex");
    }

    #[test]
    fn create_rename_duplicate() {
        let d = tmp("crd");
        let dir = path_string(&d);
        let a = create_file(&dir, "chapter").unwrap();
        assert!(a.ends_with("chapter.tex"));
        assert!(create_file(&dir, "chapter.tex").unwrap_err().contains("already exists"));
        fs::write(&a, "x").unwrap();
        let c1 = duplicate(&a).unwrap();
        assert!(c1.ends_with("chapter copy.tex"));
        let c2 = duplicate(&a).unwrap();
        assert!(c2.ends_with("chapter copy 2.tex"));
        assert_eq!(fs::read_to_string(&c2).unwrap(), "x");
        let r = rename(&a, "intro.tex").unwrap();
        assert!(r.ends_with("intro.tex") && !Path::new(&a).exists());
        assert!(rename(&c1, "intro.tex").unwrap_err().contains("already exists"));
        assert_eq!(rename(&r, "intro.tex").unwrap(), r);
        let f = create_folder(&dir, "figs").unwrap();
        fs::write(Path::new(&f).join("p.txt"), "p").unwrap();
        let fc = duplicate(&f).unwrap();
        assert!(fc.ends_with("figs copy"));
        assert_eq!(fs::read_to_string(Path::new(&fc).join("p.txt")).unwrap(), "p");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn trash_follows_the_freedesktop_spec() {
        let d = tmp("trash");
        let trash = d.join("Trash");
        for _ in 0..2 {
            let f = d.join("my file.tex");
            fs::write(&f, "x").unwrap();
            trash_into(&f, &trash, "2026-01-02T03:04:05").unwrap();
            assert!(!f.exists());
        }
        assert!(trash.join("files/my file.tex").exists());
        assert!(trash.join("files/my file.1.tex").exists());
        let info = fs::read_to_string(trash.join("info/my file.tex.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath="));
        assert!(info.contains("my%20file.tex"));
        assert!(info.contains("DeletionDate=2026-01-02T03:04:05"));
        let dir = d.join("folder");
        fs::create_dir_all(dir.join("sub")).unwrap();
        trash_into(&dir, &trash, "2026-01-02T03:04:05").unwrap();
        assert!(trash.join("files/folder/sub").is_dir());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn timestamps_and_encoding() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00");
        assert_eq!(utc_timestamp(1_709_210_096), "2024-02-29T12:34:56");
        assert_eq!(percent_encode("/a b/é"), "/a%20b/%C3%A9");
    }

    #[test]
    fn auxiliary_detection() {
        for n in ["main.aux", "main.run.xml", "main.synctex.gz", "main.FDB_LATEXMK", "a.bbl"] {
            assert!(is_auxiliary_name(n), "{n}");
        }
        for n in ["main.tex", "data.tar.gz", "refs.bib", ".aux", "aux", "fig.pdf"] {
            assert!(!is_auxiliary_name(n), "{n}");
        }
    }
}

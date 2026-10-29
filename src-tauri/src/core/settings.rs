//! App-level persistence and file helpers.
//!
//! * settings JSON + recent projects (with colour labels) in the OS config dir
//!   (Linux `$XDG_CONFIG_HOME/GabeTeX` or `~/.config/GabeTeX`, Windows `%APPDATA%\GabeTeX`, macOS
//!   `~/Library/Application Support/GabeTeX`), every write atomic (temp file + rename);
//! * recents policy of `RecentProjects.swift` / `ProjectLabels.swift`: most recent first, the cap
//!   ([`RECENT_LIMIT`] = 15) applies to **unlabeled** projects only, labels live outside the list (removing a
//!   project from the list keeps its label), a renamed or moved folder brings its label along (folder identity);
//! * [`write_atomic`] (port of `TextFileIO.writeData`): permissions preserved, hard links kept, symlinks written
//!   through, read-only files refused;
//! * text decoding / encoding (port of `TextFileIO.decode`/`encode`): UTF-8 first (BOM kept), UTF-16 with BOM,
//!   ISO Latin-1 for legacy files;
//! * [`scan_file_tree`] (port of `FileTreeScanner`) and [`reveal_in_file_manager`].

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::{FileNode, RecentProject};
use crate::core::log_parser::lexically_standardized;

// ---------------------------------------------------------------------------------------------------
// Config directory
// ---------------------------------------------------------------------------------------------------

const APP_DIR: &str = "GabeTeX";

/// Linux rule: `$XDG_CONFIG_HOME` when set to an absolute path, else `$HOME/.config`; plus `GabeTeX`.
pub fn linux_config_dir(getenv: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = match getenv("XDG_CONFIG_HOME").filter(|v| Path::new(v).is_absolute()) {
        Some(x) => PathBuf::from(x),
        None => PathBuf::from(getenv("HOME").filter(|h| !h.is_empty())?).join(".config"),
    };
    Some(base.join(APP_DIR))
}

/// Windows rule: `%APPDATA%\GabeTeX` (fallback `%USERPROFILE%\AppData\Roaming\GabeTeX`).
pub fn windows_config_dir(getenv: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = match getenv("APPDATA").filter(|v| !v.is_empty()) {
        Some(a) => PathBuf::from(a),
        None => PathBuf::from(getenv("USERPROFILE").filter(|v| !v.is_empty())?).join("AppData").join("Roaming"),
    };
    Some(base.join(APP_DIR))
}

/// The directory that holds `settings.json` and `recent.json` (not created here).
pub fn config_dir() -> PathBuf {
    let env = |k: &str| std::env::var(k).ok();
    #[cfg(windows)]
    let dir = windows_config_dir(&env);
    #[cfg(target_os = "macos")]
    let dir = env("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join("Library").join("Application Support").join(APP_DIR));
    #[cfg(not(any(windows, target_os = "macos")))]
    let dir = linux_config_dir(&env);
    dir.unwrap_or_else(|| std::env::temp_dir().join(APP_DIR))
}

// ---------------------------------------------------------------------------------------------------
// Atomic writes
// ---------------------------------------------------------------------------------------------------

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_sibling(target: &Path) -> PathBuf {
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let n = TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    let unique = format!("{:x}{:x}{:x}", std::process::id(), n, nanos);
    target.with_file_name(format!(".{name}.gabetex-{}", &unique[..unique.len().min(10)]))
}

fn read_only_error() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "The file is read-only. Make it writable, then save again.")
}

/// Writes `data` to `path` the way the Mac app saves files (`TextFileIO.writeData`):
///
/// * a symbolic link is written through;
/// * a read-only file is refused (`PermissionDenied`) and never overridden;
/// * a file with several hard links is rewritten in place (the links stay one file);
/// * otherwise the write is atomic (hidden temp file in the same folder + rename) and the original's
///   permissions are copied to the new file; a new file gets the default permissions (umask applied);
/// * when no file can be created next to a writable file, it is rewritten in place.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let target: PathBuf = match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    };
    let existing = fs::metadata(&target).ok();
    if let Some(meta) = &existing {
        if meta.is_dir() {
            return Err(io::Error::other("Is a directory"));
        }
        // The real test for "may I write this file" (honours ACLs, read-only attribute, immutable flags).
        match fs::OpenOptions::new().write(true).open(&target) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return Err(read_only_error()),
            Err(e) => return Err(e),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() > 1 {
                return overwrite_in_place(&target, data);
            }
        }
    }
    match replace_atomically(&target, data, existing.as_ref()) {
        Err(e) if existing.is_some() && e.kind() == io::ErrorKind::PermissionDenied => overwrite_in_place(&target, data),
        other => other,
    }
}

/// Rewrites the bytes through the existing inode (keeps hard links, owner and metadata).
fn overwrite_in_place(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().write(true).open(path)?;
    file.write_all(data)?;
    // Truncate after writing: the file is never shorter than either version in between.
    file.set_len(data.len() as u64)?;
    let _ = file.sync_all();
    Ok(())
}

fn replace_atomically(path: &Path, data: &[u8], original: Option<&fs::Metadata>) -> io::Result<()> {
    let temporary = temp_sibling(path);
    let result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(data)?;
        if let Some(meta) = original {
            let _ = file.set_permissions(meta.permissions());
        }
        let _ = file.sync_all();
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

// ---------------------------------------------------------------------------------------------------
// Text encoding
// ---------------------------------------------------------------------------------------------------

/// Files above this size are not opened in the editor.
pub const MAX_EDITABLE_BYTES: u64 = 24 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Latin1,
    Utf16Le,
    Utf16Be,
}

/// How a text file was encoded on disk (remembered per buffer so saving writes it back unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextFormat {
    pub encoding: TextEncoding,
    pub has_bom: bool,
}

impl TextFormat {
    pub const UTF8: TextFormat = TextFormat { encoding: TextEncoding::Utf8, has_bom: false };
    pub const LATIN1: TextFormat = TextFormat { encoding: TextEncoding::Latin1, has_bom: false };

    /// IPC name: `"utf-8"`, `"utf-8-bom"`, `"latin-1"`, `"utf-16le"`, `"utf-16be"` (the contract's two values
    /// are `utf-8` and `latin-1`; the others only occur for files that carry a BOM).
    pub fn name(self) -> &'static str {
        match (self.encoding, self.has_bom) {
            (TextEncoding::Utf8, false) => "utf-8",
            (TextEncoding::Utf8, true) => "utf-8-bom",
            (TextEncoding::Latin1, _) => "latin-1",
            (TextEncoding::Utf16Le, _) => "utf-16le",
            (TextEncoding::Utf16Be, _) => "utf-16be",
        }
    }

    /// Inverse of [`TextFormat::name`] (also accepts a few common spellings).
    pub fn parse(name: &str) -> Option<TextFormat> {
        match name.trim().to_ascii_lowercase().as_str() {
            "utf-8" | "utf8" => Some(Self::UTF8),
            "utf-8-bom" | "utf8-bom" | "utf-8 bom" => Some(TextFormat { encoding: TextEncoding::Utf8, has_bom: true }),
            "latin-1" | "latin1" | "iso-8859-1" | "iso latin 1" => Some(Self::LATIN1),
            "utf-16le" | "utf-16 le" => Some(TextFormat { encoding: TextEncoding::Utf16Le, has_bom: true }),
            "utf-16be" | "utf-16 be" => Some(TextFormat { encoding: TextEncoding::Utf16Be, has_bom: true }),
            _ => None,
        }
    }

    /// Short label for the status bar (a technical name, never translated).
    pub fn status_name(self) -> &'static str {
        match (self.encoding, self.has_bom) {
            (TextEncoding::Utf8, false) => "UTF-8",
            (TextEncoding::Utf8, true) => "UTF-8 BOM",
            (TextEncoding::Latin1, _) => "Latin-1",
            (TextEncoding::Utf16Le, _) => "UTF-16 LE",
            (TextEncoding::Utf16Be, _) => "UTF-16 BE",
        }
    }

    fn display_name(self) -> &'static str {
        match (self.encoding, self.has_bom) {
            (TextEncoding::Utf8, false) => "UTF-8",
            (TextEncoding::Utf8, true) => "UTF-8 BOM",
            (TextEncoding::Latin1, _) => "ISO Latin 1",
            _ => "UTF-16",
        }
    }
}

/// A decoded text file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub text: String,
    pub format: TextFormat,
    /// Bytes that were not valid UTF-8 and were replaced with U+FFFD (0 for a clean file).
    pub invalid_byte_count: usize,
}

/// Heuristic: a NUL byte in the first 8 KB means binary.
pub fn looks_binary(data: &[u8]) -> bool {
    data[..data.len().min(8192)].contains(&0)
}

/// Counts well-formed multi-byte UTF-8 sequences and bytes that belong to no well-formed sequence.
pub fn scan_utf8(data: &[u8]) -> (usize, usize) {
    let count = data.len();
    let (mut multi, mut invalid) = (0usize, 0usize);
    let mut i = 0;
    while i < count {
        let lead = data[i];
        if lead < 0x80 {
            i += 1;
            continue;
        }
        let (mut low, mut high) = (0x80u8, 0xBFu8);
        let continuation = match lead {
            0xC2..=0xDF => 1,
            0xE0 => {
                low = 0xA0;
                2
            }
            0xE1..=0xEC | 0xEE..=0xEF => 2,
            0xED => {
                high = 0x9F;
                2
            }
            0xF0 => {
                low = 0x90;
                3
            }
            0xF1..=0xF3 => 3,
            0xF4 => {
                high = 0x8F;
                3
            }
            _ => {
                invalid += 1;
                i += 1;
                continue;
            }
        };
        let mut valid = true;
        for offset in 1..=continuation {
            if i + offset >= count {
                valid = false;
                break;
            }
            let byte = data[i + offset];
            let (lower, upper) = if offset == 1 { (low, high) } else { (0x80, 0xBF) };
            if byte < lower || byte > upper {
                valid = false;
                break;
            }
        }
        if valid {
            multi += 1;
            i += continuation + 1;
        } else {
            invalid += 1;
            i += 1;
        }
    }
    (multi, invalid)
}

fn decode_utf16(data: &[u8], little: bool) -> Option<String> {
    if data.len() % 2 != 0 {
        return None;
    }
    let units = data.chunks_exact(2).map(|c| if little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) });
    let s: String = char::decode_utf16(units).collect::<Result<String, _>>().ok()?;
    Some(s.trim_start_matches('\u{FEFF}').to_string())
}

fn latin1_to_string(data: &[u8]) -> String {
    data.iter().map(|&b| b as char).collect()
}

/// `TextFileIO.decode`: `None` when the data is binary (NUL bytes outside UTF-16) or cannot be decoded.
///
/// UTF-8 is tried first. Data that is not valid UTF-8 is read as ISO Latin-1 only when it contains no
/// well-formed multi-byte UTF-8 sequence at all (a real legacy file); a mostly-UTF-8 file with a few stray
/// bytes stays UTF-8, the bad bytes become U+FFFD and `invalid_byte_count` says how many.
pub fn decode(data: &[u8]) -> Option<Loaded> {
    if data.is_empty() {
        return Some(Loaded { text: String::new(), format: TextFormat::UTF8, invalid_byte_count: 0 });
    }
    if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xFE {
        if let Some(text) = decode_utf16(data, true) {
            return Some(Loaded { text, format: TextFormat { encoding: TextEncoding::Utf16Le, has_bom: true }, invalid_byte_count: 0 });
        }
    }
    if data.len() >= 2 && data[0] == 0xFE && data[1] == 0xFF {
        if let Some(text) = decode_utf16(data, false) {
            return Some(Loaded { text, format: TextFormat { encoding: TextEncoding::Utf16Be, has_bom: true }, invalid_byte_count: 0 });
        }
    }
    if looks_binary(data) {
        return None;
    }
    let (body, bom) = if data.len() >= 3 && data[..3] == [0xEF, 0xBB, 0xBF] { (&data[3..], true) } else { (data, false) };
    if let Ok(s) = std::str::from_utf8(body) {
        return Some(Loaded { text: s.to_string(), format: TextFormat { encoding: TextEncoding::Utf8, has_bom: bom }, invalid_byte_count: 0 });
    }
    let (multi, invalid) = scan_utf8(body);
    if multi > 0 {
        // UTF-8 with stray bytes: keep it UTF-8, repair lossily, and let the caller warn.
        return Some(Loaded {
            text: String::from_utf8_lossy(body).into_owned(),
            format: TextFormat { encoding: TextEncoding::Utf8, has_bom: bom },
            invalid_byte_count: invalid.max(1),
        });
    }
    // Legacy Brazilian files: ISO Latin-1 never fails.
    Some(Loaded { text: latin1_to_string(data), format: TextFormat::LATIN1, invalid_byte_count: 0 })
}

/// `TextFileIO.decodeLatin1`: the "Reopen as Latin-1" action.
pub fn decode_latin1(data: &[u8]) -> Option<Loaded> {
    if looks_binary(data) {
        return None;
    }
    Some(Loaded { text: latin1_to_string(data), format: TextFormat::LATIN1, invalid_byte_count: 0 })
}

/// `TextFileIO.encode`: the bytes to store for `text` in `format` (BOM included). Fails when the text has
/// characters the encoding cannot represent (Latin-1).
pub fn encode(text: &str, format: TextFormat) -> Result<Vec<u8>, String> {
    match format.encoding {
        TextEncoding::Utf8 => {
            let mut out = Vec::with_capacity(text.len() + 3);
            if format.has_bom {
                out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            out.extend_from_slice(text.as_bytes());
            Ok(out)
        }
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            let little = format.encoding == TextEncoding::Utf16Le;
            let mut out = Vec::with_capacity(text.len() * 2 + 2);
            let mut push = |u: u16| out.extend_from_slice(&if little { u.to_le_bytes() } else { u.to_be_bytes() });
            if format.has_bom {
                push(0xFEFF);
            }
            text.encode_utf16().for_each(&mut push);
            Ok(out)
        }
        TextEncoding::Latin1 => {
            let mut out = Vec::with_capacity(text.len());
            for c in text.chars() {
                let v = c as u32;
                if v > 0xFF {
                    return Err(format!(
                        "The text contains characters that cannot be stored in the file’s {} encoding.",
                        format.display_name()
                    ));
                }
                out.push(v as u8);
            }
            Ok(out)
        }
    }
}

/// `TextFileIO.read`: reads and decodes a file (as Latin-1 when `as_latin1`). Files over 24 MB are refused.
pub fn read_text_file(path: &Path, as_latin1: bool) -> Result<Loaded, String> {
    if let Ok(meta) = fs::metadata(path) {
        if meta.len() > MAX_EDITABLE_BYTES {
            return Err("This file is too large to open in the editor.".into());
        }
    }
    let data = fs::read(path).map_err(|_| "The file could not be read.".to_string())?;
    if data.len() as u64 > MAX_EDITABLE_BYTES {
        return Err("This file is too large to open in the editor.".into());
    }
    let loaded = if as_latin1 { decode_latin1(&data) } else { decode(&data) };
    loaded.ok_or_else(|| "This file does not look like a text file.".to_string())
}

/// `TextFileIO.write`: encodes and saves atomically (see [`write_atomic`]).
pub fn write_text_file(path: &Path, text: &str, format: TextFormat) -> Result<(), String> {
    let data = encode(text, format)?;
    write_atomic(path, &data).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------------
// Settings + recents store
// ---------------------------------------------------------------------------------------------------

/// `RecentProjects.limit`: the cap on unlabeled recent projects.
pub const RECENT_LIMIT: usize = 15;

/// The Finder-like label palette (`LabelColor`).
pub const LABEL_COLORS: [&str; 7] = ["red", "orange", "yellow", "green", "blue", "purple", "gray"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct StoredRecent {
    path: String,
    name: String,
    last_opened: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct StoredLabel {
    label: String,
    /// Folder identity (device:inode on Unix) so the label follows a renamed or moved folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RecentsFile {
    projects: Vec<StoredRecent>,
    labels: BTreeMap<String, StoredLabel>,
}

/// Identity of a folder on disk that survives renames (`FolderIdentity`); `None` where unsupported.
pub fn folder_identity(path: &str) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = fs::metadata(path).ok()?;
        Some(format!("{}:{}", m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Standardized spelling of a project path (no `.`/`..`/trailing separator), like `standardizedFileURL.path`.
fn standard_path(path: &str) -> String {
    let std = lexically_standardized(path);
    // Windows: always the native spelling, whatever mix of separators came in, so one project has one key.
    if cfg!(windows) {
        std.replace('/', "\\")
    } else {
        std
    }
}

/// Comparison key: case-insensitive on Windows.
fn path_key(path: &str) -> String {
    let std = lexically_standardized(path);
    if cfg!(windows) { std.to_lowercase() } else { std }
}

/// `RecentsPolicy.trim`: keeps every labeled item and at most `limit` unlabeled ones (the first, i.e. most
/// recent, ones); the order is preserved.
pub fn trim_recents<T>(items: Vec<T>, limit: usize, is_labeled: impl Fn(&T) -> bool) -> Vec<T> {
    let mut unlabeled = 0;
    items
        .into_iter()
        .filter(|item| {
            if is_labeled(item) {
                return true;
            }
            unlabeled += 1;
            unlabeled <= limit
        })
        .collect()
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// `ProjectLabelStore.noteOpened`: remembers the folder's identity; when the folder has no label under this path
/// but another path with the same identity does and no longer leads to this folder (renamed or moved), the label
/// moves here. Returns the old key in that case.
fn note_opened(labels: &mut BTreeMap<String, StoredLabel>, std_path: &str) -> Option<String> {
    let key = path_key(std_path);
    let id = folder_identity(std_path)?;
    if let Some(entry) = labels.get_mut(&key) {
        if entry.file_id.as_deref() != Some(id.as_str()) {
            entry.file_id = Some(id);
        }
        return None;
    }
    let moved = labels
        .iter()
        .find(|(old, entry)| **old != key && entry.file_id.as_deref() == Some(id.as_str()) && folder_identity(old).as_deref() != Some(id.as_str()))
        .map(|(old, _)| old.clone())?;
    let entry = labels.remove(&moved)?;
    labels.insert(key, entry);
    Some(moved)
}

fn same_json_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

/// Settings and recent projects in one folder. Use [`store`] for the real config dir; tests pass a scratch dir.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    dir: PathBuf,
}

static STORE_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

impl ConfigStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        ConfigStore { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn settings_path(&self) -> PathBuf {
        same_json_path(&self.dir, "settings.json")
    }

    pub fn recents_path(&self) -> PathBuf {
        same_json_path(&self.dir, "recent.json")
    }

    fn write_json<T: Serialize>(&self, path: &Path, value: &T) -> Result<(), String> {
        fs::create_dir_all(&self.dir).map_err(|e| format!("Could not create {}: {e}", self.dir.display()))?;
        let mut text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
        text.push('\n');
        write_atomic(path, text.as_bytes()).map_err(|e| format!("Could not write {}: {e}", path.display()))
    }

    /// A corrupt file is moved aside (`<name>.corrupt`) so the next save does not lose evidence.
    fn read_json(&self, path: &Path) -> Option<Value> {
        let bytes = fs::read(path).ok()?;
        let text = decode(&bytes).map(|l| l.text).unwrap_or_default();
        match serde_json::from_str::<Value>(text.trim_start_matches('\u{FEFF}')) {
            Ok(v) => Some(v),
            Err(_) => {
                let mut aside = path.as_os_str().to_owned();
                aside.push(".corrupt");
                let _ = fs::rename(path, aside);
                None
            }
        }
    }

    // ---- settings ------------------------------------------------------------------------------

    /// The saved settings object (`{}` when there is none yet or the file is unreadable).
    pub fn load_settings(&self) -> Value {
        let _g = lock();
        match self.read_json(&self.settings_path()) {
            Some(v @ Value::Object(_)) => v,
            _ => Value::Object(Default::default()),
        }
    }

    /// Replaces the settings (must be a JSON object) and returns what was stored.
    pub fn save_settings(&self, settings: Value) -> Result<Value, String> {
        if !settings.is_object() {
            return Err("Settings must be a JSON object.".into());
        }
        let _g = lock();
        self.write_json(&self.settings_path(), &settings)?;
        Ok(settings)
    }

    // ---- recents -------------------------------------------------------------------------------

    fn load_recents(&self) -> RecentsFile {
        self.read_json(&self.recents_path()).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
    }

    fn view(file: &RecentsFile, only_existing: bool) -> Vec<RecentProject> {
        file.projects
            .iter()
            .filter(|p| !only_existing || Path::new(&p.path).exists())
            .map(|p| RecentProject {
                path: p.path.clone(),
                name: p.name.clone(),
                last_opened: p.last_opened,
                label: file.labels.get(&path_key(&p.path)).map(|l| l.label.clone()),
            })
            .collect()
    }

    /// Recent folders that still exist on disk, most recent first (`RecentProjects.existing`).
    pub fn recent_projects(&self) -> Vec<RecentProject> {
        let _g = lock();
        Self::view(&self.load_recents(), true)
    }

    /// `RecentProjects.add` at an explicit time (tests). A renamed or moved project brings its label along.
    pub fn add_recent_project_at(&self, path: &str, name: Option<&str>, now_ms: i64) -> Result<Vec<RecentProject>, String> {
        let _g = lock();
        let mut file = self.load_recents();
        let std = standard_path(path);
        let key = path_key(&std);
        if let Some(old) = note_opened(&mut file.labels, &std) {
            file.projects.retain(|p| path_key(&p.path) != old);
        }
        file.projects.retain(|p| path_key(&p.path) != key);
        let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => n.to_string(),
            None => Path::new(&std)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| std.clone()),
        };
        file.projects.insert(0, StoredRecent { path: std, name, last_opened: now_ms });
        let labels = &file.labels;
        file.projects = trim_recents(std::mem::take(&mut file.projects), RECENT_LIMIT, |p| labels.contains_key(&path_key(&p.path)));
        self.write_json(&self.recents_path(), &file)?;
        Ok(Self::view(&file, true))
    }

    pub fn add_recent_project(&self, path: &str, name: Option<&str>) -> Result<Vec<RecentProject>, String> {
        self.add_recent_project_at(path, name, now_ms())
    }

    /// `RecentProjects.remove`: the label stays (labeling is a deliberate act of organizing).
    pub fn remove_recent_project(&self, path: &str) -> Result<Vec<RecentProject>, String> {
        let _g = lock();
        let mut file = self.load_recents();
        let key = path_key(path);
        file.projects.retain(|p| path_key(&p.path) != key);
        self.write_json(&self.recents_path(), &file)?;
        Ok(Self::view(&file, true))
    }

    /// `RecentProjects.clear`: labeled projects stay.
    pub fn clear_recent_projects(&self) -> Result<Vec<RecentProject>, String> {
        let _g = lock();
        let mut file = self.load_recents();
        let labels = &file.labels;
        file.projects.retain(|p| labels.contains_key(&path_key(&p.path)));
        self.write_json(&self.recents_path(), &file)?;
        Ok(Self::view(&file, true))
    }

    /// Sets (`Some(color)`) or clears (`None`) a project's colour label. Colours: [`LABEL_COLORS`].
    pub fn set_project_label(&self, path: &str, label: Option<&str>) -> Result<Vec<RecentProject>, String> {
        let _g = lock();
        let mut file = self.load_recents();
        let key = path_key(path);
        match label.map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty()) {
            Some(color) => {
                if !LABEL_COLORS.contains(&color.as_str()) {
                    return Err(format!("Unknown label color “{color}”."));
                }
                let id = folder_identity(path);
                let entry = file.labels.entry(key).or_insert_with(|| StoredLabel { label: color.clone(), file_id: id.clone() });
                entry.label = color;
                if entry.file_id.is_none() {
                    entry.file_id = id;
                }
            }
            None => {
                file.labels.remove(&key);
            }
        }
        self.write_json(&self.recents_path(), &file)?;
        Ok(Self::view(&file, true))
    }
}

/// The store in the real OS config dir.
pub fn store() -> ConfigStore {
    ConfigStore::new(config_dir())
}

pub fn load_settings() -> Value {
    store().load_settings()
}

pub fn save_settings(settings: Value) -> Result<Value, String> {
    store().save_settings(settings)
}

pub fn recent_projects() -> Vec<RecentProject> {
    store().recent_projects()
}

pub fn add_recent_project(path: &str, name: Option<&str>) -> Result<Vec<RecentProject>, String> {
    store().add_recent_project(path, name)
}

pub fn remove_recent_project(path: &str) -> Result<Vec<RecentProject>, String> {
    store().remove_recent_project(path)
}

pub fn set_project_label(path: &str, label: Option<&str>) -> Result<Vec<RecentProject>, String> {
    store().set_project_label(path, label)
}

// ---------------------------------------------------------------------------------------------------
// File tree (FileTreeScanner)
// ---------------------------------------------------------------------------------------------------

/// Auxiliary files produced by TeX runs outside GabeTeX; hidden to keep the tree calm.
pub const HIDDEN_EXTENSIONS: [&str; 23] = [
    "aux", "log", "out", "toc", "lof", "lot", "fls", "fdb_latexmk", "bbl", "blg", "bcf", "nav", "snm", "vrb", "idx", "ilg", "ind",
    "run.xml", "synctex", "gz", "xdv", "dvi", "listing",
];
pub const SKIPPED_FOLDERS: [&str; 5] = ["node_modules", "__pycache__", ".git", ".build", ".svn"];
pub const MAX_TREE_ENTRIES: usize = 6000;
pub const MAX_TREE_DEPTH: usize = 10;

/// Result of [`scan_file_tree`].
#[derive(Debug, Clone)]
pub struct TreeScan {
    pub nodes: Vec<FileNode>,
    /// Every file (not folder) in the tree, depth first.
    pub files: Vec<String>,
    /// True when the scan stopped early (huge folder).
    pub truncated: bool,
}

fn is_hidden_extension(ext: &str) -> bool {
    HIDDEN_EXTENSIONS.contains(&ext)
}

/// `URL.pathExtension` (lowercased): text after the last dot of a name that has a non-empty stem.
fn path_extension(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

fn join_like(base: &str, name: &str) -> String {
    if base.ends_with('/') || base.ends_with('\\') {
        return format!("{base}{name}");
    }
    let sep = if base.contains('\\') && !base.contains('/') { '\\' } else { '/' };
    format!("{base}{sep}{name}")
}

#[cfg(windows)]
fn is_hidden_attribute(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x2 != 0
}

#[cfg(not(windows))]
fn is_hidden_attribute(_meta: &fs::Metadata) -> bool {
    false
}

/// Folds a character for Finder-like ordering: case and common Latin diacritics are ignored.
fn fold_char(c: char) -> char {
    match c {
        'à'..='å' | 'À'..='Å' => 'a',
        'ç' | 'Ç' => 'c',
        'è'..='ë' | 'È'..='Ë' => 'e',
        'ì'..='ï' | 'Ì'..='Ï' => 'i',
        'ñ' | 'Ñ' => 'n',
        'ò'..='ö' | 'ø' | 'Ò'..='Ö' | 'Ø' => 'o',
        'ù'..='ü' | 'Ù'..='Ü' => 'u',
        'ý' | 'ÿ' | 'Ý' => 'y',
        c => c.to_lowercase().next().unwrap_or(c),
    }
}

/// Approximation of `localizedStandardCompare` (Finder order): case- and diacritic-insensitive, digit runs
/// compare by numeric value ("file2" < "file10"); ties fall back to the plain comparison.
pub fn standard_compare(a: &str, b: &str) -> Ordering {
    let (mut ia, mut ib) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ia.peek().copied(), ib.peek().copied()) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut na = String::new();
                    while let Some(&c) = ia.peek().filter(|c| c.is_ascii_digit()) {
                        na.push(c);
                        ia.next();
                    }
                    let mut nb = String::new();
                    while let Some(&c) = ib.peek().filter(|c| c.is_ascii_digit()) {
                        nb.push(c);
                        ib.next();
                    }
                    let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                    let order = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if order != Ordering::Equal {
                        return order;
                    }
                } else {
                    let (fa, fb) = (fold_char(ca), fold_char(cb));
                    if fa != fb {
                        return fa.cmp(&fb);
                    }
                    ia.next();
                    ib.next();
                }
            }
        }
    }
    // Equal after folding: lowercase before uppercase, then code points.
    a.chars().map(|c| c.is_uppercase()).cmp(b.chars().map(|c| c.is_uppercase())).then_with(|| a.cmp(b))
}

struct Scanner {
    count: usize,
    truncated: bool,
    files: Vec<String>,
    ancestors: HashSet<PathBuf>,
}

impl Scanner {
    fn scan_directory(&mut self, directory: &str, physical: &Path, depth: usize) -> Vec<FileNode> {
        let Ok(entries) = fs::read_dir(physical) else { return Vec::new() };
        let mut folders: Vec<FileNode> = Vec::new();
        let mut plain: Vec<FileNode> = Vec::new();
        for entry in entries.flatten() {
            if self.count >= MAX_TREE_ENTRIES {
                self.truncated = true;
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            // `.skipsHiddenFiles`
            if name.starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else { continue };
            let mut is_directory = file_type.is_dir();
            let mut child_physical = physical.join(entry.file_name());
            if file_type.is_symlink() {
                // The entry describes the link itself; a link to a folder is a folder.
                if let Ok(target) = fs::metadata(entry.path()) {
                    if target.is_dir() {
                        is_directory = true;
                        child_physical = fs::canonicalize(entry.path()).unwrap_or(child_physical);
                    }
                }
            }
            if entry.metadata().map(|m| is_hidden_attribute(&m)).unwrap_or(false) {
                continue;
            }
            let path = join_like(directory, &name);
            if is_directory {
                if SKIPPED_FOLDERS.contains(&name.as_str()) {
                    continue;
                }
                self.count += 1;
                let children = if depth >= MAX_TREE_DEPTH || self.ancestors.contains(&child_physical) {
                    Vec::new()
                } else {
                    self.ancestors.insert(child_physical.clone());
                    let c = self.scan_directory(&path, &child_physical, depth + 1);
                    self.ancestors.remove(&child_physical);
                    c
                };
                folders.push(FileNode { name, path, is_dir: true, children: Some(children) });
            } else {
                let ext = path_extension(&name);
                let lower = name.to_lowercase();
                if is_hidden_extension(&ext) || lower.ends_with(".run.xml") || lower.ends_with(".synctex.gz") {
                    continue;
                }
                self.count += 1;
                self.files.push(path.clone());
                plain.push(FileNode { name, path, is_dir: false, children: None });
            }
        }
        let order = |a: &FileNode, b: &FileNode| standard_compare(&a.name, &b.name);
        folders.sort_by(order);
        plain.sort_by(order);
        folders.extend(plain);
        folders
    }
}

/// `FileTreeScanner.scan`: the project tree — hidden entries, build folders (`.git`, `node_modules` …) and TeX
/// auxiliary files skipped, folders first, Finder-like order, at most 6000 entries / 10 levels deep; symlinks
/// that point back up the tree show as empty folders.
pub fn scan_file_tree(root: &str) -> TreeScan {
    let physical = fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let mut scanner = Scanner { count: 0, truncated: false, files: Vec::new(), ancestors: HashSet::new() };
    scanner.ancestors.insert(physical.clone());
    let trimmed = if root.len() > 1 { root.trim_end_matches(['/', '\\']) } else { root };
    let base = if trimmed.is_empty() { root } else { trimmed };
    let nodes = scanner.scan_directory(base, &physical, 0);
    TreeScan { nodes, files: scanner.files, truncated: scanner.truncated }
}

/// The `list_files` command: the tree without the bookkeeping.
pub fn list_files(root: &str) -> Vec<FileNode> {
    scan_file_tree(root).nodes
}

// ---------------------------------------------------------------------------------------------------
// Reveal in file manager
// ---------------------------------------------------------------------------------------------------

/// What [`reveal_in_file_manager`] runs: program, arguments, and whether the last argument is passed raw
/// (Windows `explorer /select,"path"` needs its own quoting).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevealPlan {
    pub program: String,
    pub args: Vec<String>,
    pub raw_last_arg: bool,
}

/// Pure planning step (testable on any OS). `os` is `"linux"`, `"windows"` or `"macos"`.
///
/// * Linux: `xdg-open <dir>` — a file reveals its parent folder, a folder opens itself (no standard way to select);
/// * Windows: `explorer /select,"<path>"` (backslashes);
/// * macOS: `open -R <path>`.
pub fn reveal_plan(os: &str, path: &str, is_dir: bool) -> RevealPlan {
    match os {
        "windows" => {
            let native = path.replace('/', "\\");
            RevealPlan { program: "explorer".into(), args: vec![format!("/select,\"{native}\"")], raw_last_arg: true }
        }
        "macos" => RevealPlan { program: "open".into(), args: vec!["-R".into(), path.into()], raw_last_arg: false },
        _ => {
            let target = if is_dir {
                path.to_string()
            } else {
                Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).filter(|p| !p.is_empty()).unwrap_or_else(|| "/".into())
            };
            RevealPlan { program: "xdg-open".into(), args: vec![target], raw_last_arg: false }
        }
    }
}

/// Shows `path` in the system file manager.
pub fn reveal_in_file_manager(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    let meta = fs::metadata(p).map_err(|_| format!("“{path}” does not exist."))?;
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let plan = reveal_plan(os, path, meta.is_dir());
    let mut attempts = vec![plan.clone()];
    if os == "linux" {
        attempts.push(RevealPlan { program: "gio".into(), args: vec!["open".into(), plan.args[0].clone()], raw_last_arg: false });
    }
    let mut last_error = String::new();
    for attempt in attempts {
        match spawn_detached(&attempt) {
            Ok(()) => return Ok(()),
            Err(e) => last_error = format!("Could not run {}: {e}", attempt.program),
        }
    }
    Err(last_error)
}

fn spawn_detached(plan: &RevealPlan) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(&plan.program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if plan.raw_last_arg {
            for a in &plan.args {
                cmd.raw_arg(a);
            }
        } else {
            cmd.args(&plan.args);
        }
    }
    #[cfg(not(windows))]
    cmd.args(&plan.args);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    // Reap the child without blocking the caller (explorer.exe exits with 1 even on success).
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

// ---------------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gabetex-set-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: Vec<(String, String)> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k: &str| map.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone())
    }

    // ---- config dir ----------------------------------------------------------------------------

    #[test]
    fn config_dir_rules() {
        // The XDG rules use Unix absolute paths, which are not absolute on Windows.
        #[cfg(unix)]
        {
            let e = env_of(&[("XDG_CONFIG_HOME", "/x/cfg"), ("HOME", "/home/u")]);
            assert_eq!(linux_config_dir(&e), Some(PathBuf::from("/x/cfg/GabeTeX")));
        }
        let e = env_of(&[("HOME", "/home/u")]);
        assert_eq!(linux_config_dir(&e), Some(PathBuf::from("/home/u/.config/GabeTeX")));
        // A relative XDG_CONFIG_HOME is ignored (XDG spec).
        let e = env_of(&[("XDG_CONFIG_HOME", "rel"), ("HOME", "/home/u")]);
        assert_eq!(linux_config_dir(&e), Some(PathBuf::from("/home/u/.config/GabeTeX")));
        assert_eq!(linux_config_dir(&env_of(&[])), None);
        let e = env_of(&[("APPDATA", "C:\\Users\\u\\AppData\\Roaming")]);
        assert_eq!(windows_config_dir(&e), Some(PathBuf::from("C:\\Users\\u\\AppData\\Roaming").join("GabeTeX")));
        let e = env_of(&[("USERPROFILE", "C:\\Users\\u")]);
        assert_eq!(windows_config_dir(&e), Some(PathBuf::from("C:\\Users\\u").join("AppData").join("Roaming").join("GabeTeX")));
        assert!(config_dir().ends_with("GabeTeX"));
    }

    // ---- atomic writes -------------------------------------------------------------------------

    #[test]
    fn atomic_write_creates_and_replaces() {
        let dir = scratch("atomic");
        let f = dir.join("a.tex");
        write_atomic(&f, b"one").unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"one");
        write_atomic(&f, b"two two").unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"two two");
        let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(leftovers, vec!["a.tex".to_string()], "no temp files left: {leftovers:?}");
        assert!(write_atomic(&dir, b"x").is_err(), "a directory is refused");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_permissions_links_and_read_only() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = scratch("atomic-unix");
        let f = dir.join("script.tex");
        fs::write(&f, "old").unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&f, b"new").unwrap();
        assert_eq!(fs::metadata(&f).unwrap().permissions().mode() & 0o7777, 0o640);

        // Hard links stay one file.
        let link = dir.join("hard.tex");
        fs::hard_link(&f, &link).unwrap();
        let ino = fs::metadata(&f).unwrap().ino();
        write_atomic(&f, b"shared").unwrap();
        assert_eq!(fs::metadata(&f).unwrap().ino(), ino, "rewritten in place");
        assert_eq!(fs::read(&link).unwrap(), b"shared");

        // Symlinks are written through.
        let target = dir.join("real.tex");
        fs::write(&target, "t").unwrap();
        let sym = dir.join("sym.tex");
        std::os::unix::fs::symlink(&target, &sym).unwrap();
        write_atomic(&sym, b"via link").unwrap();
        assert!(fs::symlink_metadata(&sym).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"via link");

        // Read-only files are refused (unless running as root, which ignores the mode).
        let ro = dir.join("ro.tex");
        fs::write(&ro, "keep").unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o444)).unwrap();
        let is_root = fs::OpenOptions::new().write(true).open(&ro).is_ok();
        if !is_root {
            let err = write_atomic(&ro, b"nope").unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(fs::read(&ro).unwrap(), b"keep");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- encoding ------------------------------------------------------------------------------

    #[test]
    fn decode_utf8_variants() {
        assert_eq!(decode(b"").unwrap(), Loaded { text: String::new(), format: TextFormat::UTF8, invalid_byte_count: 0 });
        let l = decode("ação\r\nok".as_bytes()).unwrap();
        assert_eq!((l.text.as_str(), l.format), ("ação\r\nok", TextFormat::UTF8), "line endings untouched");
        let bom = decode(&[0xEF, 0xBB, 0xBF, b'a']).unwrap();
        assert_eq!(bom.text, "a");
        assert_eq!(bom.format, TextFormat { encoding: TextEncoding::Utf8, has_bom: true });
        assert_eq!(bom.format.name(), "utf-8-bom");
    }

    #[test]
    fn decode_latin1_legacy_files() {
        // "ação AÇÃO" in Latin-1: no well-formed multi-byte UTF-8 sequence -> Latin-1.
        let bytes = b"a\xe7\xe3o A\xc7\xc3O";
        let l = decode(bytes).unwrap();
        assert_eq!(l.text, "ação AÇÃO");
        assert_eq!(l.format, TextFormat::LATIN1);
        assert_eq!(l.format.name(), "latin-1");
        assert_eq!(encode(&l.text, l.format).unwrap(), bytes.to_vec(), "round trip");
    }

    #[test]
    fn decode_mostly_utf8_with_stray_bytes_stays_utf8() {
        let mut bytes = "caf\u{e9} cr\u{e8}me".as_bytes().to_vec();
        bytes.push(0xFF);
        let l = decode(&bytes).unwrap();
        assert_eq!(l.format, TextFormat::UTF8);
        assert_eq!(l.invalid_byte_count, 1);
        assert!(l.text.contains('\u{FFFD}') && l.text.starts_with("café"));
    }

    #[test]
    fn decode_utf16_and_binary() {
        let mut le = vec![0xFF, 0xFE];
        for u in "hi é".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        let l = decode(&le).unwrap();
        assert_eq!((l.text.as_str(), l.format.encoding, l.format.has_bom), ("hi é", TextEncoding::Utf16Le, true));
        assert_eq!(encode(&l.text, l.format).unwrap(), le);
        let mut be = vec![0xFE, 0xFF];
        for u in "x".encode_utf16() {
            be.extend_from_slice(&u.to_be_bytes());
        }
        let l = decode(&be).unwrap();
        assert_eq!((l.text.as_str(), l.format.encoding), ("x", TextEncoding::Utf16Be));
        assert!(decode(b"abc\0def").is_none(), "NUL means binary");
        assert!(decode_latin1(b"a\0b").is_none());
        assert_eq!(decode_latin1(b"\xe9").unwrap().text, "é");
        assert_eq!(scan_utf8("é€😀".as_bytes()), (3, 0));
        assert_eq!(scan_utf8(&[0xC0, 0x80, 0xE2, 0x82]), (0, 4));
    }

    #[test]
    fn encode_rejects_unrepresentable_latin1() {
        assert!(encode("€", TextFormat::LATIN1).unwrap_err().contains("ISO Latin 1"));
        assert_eq!(encode("é", TextFormat::LATIN1).unwrap(), vec![0xE9]);
        assert_eq!(encode("é", TextFormat { encoding: TextEncoding::Utf8, has_bom: true }).unwrap(), vec![0xEF, 0xBB, 0xBF, 0xC3, 0xA9]);
        for f in [TextFormat::UTF8, TextFormat::LATIN1] {
            assert_eq!(TextFormat::parse(f.name()), Some(f));
        }
        assert!(TextFormat::parse("utf-8-bom").unwrap().has_bom);
        assert_eq!(TextFormat::parse("nope"), None);
        assert_eq!(TextFormat::LATIN1.status_name(), "Latin-1");
    }

    #[test]
    fn read_write_text_file_round_trip() {
        let dir = scratch("textio");
        let f = dir.join("latin.tex");
        fs::write(&f, b"a\xe7\xe3o\r\n").unwrap();
        let l = read_text_file(&f, false).unwrap();
        assert_eq!(l.format, TextFormat::LATIN1);
        write_text_file(&f, &l.text, l.format).unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"a\xe7\xe3o\r\n");
        // Reopen as Latin-1 a file that would otherwise decode as UTF-8.
        fs::write(&f, "é".as_bytes()).unwrap();
        assert_eq!(read_text_file(&f, true).unwrap().text, "Ã©");
        fs::write(&f, b"bin\0ary").unwrap();
        assert!(read_text_file(&f, false).unwrap_err().contains("text file"));
        assert!(read_text_file(&dir.join("missing"), false).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- settings ------------------------------------------------------------------------------

    #[test]
    fn settings_round_trip_and_corruption() {
        let dir = scratch("settings");
        let store = ConfigStore::new(dir.join("cfg"));
        assert_eq!(store.load_settings(), serde_json::json!({}));
        let saved = store.save_settings(serde_json::json!({"theme": "dark", "fontSize": 14, "nested": {"a": [1, 2]}})).unwrap();
        assert_eq!(store.load_settings(), saved);
        assert!(store.save_settings(serde_json::json!([1])).is_err());
        assert_eq!(store.load_settings()["fontSize"], 14, "failed save leaves the old file");
        fs::write(store.settings_path(), "{ not json").unwrap();
        assert_eq!(store.load_settings(), serde_json::json!({}));
        assert!(store.dir().join("settings.json.corrupt").exists(), "corrupt file kept aside");
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- recents -------------------------------------------------------------------------------

    fn make_projects(dir: &Path, n: usize) -> Vec<String> {
        (0..n)
            .map(|i| {
                let p = dir.join(format!("proj{i:02}"));
                fs::create_dir_all(&p).unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect()
    }

    #[test]
    fn recents_are_most_recent_first_and_deduplicated() {
        let dir = scratch("recent");
        let store = ConfigStore::new(dir.join("cfg"));
        let ps = make_projects(&dir, 3);
        store.add_recent_project_at(&ps[0], None, 1000).unwrap();
        store.add_recent_project_at(&ps[1], Some("Second"), 2000).unwrap();
        let list = store.add_recent_project_at(&ps[0], None, 3000).unwrap();
        assert_eq!(list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["proj00", "Second"]);
        assert_eq!(list[0].last_opened, 3000);
        // A trailing slash and `.` spell the same project.
        let list = store.add_recent_project_at(&format!("{}/./", ps[1]), None, 4000).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].path, ps[1]);
        // Non-existing folders are hidden but remembered.
        fs::remove_dir_all(&ps[0]).unwrap();
        assert_eq!(store.recent_projects().len(), 1);
        let json: Value = serde_json::from_str(&fs::read_to_string(store.recents_path()).unwrap()).unwrap();
        assert_eq!(json["projects"].as_array().unwrap().len(), 2);
        assert!(json["projects"][0].get("lastOpened").is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cap_applies_to_unlabeled_projects_only() {
        let dir = scratch("cap");
        let store = ConfigStore::new(dir.join("cfg"));
        let ps = make_projects(&dir, 20);
        store.add_recent_project_at(&ps[0], None, 1).unwrap();
        store.set_project_label(&ps[0], Some("Red")).unwrap();
        let mut list = Vec::new();
        for (i, p) in ps.iter().enumerate().skip(1) {
            list = store.add_recent_project_at(p, None, 10 + i as i64).unwrap();
        }
        assert_eq!(list.len(), RECENT_LIMIT + 1, "15 unlabeled + the labeled one");
        assert_eq!(list.last().unwrap().path, ps[0], "labeled project is oldest and survived");
        assert_eq!(list.last().unwrap().label.as_deref(), Some("red"));
        assert_eq!(list[0].path, ps[19]);
        assert!(!list.iter().any(|p| p.path == ps[1]), "oldest unlabeled fell off");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trim_policy_is_pure() {
        let kept = trim_recents(vec![1, 2, 3, 4, 5, 6], 2, |n| *n == 5);
        assert_eq!(kept, vec![1, 2, 5]);
        assert_eq!(trim_recents(vec![1, 2], 0, |_| false), Vec::<i32>::new());
    }

    #[test]
    fn labels_survive_remove_and_clear_keeps_labeled() {
        let dir = scratch("labels");
        let store = ConfigStore::new(dir.join("cfg"));
        let ps = make_projects(&dir, 2);
        store.add_recent_project_at(&ps[0], None, 1).unwrap();
        store.add_recent_project_at(&ps[1], None, 2).unwrap();
        assert!(store.set_project_label(&ps[0], Some("chartreuse")).is_err());
        let list = store.set_project_label(&ps[0], Some("blue")).unwrap();
        assert_eq!(list.iter().find(|p| p.path == ps[0]).unwrap().label.as_deref(), Some("blue"));
        store.remove_recent_project(&ps[0]).unwrap();
        let list = store.add_recent_project_at(&ps[0], None, 3).unwrap();
        assert_eq!(list[0].label.as_deref(), Some("blue"), "label kept while the project was off the list");
        let list = store.clear_recent_projects().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].path, ps[0]);
        let list = store.set_project_label(&ps[0], None).unwrap();
        assert_eq!(list[0].label, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn label_follows_a_renamed_folder() {
        let dir = scratch("rename");
        let store = ConfigStore::new(dir.join("cfg"));
        let old = dir.join("Old");
        fs::create_dir_all(&old).unwrap();
        let old_s = old.to_string_lossy().into_owned();
        store.add_recent_project_at(&old_s, None, 1).unwrap();
        store.set_project_label(&old_s, Some("green")).unwrap();
        let new = dir.join("New");
        fs::rename(&old, &new).unwrap();
        let new_s = new.to_string_lossy().into_owned();
        let list = store.add_recent_project_at(&new_s, None, 2).unwrap();
        assert_eq!(list.len(), 1, "old path left the list");
        assert_eq!(list[0].path, new_s);
        assert_eq!(list[0].label.as_deref(), Some("green"));
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- file tree -----------------------------------------------------------------------------

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "x").unwrap();
    }

    fn names(nodes: &[FileNode]) -> Vec<String> {
        nodes.iter().map(|n| n.name.clone()).collect()
    }

    #[test]
    fn tree_skips_hidden_build_and_aux_files() {
        let dir = scratch("tree");
        for f in [
            "main.tex", "main.aux", "main.log", "main.synctex.gz", "main.run.xml", "main.bbl", "refs.bib", "fig.pdf", "notes.GZ",
            ".hidden", ".git/HEAD", "node_modules/x/y.js", "__pycache__/a.pyc", "chapters/b.tex", "chapters/a.tex", "chapters/.secret",
            "Zeta/z.tex", "alpha/q.tex", "file10.tex", "file2.tex", "Éclair.tex", "eclair2.tex",
        ] {
            touch(&dir.join(f));
        }
        let scan = scan_file_tree(dir.to_str().unwrap());
        assert!(!scan.truncated);
        // Folders first (Finder order, case-insensitive), then files.
        assert_eq!(
            names(&scan.nodes),
            ["alpha", "chapters", "Zeta", "Éclair.tex", "eclair2.tex", "fig.pdf", "file2.tex", "file10.tex", "main.tex", "refs.bib"]
        );
        let chapters = scan.nodes.iter().find(|n| n.name == "chapters").unwrap();
        assert!(chapters.is_dir);
        assert_eq!(names(chapters.children.as_ref().unwrap()), ["a.tex", "b.tex"]);
        let all: Vec<&str> = scan.files.iter().map(|s| s.as_str()).collect();
        assert!(all.iter().any(|f| Path::new(f).ends_with(Path::new("chapters").join("a.tex"))) && all.iter().any(|f| f.ends_with("refs.bib")));
        assert!(!all.iter().any(|f| f.contains(".git") || f.contains("node_modules") || f.ends_with(".aux") || f.ends_with(".gz")));
        assert_eq!(
            serde_json::to_string(&list_files(dir.to_str().unwrap())).unwrap(),
            serde_json::to_string(&scan.nodes).unwrap()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tree_order_is_finder_like() {
        let mut v = vec!["file10.tex", "File2.tex", "file1.tex", "Éa.tex", "eb.tex", "Zed", "alpha"];
        v.sort_by(|a, b| standard_compare(a, b));
        assert_eq!(v, ["alpha", "Éa.tex", "eb.tex", "file1.tex", "File2.tex", "file10.tex", "Zed"]);
        assert_eq!(standard_compare("a", "A"), Ordering::Less);
        assert_eq!(standard_compare("x2", "x10"), Ordering::Less);
    }

    #[test]
    fn tree_depth_and_truncation() {
        let dir = scratch("tree-deep");
        let mut deep = dir.clone();
        for i in 0..13 {
            deep = deep.join(format!("d{i}"));
        }
        touch(&deep.join("leaf.tex"));
        let scan = scan_file_tree(dir.to_str().unwrap());
        let mut node = &scan.nodes[0];
        let mut depth = 1;
        while let Some(c) = node.children.as_ref().and_then(|c| c.first()) {
            node = c;
            depth += 1;
        }
        assert_eq!(depth, MAX_TREE_DEPTH + 1, "folders below the depth limit show as empty folders");
        assert!(scan.files.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn tree_symlink_loops_show_as_empty_folders() {
        let dir = scratch("tree-link");
        touch(&dir.join("sub/a.tex"));
        std::os::unix::fs::symlink(&dir, dir.join("sub/up")).unwrap();
        std::os::unix::fs::symlink(dir.join("sub"), dir.join("alias")).unwrap();
        std::os::unix::fs::symlink(dir.join("missing"), dir.join("broken.tex")).unwrap();
        let scan = scan_file_tree(dir.to_str().unwrap());
        let sub = scan.nodes.iter().find(|n| n.name == "sub").unwrap();
        let up = sub.children.as_ref().unwrap().iter().find(|n| n.name == "up").unwrap();
        assert!(up.is_dir && up.children.as_ref().unwrap().is_empty());
        let alias = scan.nodes.iter().find(|n| n.name == "alias").unwrap();
        assert!(alias.is_dir);
        assert_eq!(names(alias.children.as_ref().unwrap()), ["up", "a.tex"]);
        assert!(scan.nodes.iter().any(|n| n.name == "broken.tex" && !n.is_dir), "dangling link is a file");
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- reveal --------------------------------------------------------------------------------

    #[test]
    fn reveal_plans() {
        let p = reveal_plan("linux", "/home/u/proj/main.tex", false);
        assert_eq!((p.program.as_str(), p.args.as_slice()), ("xdg-open", ["/home/u/proj".to_string()].as_slice()));
        let p = reveal_plan("linux", "/home/u/proj", true);
        assert_eq!(p.args, ["/home/u/proj"]);
        let p = reveal_plan("linux", "/main.tex", false);
        assert_eq!(p.args, ["/"]);
        let p = reveal_plan("windows", "C:/Users/u/My Proj/main.tex", false);
        assert_eq!(p.program, "explorer");
        assert_eq!(p.args, ["/select,\"C:\\Users\\u\\My Proj\\main.tex\""]);
        assert!(p.raw_last_arg);
        assert_eq!(reveal_plan("macos", "/a/b", false).args, ["-R", "/a/b"]);
        assert!(reveal_in_file_manager("/definitely/not/here").is_err());
    }
}

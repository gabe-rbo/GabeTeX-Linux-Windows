//! TeX distribution detection and the child-process runner.
//!
//! Port of `TeXDistribution.swift` (locator, `inspect`, year/name parsing, biber health check) and
//! `ProcessRunner.swift` (threads instead of Swift concurrency):
//!
//! * the child runs in its own process group (Unix) so [`run`] can terminate the whole tree
//!   (latexmk -> pdflatex/bibtex/biber) with SIGTERM, then SIGKILL after a grace period; on Windows the
//!   tree is killed with `taskkill /T /F` and no console window is created (`CREATE_NO_WINDOW`);
//! * stdout/stderr are drained concurrently (no pipe deadlocks), stdin is written from a thread;
//! * every live child is listed in a registry so [`terminate_all`] can kill them synchronously at app quit
//!   (tlmgr installs register with `kill_on_terminate_all = false`).
//!
//! Linux: custom dir, `/usr/local/texlive/<year>/bin/<arch>`, `/opt/texlive`, `~/texlive`, TinyTeX
//! (`~/.TinyTeX`), then PATH (+ login shell PATH), `/usr/local/bin`, `/usr/bin`.
//! Windows: custom dir, `C:\texlive\<year>\bin\{windows,win64,win32}`, TinyTeX in `%APPDATA%`, MiKTeX
//! (`%LOCALAPPDATA%\Programs\MiKTeX\miktex\bin\x64`, `%ProgramFiles%\MiKTeX\miktex\bin\x64`), then PATH.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------------------------------
// Tools and distribution
// ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tool {
    Pdflatex,
    Xelatex,
    Lualatex,
    Latexmk,
    Bibtex,
    Biber,
    Makeindex,
    Chktex,
    Synctex,
    Tlmgr,
    Kpsewhich,
    /// MiKTeX package manager (`mpm`).
    Mpm,
    /// MiKTeX command line front end (`miktex packages install ...`).
    Miktex,
}

impl Tool {
    pub const ALL: [Tool; 13] = [
        Tool::Pdflatex,
        Tool::Xelatex,
        Tool::Lualatex,
        Tool::Latexmk,
        Tool::Bibtex,
        Tool::Biber,
        Tool::Makeindex,
        Tool::Chktex,
        Tool::Synctex,
        Tool::Tlmgr,
        Tool::Kpsewhich,
        Tool::Mpm,
        Tool::Miktex,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Pdflatex => "pdflatex",
            Tool::Xelatex => "xelatex",
            Tool::Lualatex => "lualatex",
            Tool::Latexmk => "latexmk",
            Tool::Bibtex => "bibtex",
            Tool::Biber => "biber",
            Tool::Makeindex => "makeindex",
            Tool::Chktex => "chktex",
            Tool::Synctex => "synctex",
            Tool::Tlmgr => "tlmgr",
            Tool::Kpsewhich => "kpsewhich",
            Tool::Mpm => "mpm",
            Tool::Miktex => "miktex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    TeXLive,
    MiKTeX,
}

/// A detected TeX installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Distribution {
    /// "TeX Live 2026", "TinyTeX 2026", "MiKTeX 24.1", ...
    pub name: String,
    pub year: Option<u32>,
    pub kind: Kind,
    /// Directory containing the binaries.
    pub bin_dir: String,
    /// TEXMFROOT, if known.
    pub texmf_root: Option<String>,
    /// True when the package manager can modify the installation without administrator rights.
    pub user_writable: bool,
    /// Absolute paths of the tools that were found.
    pub tools: BTreeMap<Tool, String>,
    /// First line of `pdftex --version` / `tlmgr --version`.
    pub version_description: Option<String>,
}

impl Distribution {
    pub fn tool(&self, tool: Tool) -> Option<&str> {
        self.tools.get(&tool).map(|s| s.as_str())
    }

    pub fn has(&self, tool: Tool) -> bool {
        self.tools.contains_key(&tool)
    }

    pub fn is_miktex(&self) -> bool {
        self.kind == Kind::MiKTeX
    }

    /// Environment overrides for child processes: `PATH` starts with `bin_dir` (GUI apps do not inherit the
    /// login shell PATH), plus `max_print_line`/`error_line`/`half_error_line` so logs do not wrap.
    pub fn process_environment(&self) -> Vec<(String, String)> {
        let mut env = vec![("PATH".to_string(), path_with_prefix(&self.bin_dir))];
        env.push(("max_print_line".into(), "10000".into()));
        env.push(("error_line".into(), "254".into()));
        env.push(("half_error_line".into(), "238".into()));
        env
    }

    /// A package manager is available (tlmgr for TeX Live; mpm / miktex for MiKTeX).
    pub fn has_package_manager(&self) -> bool {
        match self.kind {
            Kind::TeXLive => self.has(Tool::Tlmgr),
            Kind::MiKTeX => self.has(Tool::Mpm) || self.has(Tool::Miktex),
        }
    }

    /// IPC shape (`detect_tex`). `tlmgr` is true when a package manager exists (MiKTeX: mpm / miktex).
    pub fn to_ipc(&self) -> crate::commands::TeXDistribution {
        crate::commands::TeXDistribution {
            name: self.name.clone(),
            bin_dir: self.bin_dir.clone(),
            latexmk: self.has(Tool::Latexmk),
            tlmgr: self.has_package_manager(),
            synctex: self.has(Tool::Synctex),
        }
    }

    /// Explains why an installed biber does not run (cached per binary); `None` when it works, is absent,
    /// or the check was inconclusive. Not run for MiKTeX (a missing biber would pop up its installer).
    pub fn biber_problem(&self) -> Option<String> {
        if self.is_miktex() {
            return None;
        }
        biber_health(self.tool(Tool::Biber)?, &self.process_environment())
    }
}

pub const PATH_SEPARATOR: char = if cfg!(windows) { ';' } else { ':' };

/// `<dir><sep><current PATH>`.
pub fn path_with_prefix(dir: &str) -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    if current.is_empty() {
        dir.to_string()
    } else {
        format!("{dir}{PATH_SEPARATOR}{current}")
    }
}

// ---------------------------------------------------------------------------------------------------
// Locator
// ---------------------------------------------------------------------------------------------------

/// Finds the best installation: `custom_bin_dir` if valid, else the static candidates, then PATH.
/// Returns `None` when no directory contains a working `kpsewhich` plus a LaTeX engine or latexmk.
pub fn locate(custom_bin_dir: Option<&str>) -> Option<Distribution> {
    if let Some(dir) = custom_bin_dir.filter(|d| !d.is_empty()) {
        if let Some(d) = inspect(Path::new(dir)) {
            return Some(d);
        }
    }
    let mut tried = HashSet::new();
    for dir in candidate_directories(false) {
        if !tried.insert(dedup_key(&dir)) {
            continue;
        }
        if let Some(d) = inspect(&dir) {
            return Some(d);
        }
    }
    for dir in login_shell_path_directories() {
        if !tried.insert(dedup_key(&dir)) {
            continue;
        }
        if let Some(d) = inspect(&dir) {
            return Some(d);
        }
    }
    None
}

/// Cached `locate(None)` for the IPC layer (detection spawns a few processes). Use [`invalidate_cache`]
/// after the user changes the TeX location or installs TeX.
pub fn detect_cached() -> Option<Distribution> {
    let mut guard = detect_cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cached) = guard.as_ref() {
        return cached.clone();
    }
    let found = locate(None);
    *guard = Some(found.clone());
    found
}

/// Forgets the result of [`detect_cached`].
pub fn invalidate_cache() {
    *detect_cache().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn detect_cache() -> &'static Mutex<Option<Option<Distribution>>> {
    static CACHE: OnceLock<Mutex<Option<Option<Distribution>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// All installations found (for a settings picker), best first, one entry per TEXMFROOT.
pub fn candidates() -> Vec<Distribution> {
    let mut result = Vec::new();
    let mut seen_roots = HashSet::new();
    let mut seen_dirs = HashSet::new();
    let mut dirs = candidate_directories(false);
    dirs.extend(login_shell_path_directories());
    for dir in dirs {
        let resolved = resolve(&dir);
        if !seen_dirs.insert(resolved.clone()) {
            continue;
        }
        let Some(d) = inspect(&dir) else { continue };
        let key = d.texmf_root.as_deref().map(resolve_str).unwrap_or(resolved);
        if seen_roots.insert(key) {
            result.push(d);
        }
    }
    result
}

fn dedup_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/').to_string();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

fn resolve(p: &Path) -> String {
    std::fs::canonicalize(p)
        .map(|c| strip_verbatim(&c.to_string_lossy()))
        .unwrap_or_else(|_| p.to_string_lossy().into_owned())
}

fn resolve_str(p: &str) -> String {
    resolve(Path::new(p))
}

/// `\\?\C:\x` -> `C:\x`.
pub fn strip_verbatim(p: &str) -> String {
    if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = p.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        p.to_string()
    }
}

/// Executable file name candidates for a tool in `dir` (`.exe`, `.bat`, `.cmd` on Windows).
pub fn find_tool(dir: &Path, name: &str) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.bat"), format!("{name}.cmd")]
    } else {
        vec![name.to_string()]
    };
    names.into_iter().map(|n| dir.join(n)).find(|p| is_executable_file(p))
}

pub fn is_executable_file(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else { return false };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Directories probed (in priority order). With `include_path` the process PATH is appended too.
pub fn candidate_directories(include_path: bool) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        // C:\texlive\<year>\bin\{windows,win64,win32}, newest year first.
        for root in ["C:\\texlive", "D:\\texlive"] {
            for year in year_directories(Path::new(root)) {
                for arch in ["windows", "win64", "win32"] {
                    dirs.push(Path::new(root).join(year.to_string()).join("bin").join(arch));
                }
            }
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            for arch in ["windows", "win32", "win64"] {
                dirs.push(Path::new(&appdata).join("TinyTeX").join("bin").join(arch));
            }
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            for year in year_directories(&Path::new(&profile).join("texlive")) {
                for arch in ["windows", "win64", "win32"] {
                    dirs.push(Path::new(&profile).join("texlive").join(year.to_string()).join("bin").join(arch));
                }
            }
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(Path::new(&local).join("Programs").join("MiKTeX").join("miktex").join("bin").join("x64"));
        }
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(pf) = std::env::var_os(var) {
                dirs.push(Path::new(&pf).join("MiKTeX").join("miktex").join("bin").join("x64"));
                dirs.push(Path::new(&pf).join("MiKTeX").join("miktex").join("bin"));
                dirs.push(Path::new(&pf).join("MiKTeX 2.9").join("miktex").join("bin").join("x64"));
            }
        }
    }
    #[cfg(not(windows))]
    {
        let mut roots: Vec<PathBuf> = vec![PathBuf::from("/usr/local/texlive"), PathBuf::from("/opt/texlive")];
        let home = home_dir();
        if let Some(h) = &home {
            roots.push(h.join("texlive"));
        }
        for root in roots {
            for year in year_directories(&root) {
                dirs.extend(arch_directories(&root.join(year.to_string()).join("bin")));
            }
        }
        if let Some(h) = &home {
            dirs.extend(arch_directories(&h.join(".TinyTeX").join("bin")));
            dirs.extend(arch_directories(&h.join("Library").join("TinyTeX").join("bin")));
        }
    }
    if include_path {
        dirs.extend(path_directories());
    }
    #[cfg(not(windows))]
    {
        for d in ["/Library/TeX/texbin", "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
            dirs.push(PathBuf::from(d));
        }
    }
    dirs
}

/// Directories of the process PATH.
pub fn path_directories() -> Vec<PathBuf> {
    let Some(path) = std::env::var_os("PATH") else { return Vec::new() };
    std::env::split_paths(&path).filter(|p| !p.as_os_str().is_empty()).collect()
}

pub fn home_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// Numeric sub-directories of `root` (4-digit years), newest first.
fn year_directories(root: &Path) -> Vec<u32> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut years: Vec<u32> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()))
        .collect();
    years.sort_unstable_by(|a, b| b.cmp(a));
    years
}

/// Architecture directories below a `bin` root (`x86_64-linux` first, then the rest alphabetically).
#[cfg_attr(windows, allow(dead_code))]
fn arch_directories(bin_root: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(bin_root) else { return Vec::new() };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .filter(|n| !n.starts_with('.'))
        .collect();
    let preferred = if cfg!(target_arch = "aarch64") { "aarch64-linux" } else { "x86_64-linux" };
    names.sort_by(|a, b| (a != preferred, a.as_str()).cmp(&(b != preferred, b.as_str())));
    names.into_iter().map(|n| bin_root.join(n)).collect()
}

/// Directories of the login shell's PATH (`$SHELL -lc 'printf %s "$PATH"'`, 3 s timeout). Unix only: GUI
/// apps started from a desktop launcher do not inherit the shell's PATH.
pub fn login_shell_path_directories() -> Vec<PathBuf> {
    #[allow(unused_mut)]
    let mut dirs = path_directories();
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL").ok().filter(|s| Path::new(s).exists()).unwrap_or_else(|| "/bin/sh".into());
        let opts = RunOptions { timeout: Some(Duration::from_secs(3)), ..Default::default() };
        if let Ok(out) = run(&shell, &["-lc", "printf %s \"$PATH\""], &opts) {
            if out.succeeded() {
                dirs.extend(parse_login_shell_path(&out.stdout_text()));
            }
        }
    }
    dirs
}

/// The PATH is the last line of the output (profile scripts may print before it). Absolute entries only,
/// de-duplicated.
pub fn parse_login_shell_path(output: &str) -> Vec<PathBuf> {
    let last = output.split('\n').rfind(|l| !l.is_empty()).unwrap_or("");
    let mut seen = HashSet::new();
    last.split(':')
        .filter(|p| p.starts_with('/') && seen.insert(p.to_string()))
        .map(PathBuf::from)
        .collect()
}

// ---------------------------------------------------------------------------------------------------
// Inspect
// ---------------------------------------------------------------------------------------------------

/// Validates one bin directory and describes the installation it belongs to. `None` if it is not a TeX
/// installation (no `kpsewhich`, `kpsewhich --var-value TEXMFROOT` fails, or no engine/latexmk).
pub fn inspect(bin_dir: &Path) -> Option<Distribution> {
    let bin = PathBuf::from(lexical_native(bin_dir));
    let mut tools: BTreeMap<Tool, String> = BTreeMap::new();
    for tool in Tool::ALL {
        if let Some(p) = find_tool(&bin, tool.name()) {
            tools.insert(tool, p.to_string_lossy().into_owned());
        }
    }
    let kpsewhich = tools.get(&Tool::Kpsewhich)?.clone();
    if !(tools.contains_key(&Tool::Latexmk)
        || tools.contains_key(&Tool::Pdflatex)
        || tools.contains_key(&Tool::Xelatex)
        || tools.contains_key(&Tool::Lualatex))
    {
        return None;
    }
    let bin_str = bin.to_string_lossy().into_owned();
    let miktex = looks_like_miktex(&bin_str, &tools);
    let env = vec![("PATH".to_string(), path_with_prefix(&bin_str))];
    let opts = RunOptions { env: env.clone(), timeout: Some(Duration::from_secs(5)), ..Default::default() };
    let root_out = run(&kpsewhich, &["--var-value", "TEXMFROOT"], &opts).ok()?;
    if !root_out.succeeded() {
        return None;
    }
    let root_path = root_out.stdout_text().trim().to_string();
    let texmf_root: Option<String> = if !root_path.is_empty() && Path::new(&root_path).is_dir() {
        Some(root_path)
    } else if miktex {
        None // MiKTeX does not define TEXMFROOT
    } else {
        return None;
    };

    // Version: pdftex is fast; the package manager only as a fallback.
    let mut version_line: Option<String> = None;
    if let Some(pdftex) = find_tool(&bin, "pdftex") {
        let o = RunOptions { env: env.clone(), timeout: Some(Duration::from_secs(5)), ..Default::default() };
        if let Ok(out) = run(&pdftex.to_string_lossy(), &["--version"], &o) {
            if out.succeeded() {
                version_line = first_line(&out.stdout_text());
            }
        }
    }
    if version_line.is_none() {
        if let Some(tlmgr) = tools.get(&Tool::Tlmgr) {
            let o = RunOptions { env: env.clone(), timeout: Some(Duration::from_secs(10)), ..Default::default() };
            if let Ok(out) = run(tlmgr, &["--version"], &o) {
                if out.succeeded() {
                    let text = out.stdout_text();
                    version_line = text
                        .lines()
                        .find(|l| l.contains("version"))
                        .map(|l| l.trim().to_string())
                        .or_else(|| first_line(&text));
                }
            }
        }
    }
    let year = if miktex { None } else { parse_year(version_line.as_deref(), texmf_root.as_deref()) };
    let user_writable = if miktex {
        miktex_user_writable(&bin_str)
    } else {
        texmf_root
            .as_deref()
            .map(|r| is_dir_writable(&Path::new(r).join("tlpkg")))
            .unwrap_or(false)
    };
    let miktex_version = if miktex { version_line.as_deref().and_then(parse_miktex_version) } else { None };
    let name = distribution_name(&bin_str, texmf_root.as_deref(), year, miktex, miktex_version.as_deref());
    Some(Distribution {
        name,
        year,
        kind: if miktex { Kind::MiKTeX } else { Kind::TeXLive },
        bin_dir: bin_str,
        texmf_root,
        user_writable,
        tools,
        version_description: version_line,
    })
}

fn lexical_native(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if s.len() > 1 {
        s.trim_end_matches(['/', '\\']).to_string()
    } else {
        s
    }
}

fn looks_like_miktex(bin: &str, tools: &BTreeMap<Tool, String>) -> bool {
    bin.to_lowercase().contains("miktex") || tools.contains_key(&Tool::Mpm) || tools.contains_key(&Tool::Miktex)
}

/// MiKTeX installed "for the current user" lives below %LOCALAPPDATA%; "for all users" below Program Files.
fn miktex_user_writable(bin: &str) -> bool {
    let b = bin.replace('\\', "/").to_lowercase();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let l = local.to_string_lossy().replace('\\', "/").to_lowercase();
        if !l.is_empty() && b.starts_with(&l) {
            return true;
        }
    }
    !b.contains("/program files")
}

/// Tries to create (and remove) a file in `dir`.
pub fn is_dir_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".gabetex-write-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn first_line(text: &str) -> Option<String> {
    text.lines().map(|l| l.trim()).find(|l| !l.is_empty()).map(|l| l.to_string())
}

/// Year from "pdfTeX 3.14... (TeX Live 2026)", "TeX Live (https://tug.org/texlive) version 2026", or a
/// TEXMFROOT ending in the year.
pub fn parse_year(version_line: Option<&str>, texmf_root: Option<&str>) -> Option<u32> {
    if let Some(line) = version_line {
        for marker in ["TeX Live ", "version "] {
            if let Some(y) = year_after(line, marker) {
                return Some(y);
            }
        }
    }
    let root = texmf_root?.trim_end_matches(['/', '\\']);
    let last = root.rsplit(['/', '\\']).next()?;
    if last.len() == 4 && last.bytes().all(|b| b.is_ascii_digit()) {
        return last.parse().ok();
    }
    None
}

fn year_after(line: &str, marker: &str) -> Option<u32> {
    let mut from = 0;
    while let Some(i) = line[from..].find(marker) {
        let start = from + i + marker.len();
        let digits = line.as_bytes().get(start..start + 4);
        if let Some(d) = digits {
            if d.iter().all(|b| b.is_ascii_digit()) {
                return line[start..start + 4].parse().ok();
            }
        }
        from += i + 1;
    }
    None
}

/// "24.1" from "MiKTeX-pdfTeX 4.1 (MiKTeX 24.1)".
pub fn parse_miktex_version(line: &str) -> Option<String> {
    let i = line.find("(MiKTeX ")? + "(MiKTeX ".len();
    let rest = &line[i..];
    let end = rest.find(')')?;
    let v = rest[..end].trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// "TinyTeX 2026", "MiKTeX 24.1", "MacTeX 2026", "TeX Live (Homebrew)", else "TeX Live 2026".
pub fn distribution_name(
    bin_dir: &str,
    texmf_root: Option<&str>,
    year: Option<u32>,
    miktex: bool,
    miktex_version: Option<&str>,
) -> String {
    let bin = bin_dir.replace('\\', "/");
    let root = texmf_root.unwrap_or("").replace('\\', "/");
    let suffix = year.map(|y| format!(" {y}")).unwrap_or_default();
    if miktex {
        return match miktex_version {
            Some(v) => format!("MiKTeX {v}"),
            None => "MiKTeX".to_string(),
        };
    }
    if bin.starts_with("/Library/TeX/") {
        return format!("MacTeX{suffix}");
    }
    if bin.to_lowercase().contains("tinytex") || root.to_lowercase().contains("tinytex") {
        return format!("TinyTeX{suffix}");
    }
    if bin.starts_with("/opt/homebrew") || root.starts_with("/opt/homebrew") || root.contains("/Cellar/") {
        return "TeX Live (Homebrew)".to_string();
    }
    format!("TeX Live{suffix}")
}

// ---------------------------------------------------------------------------------------------------
// Biber health
// ---------------------------------------------------------------------------------------------------

fn health_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Runs `biber --version` (cached per binary path + modification time). Returns a user-facing problem
/// description when biber is installed but cannot run, `None` when it works or the check was
/// inconclusive (timeout: the first run of a healthy biber unpacks itself and can be slow).
pub fn biber_health(biber: &str, env: &[(String, String)]) -> Option<String> {
    let resolved = resolve_str(biber);
    let mtime = std::fs::metadata(&resolved)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let key = format!("{resolved}#{mtime}");
    if let Some(cached) = health_cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return cached.clone();
    }
    let opts = RunOptions { env: env.to_vec(), timeout: Some(Duration::from_secs(20)), ..Default::default() };
    let store = |v: Option<String>| {
        health_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), v.clone());
        v
    };
    let out = match run(biber, &["--version"], &opts) {
        Ok(o) => o,
        Err(_) => return store(Some("biber could not be started.".to_string())),
    };
    if out.timed_out || out.cancelled {
        return None;
    }
    let text = out.combined_text();
    if out.succeeded() && text.to_lowercase().contains("biber version") {
        return store(None);
    }
    let lines: Vec<&str> = text.lines().collect();
    let reason = lines
        .iter()
        .rev()
        .find(|l| l.contains("biber") && !l.contains("usage"))
        .map(|l| l.trim().to_string())
        .or_else(|| lines.first().map(|l| l.trim().to_string()))
        .unwrap_or_default();
    store(Some(if reason.is_empty() {
        "biber is installed but does not run.".to_string()
    } else {
        format!("biber is installed but does not run ({reason}).")
    }))
}

// ---------------------------------------------------------------------------------------------------
// Process runner
// ---------------------------------------------------------------------------------------------------

/// Result of a finished child process.
#[derive(Debug, Clone, Default)]
pub struct ProcessOutput {
    /// Exit code; `None` when the process was killed by a signal.
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// The process was terminated because its timeout elapsed.
    pub timed_out: bool,
    /// The process was terminated because the cancel callback returned true or `terminate_all` ran.
    pub cancelled: bool,
}

impl ProcessOutput {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && !self.cancelled
    }

    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// stdout followed by stderr.
    pub fn combined_text(&self) -> String {
        let out = self.stdout_text();
        let err = self.stderr_text();
        if err.is_empty() {
            out
        } else if out.is_empty() {
            err
        } else if out.ends_with('\n') {
            out + &err
        } else {
            format!("{out}\n{err}")
        }
    }
}

/// Options of [`run`]. `env` entries override the inherited environment.
pub struct RunOptions<'a> {
    pub env: Vec<(String, String)>,
    pub cwd: Option<&'a str>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Option<Duration>,
    /// Polled every ~25 ms; when it returns true the process tree is terminated (`cancelled`).
    pub cancel: Option<&'a dyn Fn() -> bool>,
    /// Register the process for [`terminate_all`] (false for tlmgr installs: never interrupt half-way).
    pub kill_on_terminate_all: bool,
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        RunOptions { env: Vec::new(), cwd: None, stdin: None, timeout: None, cancel: None, kill_on_terminate_all: true }
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `Command` that never shows a console window on Windows.
pub fn command<S: AsRef<OsStr>>(exe: S) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(unix)]
extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
const SIGKILL: i32 = 9;

struct Registered {
    killable: bool,
    killed: bool,
}

fn registry() -> &'static Mutex<HashMap<u32, Registered>> {
    static REG: OnceLock<Mutex<HashMap<u32, Registered>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Synchronously kills every registered child process tree still running (their `run` calls return with
/// `cancelled == true`). Call when the app quits. Returns how many were killed.
pub fn terminate_all() -> usize {
    let pids: Vec<u32> = {
        let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        reg.iter_mut()
            .filter(|(_, r)| r.killable && !r.killed)
            .map(|(pid, r)| {
                r.killed = true;
                *pid
            })
            .collect()
    };
    for pid in &pids {
        kill_tree(*pid, true);
    }
    pids.len()
}

/// Number of live registered children (for checks).
pub fn registered_count() -> usize {
    registry().lock().unwrap_or_else(|e| e.into_inner()).len()
}

fn kill_tree(pid: u32, force: bool) {
    #[cfg(unix)]
    {
        let sig = if force { SIGKILL } else { SIGTERM };
        // SAFETY: plain syscall; the child leads its own process group (pgid == pid).
        unsafe {
            kill(-(pid as i32), sig);
            kill(pid as i32, sig);
        }
    }
    #[cfg(windows)]
    {
        let _ = force;
        let _ = command("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Spawns `exe` and waits for it to finish. The call blocks the calling thread (run it from a worker).
pub fn run<S: AsRef<OsStr>>(exe: &str, args: &[S], o: &RunOptions) -> Result<ProcessOutput, String> {
    let mut cmd = command(exe);
    cmd.args(args);
    for (k, v) in &o.env {
        cmd.env(k, v);
    }
    if let Some(cwd) = o.cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdin(if o.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| format!("Could not run {exe}: {e}"))?;
    let pid = child.id();
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(pid, Registered { killable: o.kill_on_terminate_all, killed: false });

    if let (Some(data), Some(mut stdin)) = (o.stdin.clone(), child.stdin.take()) {
        thread::spawn(move || {
            let _ = stdin.write_all(&data); // a closed pipe (child exited early) is fine
        });
    }
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let out_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let err_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    for (stream, buf) in [
        (child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), out_buf.clone()),
        (child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>), err_buf.clone()),
    ] {
        let tx = done_tx.clone();
        match stream {
            Some(mut s) => {
                thread::spawn(move || {
                    let mut chunk = vec![0u8; 64 * 1024];
                    loop {
                        match s.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(n) => buf.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&chunk[..n]),
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(_) => break,
                        }
                    }
                    let _ = tx.send(());
                });
            }
            None => {
                let _ = tx.send(());
            }
        }
    }
    drop(done_tx);

    let (wait_tx, wait_rx) = mpsc::channel();
    thread::spawn(move || {
        let status = child.wait();
        let _ = wait_tx.send(status);
    });

    let started = Instant::now();
    let grace = Duration::from_millis(1500);
    let mut terminate_requested: Option<Instant> = None;
    let mut force_sent = false;
    let (mut timed_out, mut cancelled) = (false, false);
    let status = loop {
        match wait_rx.recv_timeout(Duration::from_millis(25)) {
            Ok(status) => break status.ok(),
            Err(mpsc::RecvTimeoutError::Disconnected) => break None,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        match terminate_requested {
            None => {
                if o.cancel.is_some_and(|c| c()) {
                    cancelled = true;
                } else if o.timeout.is_some_and(|t| started.elapsed() >= t) {
                    timed_out = true;
                }
                if cancelled || timed_out {
                    terminate_requested = Some(Instant::now());
                    kill_tree(pid, cfg!(windows));
                }
            }
            Some(at) => {
                if !force_sent && at.elapsed() >= grace {
                    force_sent = true;
                    kill_tree(pid, true);
                }
            }
        }
    };
    // Stray group members of a terminated tree must not outlive the leader.
    if terminate_requested.is_some() {
        kill_tree(pid, true);
    }
    let killed_by_quit = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&pid)
        .map(|r| r.killed)
        .unwrap_or(false);
    if killed_by_quit {
        cancelled = true;
    }
    // A grandchild that escaped the group could keep the pipes open: stop waiting for EOF after 3 s.
    let deadline = Instant::now() + Duration::from_secs(3);
    for _ in 0..2 {
        let left = deadline.saturating_duration_since(Instant::now());
        if done_rx.recv_timeout(left).is_err() {
            break;
        }
    }
    let stdout = std::mem::take(&mut *out_buf.lock().unwrap_or_else(|e| e.into_inner()));
    let stderr = std::mem::take(&mut *err_buf.lock().unwrap_or_else(|e| e.into_inner()));
    Ok(ProcessOutput {
        exit_code: status.and_then(|s| s.code()).or(if status.is_none() { Some(-1) } else { None }),
        stdout,
        stderr,
        timed_out,
        cancelled,
    })
}

// ---------------------------------------------------------------------------------------------------
// SHA-256 (project identifier of the shadow workspace; no crate available)
// ---------------------------------------------------------------------------------------------------

pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
        0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
        0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
        0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] =
        [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}

// ---------------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(hex(&sha256(long)), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    }

    #[test]
    fn year_and_name_parsing() {
        assert_eq!(parse_year(Some("pdfTeX 3.141592653-2.6-1.40.28 (TeX Live 2026)"), None), Some(2026));
        assert_eq!(parse_year(Some("TeX Live (https://tug.org/texlive) version 2025"), None), Some(2025));
        assert_eq!(parse_year(Some("pdfTeX 3.14 (TeX Live 2022/Debian)"), None), Some(2022));
        assert_eq!(parse_year(Some("garbage"), Some("/usr/local/texlive/2024")), Some(2024));
        assert_eq!(parse_year(Some("garbage"), Some("/usr/share/texlive")), None);
        assert_eq!(parse_year(Some("TeX Live x TeX Live 2023"), None), Some(2023));
        assert_eq!(parse_year(None, Some(r"C:\texlive\2025\")), Some(2025));
        assert_eq!(
            distribution_name("/usr/local/texlive/2026/bin/x86_64-linux", Some("/usr/local/texlive/2026"), Some(2026), false, None),
            "TeX Live 2026"
        );
        assert_eq!(distribution_name("/home/g/.TinyTeX/bin/x86_64-linux", Some("/home/g/.TinyTeX"), Some(2026), false, None), "TinyTeX 2026");
        assert_eq!(distribution_name(r"C:\Users\g\AppData\Roaming\TinyTeX\bin\windows", None, Some(2025), false, None), "TinyTeX 2025");
        assert_eq!(distribution_name("/Library/TeX/texbin", None, Some(2026), false, None), "MacTeX 2026");
        assert_eq!(distribution_name("/opt/homebrew/bin", Some("/opt/homebrew/Cellar/x"), None, false, None), "TeX Live (Homebrew)");
        assert_eq!(distribution_name(r"C:\miktex\bin\x64", None, None, true, Some("24.1")), "MiKTeX 24.1");
        assert_eq!(distribution_name(r"C:\miktex\bin\x64", None, None, true, None), "MiKTeX");
        assert_eq!(parse_miktex_version("MiKTeX-pdfTeX 4.17 (MiKTeX 24.1)"), Some("24.1".into()));
        assert_eq!(parse_miktex_version("pdfTeX 3.14"), None);
    }

    #[test]
    fn login_shell_path_parsing() {
        let out = "Welcome!\nmotd line\n/usr/bin:/usr/local/bin:relative:/usr/bin:/opt/x/bin";
        let dirs = parse_login_shell_path(out);
        assert_eq!(dirs, vec![PathBuf::from("/usr/bin"), PathBuf::from("/usr/local/bin"), PathBuf::from("/opt/x/bin")]);
        assert!(parse_login_shell_path("").is_empty());
        assert_eq!(parse_login_shell_path("/a:/b\n"), vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn strip_verbatim_prefix() {
        assert_eq!(strip_verbatim(r"\\?\C:\x\y"), r"C:\x\y");
        assert_eq!(strip_verbatim(r"\\?\UNC\srv\share"), r"\\srv\share");
        assert_eq!(strip_verbatim("/usr/bin"), "/usr/bin");
    }

    #[test]
    fn path_prefix_and_environment() {
        let d = Distribution {
            name: "TeX Live 2026".into(),
            year: Some(2026),
            kind: Kind::TeXLive,
            bin_dir: "/x/bin".into(),
            texmf_root: None,
            user_writable: true,
            tools: BTreeMap::from([(Tool::Latexmk, "/x/bin/latexmk".into()), (Tool::Tlmgr, "/x/bin/tlmgr".into())]),
            version_description: None,
        };
        let env = d.process_environment();
        let path = &env.iter().find(|(k, _)| k == "PATH").unwrap().1;
        assert!(path.starts_with(&format!("/x/bin{PATH_SEPARATOR}")) || path == "/x/bin");
        assert!(env.contains(&("max_print_line".into(), "10000".into())));
        let ipc = d.to_ipc();
        assert!(ipc.latexmk && ipc.tlmgr && !ipc.synctex);
        assert_eq!(ipc.bin_dir, "/x/bin");
        let mut m = d.clone();
        m.kind = Kind::MiKTeX;
        m.tools = BTreeMap::from([(Tool::Mpm, "mpm".into())]);
        assert!(m.to_ipc().tlmgr, "MiKTeX: mpm counts as the package manager");
    }

    #[cfg(unix)]
    mod unix {
        use super::super::*;

        #[test]
        fn runner_stdin_output_exit_codes() {
            let out = run("/bin/sh", &["-c", "cat; echo err >&2; exit 3"], &RunOptions { stdin: Some(b"hello".to_vec()), ..Default::default() }).unwrap();
            assert_eq!(out.stdout_text(), "hello");
            assert_eq!(out.stderr_text(), "err\n");
            assert_eq!(out.exit_code, Some(3));
            assert!(!out.succeeded());
            assert_eq!(out.combined_text(), "hello\nerr\n");
            let ok = run("/bin/echo", &["ok"], &RunOptions::default()).unwrap();
            assert!(ok.succeeded());
            assert_eq!(ok.stdout_text(), "ok\n");
            assert!(run("/nonexistent/binary", &["x"], &RunOptions::default()).is_err());
        }

        #[test]
        fn runner_large_output_does_not_deadlock() {
            // 3 MB on stdout and 3 MB on stderr.
            let out = run("/bin/sh", &["-c", "head -c 3000000 /dev/zero | tr '\\0' a; head -c 3000000 /dev/zero | tr '\\0' b >&2"], &RunOptions::default()).unwrap();
            assert_eq!(out.stdout.len(), 3_000_000);
            assert_eq!(out.stderr.len(), 3_000_000);
        }

        #[test]
        fn runner_timeout_kills_the_tree() {
            let marker = format!("gabetex-timeout-{}", std::process::id());
            let started = Instant::now();
            let out = run(
                "/bin/sh",
                &["-c", "(while :; do sleep 1; done) & (while :; do sleep 1; done) & wait", &marker],
                &RunOptions { timeout: Some(Duration::from_millis(300)), ..Default::default() },
            )
            .unwrap();
            assert!(out.timed_out);
            assert!(started.elapsed() < Duration::from_secs(8));
            thread::sleep(Duration::from_millis(200));
            assert_eq!(pgrep(&marker), 0, "nothing of the group survives");
        }

        #[test]
        fn runner_cancellation() {
            let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let f2 = flag.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(200));
                f2.store(true, std::sync::atomic::Ordering::SeqCst);
            });
            let cancel = || flag.load(std::sync::atomic::Ordering::SeqCst);
            let started = Instant::now();
            let out = run("/bin/sh", &["-c", "sleep 30"], &RunOptions { cancel: Some(&cancel), ..Default::default() }).unwrap();
            assert!(out.cancelled && !out.timed_out && !out.succeeded());
            assert!(started.elapsed() < Duration::from_secs(8));
        }

        #[test]
        #[ignore = "global: kills every registered child of the test process; run with --ignored --test-threads=1"]
        fn terminate_all_kills_registered_but_spares_unregistered() {
            let marker = format!("gabetex-killall-{}", std::process::id());
            let spared = format!("gabetex-spared-{}", std::process::id());
            let m2 = marker.clone();
            let t1 = thread::spawn(move || run("/bin/sh", &["-c", "sleep 30 & wait", &m2], &RunOptions::default()).unwrap());
            let s2 = spared.clone();
            let t2 = thread::spawn(move || {
                run("/bin/sh", &["-c", "sleep 2 & wait", &s2], &RunOptions { kill_on_terminate_all: false, ..Default::default() }).unwrap()
            });
            for _ in 0..50 {
                if pgrep(&marker) >= 1 && pgrep(&spared) >= 1 {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            assert!(terminate_all() >= 1);
            let out = t1.join().unwrap();
            assert!(out.cancelled);
            assert!(pgrep(&spared) >= 1, "kill_on_terminate_all=false is spared");
            let out2 = t2.join().unwrap();
            assert!(out2.succeeded());
        }

        fn pgrep(marker: &str) -> usize {
            let out = run("/usr/bin/pgrep", &["-f", marker], &RunOptions::default());
            match out {
                Ok(o) => o.stdout_text().lines().filter(|l| !l.trim().is_empty()).count(),
                Err(_) => 0,
            }
        }

        /// A fake TeX Live tree: kpsewhich + latexmk + pdftex shell scripts.
        fn fake_bin(name: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-scratch")).join(format!("dist-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let year_root = root.join("texlive").join("2031");
            let bin = year_root.join("bin").join("x86_64-linux");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::create_dir_all(year_root.join("tlpkg")).unwrap();
            let script = |n: &str, body: &str| {
                let p = bin.join(n);
                std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            };
            script("kpsewhich", &format!("echo '{}'", year_root.display()));
            script("latexmk", "exit 0");
            script("tlmgr", "echo tlmgr");
            script("pdftex", "echo 'pdfTeX 3.14159265-2.6-1.40.99 (TeX Live 2031)'");
            bin
        }

        #[test]
        fn inspect_fake_distribution() {
            let bin = fake_bin("inspect");
            let d = inspect(&bin).expect("fake TeX Live is detected");
            assert_eq!(d.name, "TeX Live 2031");
            assert_eq!(d.year, Some(2031));
            assert!(d.has(Tool::Latexmk) && d.has(Tool::Tlmgr) && !d.has(Tool::Synctex) && !d.has(Tool::Chktex));
            assert!(d.user_writable, "tlpkg is writable");
            assert_eq!(d.version_description.as_deref(), Some("pdfTeX 3.14159265-2.6-1.40.99 (TeX Live 2031)"));
            let ipc = d.to_ipc();
            assert!(ipc.latexmk && ipc.tlmgr && !ipc.synctex);
            assert_eq!(locate(Some(&bin.to_string_lossy())).map(|d| d.name), Some("TeX Live 2031".into()));
            let _ = std::fs::remove_dir_all(bin.ancestors().nth(4).unwrap());
        }

        #[test]
        fn inspect_rejects_non_tex_dirs() {
            // `/bin` holds TeX on distro installs (apt texlive), so use a directory known to have no TeX.
            let empty = std::env::temp_dir().join(format!("gtx-notex-{}", std::process::id()));
            std::fs::create_dir_all(empty.join("bin")).unwrap();
            std::fs::write(empty.join("bin").join("ls"), "").unwrap();
            assert!(inspect(&empty.join("bin")).is_none());
            let _ = std::fs::remove_dir_all(&empty);
            assert!(inspect(Path::new("/nonexistent-gtx")).is_none());
        }
    }
}

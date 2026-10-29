//! Missing-package installer: port of `PackageManager.swift` (`TeXPackageManager`).
//!
//! * file -> package: `tlmgr search --global --file /<regex-escaped name>` (needs the network), results
//!   ranked like the Swift app (package named like the file stem first, then like its directory);
//! * install: `tlmgr install <packages>`. When the TeX tree is not user-writable: Linux uses `pkexec`
//!   (polkit prompt), Windows re-runs the command elevated through PowerShell `Start-Process -Verb RunAs`
//!   (output captured in a temp file); the macOS `osascript` variant of the Swift app does not apply;
//! * MiKTeX has no `tlmgr`: packages are installed with `mpm --install=<id>` (or `miktex packages install`),
//!   `--admin` + elevation for an all-users installation. MiKTeX cannot search packages by file name, so
//!   the file stem is used as the package id.

use std::collections::HashSet;
use std::time::Duration;

use crate::commands::InstallResult;
use crate::core::distribution::{self, Distribution, Kind, ProcessOutput, RunOptions, Tool};

// ---------------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageError {
    TlmgrNotFound,
    UserCancelledAuthorization,
    NotFoundInRepository(String),
    /// The local TeX Live is older than the remote repository (frozen): the user must upgrade TeX Live.
    DistributionFrozen(String),
    /// tlmgr itself must be updated first (`tlmgr update --self`).
    TlmgrNeedsUpdate,
    Failed(String),
}

impl std::fmt::Display for PackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageError::TlmgrNotFound => f.write_str("tlmgr was not found in the TeX distribution."),
            PackageError::UserCancelledAuthorization => f.write_str("Authorization was cancelled."),
            PackageError::NotFoundInRepository(name) => write!(f, "No TeX Live package provides {name}."),
            PackageError::DistributionFrozen(m) => f.write_str(m),
            PackageError::TlmgrNeedsUpdate => f.write_str("The TeX Live manager needs to be updated first."),
            PackageError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for PackageError {}

// ---------------------------------------------------------------------------------------------------
// Public entry points (IPC)
// ---------------------------------------------------------------------------------------------------

/// `install_packages` command: installs the packages needed for `packages`, which may be TeX file names
/// (`tikz-cd.sty`, as listed in `CompileResult.missingPackages`) or package names (`tikz-cd`).
pub fn install_packages(packages: &[String]) -> InstallResult {
    match distribution::detect_cached() {
        Some(dist) => install_with(&dist, packages),
        None => InstallResult {
            ok: false,
            output: "No TeX distribution was found. Install TeX Live (or MiKTeX on Windows).".to_string(),
        },
    }
}

/// Like [`install_packages`] for a known distribution. Never panics; failures are `ok == false` with the
/// explanation (and whatever the tools printed) in `output`.
pub fn install_with(dist: &Distribution, packages: &[String]) -> InstallResult {
    let manager = PackageManager::new(dist);
    let mut log = String::new();
    let mut ok = true;
    let mut names: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut requested: Vec<&str> = Vec::new();
    for p in packages {
        let p = p.trim();
        if !p.is_empty() && !requested.contains(&p) {
            requested.push(p);
        }
    }
    for item in requested {
        if looks_like_file(item) {
            match manager.package_for_file(item) {
                Ok(Some(name)) => {
                    log.push_str(&format!("{item} -> {name}\n"));
                    if seen.insert(name.clone()) {
                        names.push(name);
                    }
                }
                Ok(None) => {
                    ok = false;
                    log.push_str(&format!("{}\n", PackageError::NotFoundInRepository(item.to_string())));
                }
                Err(e) => {
                    ok = false;
                    log.push_str(&format!("{e}\n"));
                }
            }
        } else if seen.insert(item.to_string()) {
            names.push(item.to_string());
        }
    }
    if names.is_empty() {
        if log.is_empty() {
            log.push_str("Nothing to install.\n");
        }
        return InstallResult { ok: ok && packages.iter().all(|p| p.trim().is_empty()), output: log };
    }
    match manager.install(&names) {
        Ok(output) => {
            log.push_str(&output);
        }
        Err(e) => {
            ok = false;
            log.push_str(&e.to_string());
            log.push('\n');
        }
    }
    distribution::invalidate_cache();
    InstallResult { ok, output: log }
}

/// A requested item that is a TeX file (`foo.sty`) rather than a package name.
pub fn looks_like_file(item: &str) -> bool {
    crate::core::log_parser::is_package_file(item)
}

// ---------------------------------------------------------------------------------------------------
// Package manager
// ---------------------------------------------------------------------------------------------------

pub struct PackageManager<'a> {
    pub distribution: &'a Distribution,
}

impl<'a> PackageManager<'a> {
    pub fn new(distribution: &'a Distribution) -> Self {
        PackageManager { distribution }
    }

    /// `kpsewhich file` succeeds (the file is findable by TeX).
    pub fn is_installed(&self, file: &str) -> bool {
        let Some(kpsewhich) = self.distribution.tool(Tool::Kpsewhich) else { return false };
        if file.is_empty() {
            return false;
        }
        let opts = RunOptions { env: self.distribution.process_environment(), timeout: Some(Duration::from_secs(10)), ..Default::default() };
        match distribution::run(kpsewhich, &[file], &opts) {
            Ok(out) => out.succeeded() && !out.stdout_text().trim().is_empty(),
            Err(_) => false,
        }
    }

    /// Package names whose files include `file` (e.g. "tikz-cd.sty" -> ["tikz-cd"]), via
    /// `tlmgr search --global --file /<file>` (needs the network; 60 s timeout); best match first.
    pub fn packages_providing(&self, file: &str) -> Result<Vec<String>, PackageError> {
        let Some(tlmgr) = self.distribution.tool(Tool::Tlmgr) else { return Err(PackageError::TlmgrNotFound) };
        let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
        if name.is_empty() {
            return Ok(Vec::new());
        }
        let opts = RunOptions {
            env: self.distribution.process_environment(),
            timeout: Some(Duration::from_secs(60)),
            ..Default::default()
        };
        let output = distribution::run(tlmgr, &search_arguments(name), &opts).map_err(PackageError::Failed)?;
        if output.cancelled {
            return Err(PackageError::Failed("The package search was cancelled.".into()));
        }
        if output.timed_out {
            return Err(PackageError::Failed(
                "The TeX Live package repository did not respond in time. Check your internet connection.".into(),
            ));
        }
        let text = output.combined_text();
        if let Some(error) = map_error(&text, output.exit_code.unwrap_or(-1)) {
            // tlmgr printed results and a harmless warning: use the results.
            if !(matches!(error, PackageError::Failed(_)) && !parse_search_output(&text).is_empty()) {
                return Err(error);
            }
        }
        Ok(rank(&parse_search_output(&text), name))
    }

    /// The package to install for a missing file: TeX Live asks tlmgr; MiKTeX uses the file stem.
    pub fn package_for_file(&self, file: &str) -> Result<Option<String>, PackageError> {
        match self.distribution.kind {
            Kind::TeXLive => Ok(self.packages_providing(file)?.into_iter().next()),
            Kind::MiKTeX => {
                let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
                let stem = match name.rfind('.') {
                    Some(i) if i > 0 => &name[..i],
                    _ => name,
                };
                Ok((!stem.is_empty()).then(|| stem.to_string()))
            }
        }
    }

    /// Installs packages; returns the tool's output. Elevates when the distribution is not user-writable.
    pub fn install(&self, packages: &[String]) -> Result<String, PackageError> {
        let names: Vec<&String> = packages.iter().filter(|p| !p.is_empty()).collect();
        if names.is_empty() {
            return Ok(String::new());
        }
        if let Some(bad) = names.iter().find(|n| !is_valid_package_name(n)) {
            return Err(PackageError::Failed(format!("“{bad}” is not a valid TeX Live package name.")));
        }
        match self.distribution.kind {
            Kind::TeXLive => {
                let mut args = vec!["install".to_string()];
                args.extend(names.iter().map(|s| s.to_string()));
                self.run_tlmgr(&args)
            }
            Kind::MiKTeX => self.install_miktex(&names.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        }
    }

    /// `tlmgr update --self` (with an admin prompt if needed).
    pub fn update_manager(&self) -> Result<String, PackageError> {
        self.run_tlmgr(&["update".to_string(), "--self".to_string()])
    }

    fn run_tlmgr(&self, arguments: &[String]) -> Result<String, PackageError> {
        let Some(tlmgr) = self.distribution.tool(Tool::Tlmgr) else { return Err(PackageError::TlmgrNotFound) };
        let (output, elevated_with) = self.run_maybe_elevated(tlmgr, arguments)?;
        if output.cancelled {
            return Err(PackageError::Failed("The installation was cancelled.".into()));
        }
        if output.timed_out {
            return Err(PackageError::Failed("The installation took too long and was stopped.".into()));
        }
        let text = output.combined_text();
        if let Some(error) = map_error_elevated(&text, output.exit_code.unwrap_or(-1), elevated_with == Elevation::Pkexec) {
            return Err(error);
        }
        Ok(text)
    }

    fn install_miktex(&self, names: &[String]) -> Result<String, PackageError> {
        let d = self.distribution;
        let admin = !d.user_writable;
        let (exe, per_package): (&str, bool) = match (d.tool(Tool::Mpm), d.tool(Tool::Miktex)) {
            (Some(mpm), _) => (mpm, true),
            (None, Some(miktex)) => (miktex, false),
            (None, None) => return Err(PackageError::TlmgrNotFound),
        };
        let mut invocations: Vec<Vec<String>> = Vec::new();
        if per_package {
            for n in names {
                let mut a = Vec::new();
                if admin {
                    a.push("--admin".to_string());
                }
                a.push(format!("--install={n}"));
                invocations.push(a);
            }
        } else {
            let mut a = Vec::new();
            if admin {
                a.push("--admin".to_string());
            }
            a.extend(["packages".to_string(), "install".to_string()]);
            a.extend(names.iter().cloned());
            invocations.push(a);
        }
        let mut text = String::new();
        let mut last: Option<ProcessOutput> = None;
        if cfg!(windows) && admin {
            let (out, _) = self.run_elevated_windows(exe, &invocations)?;
            text = out.combined_text();
            last = Some(out);
        } else {
            for args in &invocations {
                let opts = RunOptions {
                    env: d.process_environment(),
                    timeout: Some(Duration::from_secs(1800)),
                    kill_on_terminate_all: false,
                    ..Default::default()
                };
                let out = distribution::run(exe, args, &opts).map_err(PackageError::Failed)?;
                text.push_str(&out.combined_text());
                let failed = !out.succeeded();
                last = Some(out);
                if failed {
                    break;
                }
            }
        }
        let out = last.unwrap_or_default();
        if out.cancelled {
            return Err(PackageError::Failed("The installation was cancelled.".into()));
        }
        if out.timed_out {
            return Err(PackageError::Failed("The installation took too long and was stopped.".into()));
        }
        if let Some(e) = map_error_elevated(&text, out.exit_code.unwrap_or(-1), false) {
            return Err(e);
        }
        Ok(text)
    }

    /// Runs a tlmgr command directly, or elevated when the TeX tree is not writable by the user.
    fn run_maybe_elevated(&self, tlmgr: &str, arguments: &[String]) -> Result<(ProcessOutput, Elevation), PackageError> {
        let d = self.distribution;
        // Not killed at app quit: interrupting tlmgr half-way can leave TeX Live inconsistent.
        let opts = RunOptions {
            env: d.process_environment(),
            timeout: Some(Duration::from_secs(1800)),
            kill_on_terminate_all: false,
            ..Default::default()
        };
        if cfg!(windows) && !d.user_writable {
            let (out, _) = self.run_elevated_windows(tlmgr, &[arguments.to_vec()])?;
            return Ok((out, Elevation::Windows));
        }
        let pkexec = if cfg!(unix) && !d.user_writable { find_in_path("pkexec") } else { None };
        let invocation = invocation(tlmgr, arguments, d.user_writable, pkexec.as_deref());
        let out = distribution::run(&invocation.executable, &invocation.arguments, &opts).map_err(PackageError::Failed)?;
        let elevation = if pkexec.is_some() { Elevation::Pkexec } else { Elevation::None };
        Ok((out, elevation))
    }

    /// PowerShell `Start-Process -Verb RunAs -Wait` around `cmd /c "<exe> args > out.txt 2>&1"`.
    fn run_elevated_windows(&self, exe: &str, invocations: &[Vec<String>]) -> Result<(ProcessOutput, Elevation), PackageError> {
        let out_file = std::env::temp_dir().join(format!("gabetex-elevated-{}-{}.txt", std::process::id(), nanos()));
        let out_path = out_file.to_string_lossy().into_owned();
        let script = windows_elevation_script(exe, invocations, &out_path);
        let ps_args = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script.as_str()];
        let opts = RunOptions { timeout: Some(Duration::from_secs(1800)), kill_on_terminate_all: false, ..Default::default() };
        let ps = distribution::run("powershell.exe", &ps_args, &opts).map_err(PackageError::Failed)?;
        let captured = std::fs::read(&out_file).map(|d| String::from_utf8_lossy(&d).into_owned()).unwrap_or_default();
        let _ = std::fs::remove_file(&out_file);
        let ps_err = ps.stderr_text();
        if captured.is_empty() && (ps_err.contains("canceled by the user") || ps_err.contains("cancelled by the user")) {
            return Err(PackageError::UserCancelledAuthorization);
        }
        let mut out = ps;
        out.stdout = captured.into_bytes();
        Ok((out, Elevation::Windows))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Elevation {
    None,
    Pkexec,
    Windows,
}

fn nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

fn find_in_path(name: &str) -> Option<String> {
    distribution::path_directories()
        .into_iter()
        .chain(["/usr/bin", "/bin", "/usr/local/bin"].map(std::path::PathBuf::from))
        .find_map(|d| distribution::find_tool(&d, name))
        .map(|p| p.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------------------------------
// Command construction (pure, unit-tested)
// ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub executable: String,
    pub arguments: Vec<String>,
}

/// `tlmgr search --global --file /<file>`. tlmgr treats the pattern as a Perl regular expression, so
/// metacharacters in the file name are escaped.
pub fn search_arguments(file: &str) -> Vec<String> {
    vec!["search".into(), "--global".into(), "--file".into(), format!("/{}", regex_escaped(file))]
}

pub fn regex_escaped(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 4);
    for ch in text.chars() {
        if "\\.^$|?*+()[]{}".contains(ch) {
            result.push('\\');
        }
        result.push(ch);
    }
    result
}

/// TeX Live / MiKTeX package names: ASCII letters, digits and `-_.+` (e.g. "tikz-cd", "biber.x86_64-linux").
pub fn is_valid_package_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.+".contains(c))
}

/// Direct tlmgr when the installation is user-writable, else `pkexec <tlmgr> ...` (Linux polkit prompt)
/// when pkexec is available (without it the direct call is returned and fails with a permission error).
pub fn invocation(tlmgr: &str, arguments: &[String], user_writable: bool, pkexec: Option<&str>) -> Invocation {
    match (user_writable, pkexec) {
        (false, Some(pkexec)) => {
            let mut args = vec![tlmgr.to_string()];
            args.extend(arguments.iter().cloned());
            Invocation { executable: pkexec.to_string(), arguments: args }
        }
        _ => Invocation { executable: tlmgr.to_string(), arguments: arguments.to_vec() },
    }
}

/// Single-quotes a word for /bin/sh (`'` becomes `'\''`).
pub fn shell_quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// A PowerShell single-quoted string literal (`'` doubled).
pub fn powershell_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The PowerShell script that runs each invocation elevated through `cmd /c`, capturing stdout+stderr in
/// `out_file`, and exits with the exit code of the elevated command.
///
/// `cmd /c ""<exe>" a b & "<exe>" c d > "<out>" 2>&1"`: cmd strips the outer quote pair.
pub fn windows_elevation_script(exe: &str, invocations: &[Vec<String>], out_file: &str) -> String {
    let commands: Vec<String> = invocations
        .iter()
        .map(|args| {
            let mut c = format!("\"{exe}\"");
            for a in args {
                c.push(' ');
                if a.chars().any(|ch| ch == ' ' || ch == '"') {
                    c.push_str(&format!("\"{}\"", a.replace('"', "")));
                } else {
                    c.push_str(a);
                }
            }
            c
        })
        .collect();
    let inner = format!("\"({}) > \"{out_file}\" 2>&1\"", commands.join(" & "));
    format!(
        "$p = Start-Process -FilePath 'cmd.exe' -ArgumentList '/c',{} -Verb RunAs -Wait -WindowStyle Hidden -PassThru; exit $p.ExitCode",
        powershell_quoted(&inner)
    )
}

// ---------------------------------------------------------------------------------------------------
// Output parsing (pure, unit-tested)
// ---------------------------------------------------------------------------------------------------

/// One `pkg:` block of `tlmgr search --file` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchEntry {
    pub package: String,
    pub files: Vec<String>,
}

/// `tlmgr search --file` output: `pkg:` header lines followed by tab-indented file paths, in output order.
pub fn parse_search_output(text: &str) -> Vec<SearchEntry> {
    let mut result: Vec<SearchEntry> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() {
            continue;
        }
        if line.starts_with('\t') || line.starts_with(' ') {
            let path = line.trim_matches(|c| c == ' ' || c == '\t');
            if !path.is_empty() {
                if let Some(last) = result.last_mut() {
                    last.files.push(path.to_string());
                }
            }
            continue;
        }
        if !line.ends_with(':') || line.starts_with("tlmgr") || line.contains(' ') {
            continue;
        }
        let name = &line[..line.len() - 1];
        if !name.is_empty() {
            result.push(SearchEntry { package: name.to_string(), files: Vec::new() });
        }
    }
    result
}

fn last_component(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn parent_name(path: &str) -> &str {
    let p = path.trim_end_matches(['/', '\\']);
    match p.rfind(['/', '\\']) {
        Some(i) => last_component(&p[..i]),
        None => "",
    }
}

/// Keeps packages that contain a file named exactly `file`, best first: name == file stem, then name ==
/// the parent directory of the file, then the rest (in tlmgr's order).
pub fn rank(entries: &[SearchEntry], file: &str) -> Vec<String> {
    let stem = match file.rfind('.') {
        Some(i) if i > 0 => &file[..i],
        _ => file,
    }
    .to_lowercase();
    let mut scored: Vec<(u8, usize, &str)> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let matching: Vec<&String> = entry.files.iter().filter(|f| last_component(f) == file).collect();
        if matching.is_empty() && !entry.files.is_empty() {
            continue;
        }
        let base = entry.package.split('.').next().unwrap_or(&entry.package).to_lowercase();
        let parents: HashSet<String> = matching.iter().map(|f| parent_name(f).to_lowercase()).collect();
        let score = if base == stem {
            0
        } else if parents.contains(&base) {
            1
        } else {
            2
        };
        scored.push((score, index, entry.package.as_str()));
    }
    scored.sort_by_key(|(s, i, _)| (*s, *i));
    let mut seen = HashSet::new();
    scored.into_iter().map(|(_, _, n)| n).filter(|n| seen.insert(*n)).map(|n| n.to_string()).collect()
}

/// Maps tlmgr failures. `None` when the output does not indicate a failure.
pub fn map_error(output: &str, exit_code: i32) -> Option<PackageError> {
    if output.contains("(-128)") || output.contains("User canceled") || output.contains("User cancelled") {
        return Some(PackageError::UserCancelledAuthorization);
    }
    if output.contains("tlmgr itself needs to be updated") {
        return Some(PackageError::TlmgrNeedsUpdate);
    }
    if let Some(line) = output.split('\n').find(|l| l.contains("is older than remote repository")) {
        let detail = line.replace("tlmgr: ", "");
        let detail = detail.trim_matches(|c: char| c == ' ' || c == '\t' || c == '\r');
        return Some(PackageError::DistributionFrozen(format!(
            "Your TeX Live is older than the package repository ({detail}). Install the current TeX Live / MacTeX to get new packages."
        )));
    }
    if let Some(line) = output.split('\n').find(|l| l.contains("not present in repository")) {
        // "tlmgr install: package foo not present in repository."
        let name = line
            .find("package ")
            .map(|i| line[i + "package ".len()..].chars().take_while(|c| *c != ' ').collect::<String>())
            .unwrap_or_default();
        return Some(PackageError::NotFoundInRepository(if name.is_empty() { line.trim().to_string() } else { name }));
    }
    if exit_code == 0 {
        return None;
    }
    let lines: Vec<&str> = output.split('\n').filter(|l| !l.is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(6)..].join("\n");
    let tail = tail.trim();
    Some(PackageError::Failed(if tail.is_empty() {
        format!("tlmgr failed (exit status {exit_code}).")
    } else {
        tail.to_string()
    }))
}

/// [`map_error`] plus the polkit / UAC outcomes: `pkexec` exits 126 (not authorized) / 127 (dialog
/// dismissed) without running tlmgr; a declined UAC prompt makes PowerShell print "canceled by the user".
pub fn map_error_elevated(output: &str, exit_code: i32, used_pkexec: bool) -> Option<PackageError> {
    if used_pkexec && (exit_code == 126 || exit_code == 127) && !output.contains("tlmgr") {
        return Some(PackageError::UserCancelledAuthorization);
    }
    if output.contains("canceled by the user") || output.contains("cancelled by the user") {
        return Some(PackageError::UserCancelledAuthorization);
    }
    map_error(output, exit_code)
}

// ---------------------------------------------------------------------------------------------------
// Tests (ports of UnitChecks "TeXPackageManager" suites + a fake tlmgr end-to-end on Unix)
// ---------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Tests/Fixtures/TeXToolchain").join(name);
        String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {}: {e}", path.display()))).into_owned()
    }

    #[test]
    fn search_parsing_and_ranking() {
        let real = fixture("tools/tlmgr-search-tikz-cd.txt");
        let parsed = parse_search_output(&real);
        assert_eq!(parsed.iter().map(|e| e.package.as_str()).collect::<Vec<_>>(), ["tikz-cd"]);
        assert_eq!(parsed[0].files, ["texmf-dist/tex/latex/tikz-cd/tikz-cd.sty"]);
        assert_eq!(rank(&parsed, "tikz-cd.sty"), ["tikz-cd"]);
        let multi = "tlmgr: package repository https://mirror/tlnet (verified)\nbundle-x:\n\ttexmf-dist/tex/latex/bundle-x/foo.sty\nfoo-extra:\n\ttexmf-dist/tex/latex/foo/foo.sty\nfoo:\n\ttexmf-dist/tex/latex/foo/foo.sty\n\ttexmf-dist/doc/latex/foo/README\ndecoy:\n\ttexmf-dist/tex/latex/decoy/foo.sty.bak\n";
        assert_eq!(
            rank(&parse_search_output(multi), "foo.sty"),
            ["foo", "bundle-x", "foo-extra"],
            "stem match, then tlmgr order; non-exact file matches dropped"
        );
        let by_directory = "pgf-a:\n\ttexmf-dist/tex/latex/pgfplots/pgfplots.sty\npgfplots:\n\ttexmf-dist/tex/latex/pgfplots/pgfplots.sty\nzz:\n\ttexmf-dist/tex/latex/zz/pgfplots.sty\n";
        assert_eq!(rank(&parse_search_output(by_directory), "pgfplots.sty").first().map(|s| s.as_str()), Some("pgfplots"));
        // The directory name decides between two non-stem packages.
        let dirs = "a-bundle:\n\ttexmf-dist/tex/latex/other/x.sty\nz-pkg:\n\ttexmf-dist/tex/latex/z-pkg/x.sty\n";
        assert_eq!(rank(&parse_search_output(dirs), "x.sty"), ["z-pkg", "a-bundle"]);
        // CRLF output (Windows).
        let crlf = "tikz-cd:\r\n\ttexmf-dist/tex/latex/tikz-cd/tikz-cd.sty\r\n";
        assert_eq!(parse_search_output(crlf)[0].files, ["texmf-dist/tex/latex/tikz-cd/tikz-cd.sty"]);
        assert_eq!(search_arguments("tikz-cd.sty"), ["search", "--global", "--file", "/tikz-cd\\.sty"]);
        assert_eq!(regex_escaped("c++(x).sty"), "c\\+\\+\\(x\\)\\.sty");
    }

    #[test]
    fn command_construction() {
        let tlmgr = "/usr/local/texlive/2026/bin/x86_64-linux/tlmgr";
        let args = vec!["install".to_string(), "tikz-cd".to_string()];
        let direct = invocation(tlmgr, &args, true, Some("/usr/bin/pkexec"));
        assert_eq!(direct, Invocation { executable: tlmgr.into(), arguments: args.clone() }, "writable: no elevation");
        let admin = invocation(tlmgr, &args, false, Some("/usr/bin/pkexec"));
        assert_eq!(admin.executable, "/usr/bin/pkexec");
        assert_eq!(admin.arguments, [tlmgr, "install", "tikz-cd"]);
        let no_pkexec = invocation(tlmgr, &args, false, None);
        assert_eq!(no_pkexec.executable, tlmgr);
        assert_eq!(shell_quoted("a'b"), r#"'a'\''b'"#);
        assert_eq!(powershell_quoted("it's"), "'it''s'");
        assert!(is_valid_package_name("tikz-cd") && is_valid_package_name("biber.x86_64-linux") && is_valid_package_name("c++"));
        assert!(!is_valid_package_name("x; rm -rf ~") && !is_valid_package_name("--all") && !is_valid_package_name(""));
        assert!(!is_valid_package_name("a b") && !is_valid_package_name("a\"b") && !is_valid_package_name("ação"));
    }

    #[test]
    fn windows_elevation_script_quoting() {
        let script = windows_elevation_script(
            r"C:\texlive\2026\bin\windows\tlmgr.bat",
            &[vec!["install".into(), "tikz-cd".into(), "pgf".into()]],
            r"C:\Users\John's\AppData\Local\Temp\o.txt",
        );
        assert!(script.contains("-Verb RunAs") && script.contains("-Wait") && script.contains("exit $p.ExitCode"));
        assert!(script.contains(r#"'"("C:\texlive\2026\bin\windows\tlmgr.bat" install tikz-cd pgf) > "C:\Users\John''s\AppData\Local\Temp\o.txt" 2>&1"'"#), "{script}");
        let two = windows_elevation_script("mpm.exe", &[vec!["--admin".into(), "--install=a".into()], vec!["--admin".into(), "--install=b".into()]], "o.txt");
        assert!(two.contains(r#"("mpm.exe" --admin --install=a & "mpm.exe" --admin --install=b)"#), "{two}");
    }

    #[test]
    fn error_mapping() {
        assert_eq!(map_error("0:47: execution error: User canceled. (-128)", 1), Some(PackageError::UserCancelledAuthorization));
        assert_eq!(
            map_error("tlmgr: Remote database (revision 7) seems to be older than local (revision 9 of x); please use a different mirror.\ntlmgr itself needs to be updated.\nPlease do this via either\n  tlmgr update --self", 1),
            Some(PackageError::TlmgrNeedsUpdate)
        );
        match map_error("tlmgr: Local TeX Live (2025) is older than remote repository (2026).\nCross release updates are only supported with\n  update-tlmgr-latest(.sh/.exe) --update", 1) {
            Some(PackageError::DistributionFrozen(m)) => assert!(m.contains("Local TeX Live (2025) is older than remote repository (2026)."), "{m}"),
            other => panic!("frozen distribution not detected: {other:?}"),
        }
        assert_eq!(
            map_error("tlmgr install: package nosuchpkg not present in repository.", 1),
            Some(PackageError::NotFoundInRepository("nosuchpkg".into()))
        );
        assert_eq!(map_error("tlmgr: package repository x (verified)\n[1/1, ??:??/??:??] install: tikz-cd [5k]\nrunning mktexlsr ...\ndone", 0), None);
        assert!(matches!(map_error("boom", 2), Some(PackageError::Failed(m)) if m == "boom"));
        assert!(matches!(map_error("", 3), Some(PackageError::Failed(m)) if m.contains("exit status 3")));
        // Only the last 6 lines of a long failure are kept.
        let long = (1..=10).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        assert!(matches!(map_error(&long, 1), Some(PackageError::Failed(m)) if m.starts_with("line5") && m.ends_with("line10")));
        // pkexec / UAC outcomes
        assert_eq!(map_error_elevated("", 126, true), Some(PackageError::UserCancelledAuthorization));
        assert_eq!(map_error_elevated("Error executing command as another user: Not authorized", 127, true), Some(PackageError::UserCancelledAuthorization));
        assert!(matches!(map_error_elevated("tlmgr: boom", 127, true), Some(PackageError::Failed(_))));
        assert_eq!(map_error_elevated("This command cannot be run due to the error: The operation was canceled by the user.", 1, false), Some(PackageError::UserCancelledAuthorization));
        assert_eq!(map_error_elevated("", 126, false).map(|e| matches!(e, PackageError::Failed(_))), Some(true));
        assert_eq!(PackageError::NotFoundInRepository("x".into()).to_string(), "No TeX Live package provides x.");
    }

    #[test]
    fn no_distribution_tools() {
        let d = Distribution {
            name: "x".into(),
            year: None,
            kind: Kind::TeXLive,
            bin_dir: "/nonexistent".into(),
            texmf_root: None,
            user_writable: true,
            tools: Default::default(),
            version_description: None,
        };
        let m = PackageManager::new(&d);
        assert!(!m.is_installed("amsmath.sty"));
        assert_eq!(m.packages_providing("x.sty"), Err(PackageError::TlmgrNotFound));
        assert_eq!(m.install(&["tikz-cd".into()]), Err(PackageError::TlmgrNotFound));
        assert_eq!(m.install(&[]), Ok(String::new()));
        assert!(matches!(m.install(&["bad name".into()]), Err(PackageError::Failed(msg)) if msg.contains("not a valid")));
        let r = install_with(&d, &["tikz-cd".into()]);
        assert!(!r.ok && r.output.contains("tlmgr was not found"), "{r:?}");
        let empty = install_with(&d, &[]);
        assert!(empty.ok && empty.output.contains("Nothing to install"), "{empty:?}");
    }

    #[test]
    fn miktex_uses_the_file_stem() {
        let mut d = Distribution {
            name: "MiKTeX".into(),
            year: None,
            kind: Kind::MiKTeX,
            bin_dir: "C:/miktex".into(),
            texmf_root: None,
            user_writable: true,
            tools: Default::default(),
            version_description: None,
        };
        let m = PackageManager::new(&d);
        assert_eq!(m.package_for_file("tikz-cd.sty"), Ok(Some("tikz-cd".into())));
        assert_eq!(m.install(&["tikz-cd".into()]), Err(PackageError::TlmgrNotFound), "no mpm / miktex");
        d.tools.insert(Tool::Mpm, "mpm".into());
        assert!(d.has_package_manager());
    }

    #[cfg(unix)]
    mod unix {
        use super::super::*;
        use std::os::unix::fs::PermissionsExt;

        /// A fake TeX Live whose tlmgr records its arguments and answers `search` with canned output.
        fn fake_distribution(name: &str, install_body: &str) -> (Distribution, std::path::PathBuf) {
            let base = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target/test-scratch"))
                .join(format!("pkg-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let bin = base.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            let log = base.join("calls.txt");
            let tlmgr = bin.join("tlmgr");
            let script = format!(
                r#"#!/bin/sh
echo "$@" >> '{log}'
case "$1" in
  search)
    case "$4" in
      /tikz-cd*) printf 'tlmgr: package repository https://x (verified)\ntikz-cd:\n\ttexmf-dist/tex/latex/tikz-cd/tikz-cd.sty\n';;
      /pgfplots*) printf 'pgf-a:\n\ttexmf-dist/tex/latex/pgfplots/pgfplots.sty\npgfplots:\n\ttexmf-dist/tex/latex/pgfplots/pgfplots.sty\n';;
      *) ;;
    esac;;
  install) {install_body};;
esac
"#,
                log = log.display()
            );
            std::fs::write(&tlmgr, script).unwrap();
            std::fs::set_permissions(&tlmgr, std::fs::Permissions::from_mode(0o755)).unwrap();
            let d = Distribution {
                name: "Fake".into(),
                year: None,
                kind: Kind::TeXLive,
                bin_dir: bin.to_string_lossy().into_owned(),
                texmf_root: None,
                user_writable: true,
                tools: [(Tool::Tlmgr, tlmgr.to_string_lossy().into_owned())].into_iter().collect(),
                version_description: None,
            };
            (d, base)
        }

        #[test]
        fn maps_files_to_packages_and_installs() {
            let (d, base) = fake_distribution("ok", "echo \"installed: $2 $3\"");
            let m = PackageManager::new(&d);
            assert_eq!(m.packages_providing("tikz-cd.sty"), Ok(vec!["tikz-cd".to_string()]));
            assert_eq!(m.packages_providing("pgfplots.sty"), Ok(vec!["pgfplots".to_string(), "pgf-a".to_string()]));
            assert_eq!(m.packages_providing("nothing.sty"), Ok(vec![]));
            let result = install_with(&d, &["tikz-cd.sty".into(), "pgfplots.sty".into(), "tikz-cd.sty".into(), "booktabs".into()]);
            assert!(result.ok, "{result:?}");
            assert!(result.output.contains("tikz-cd.sty -> tikz-cd"), "{}", result.output);
            assert!(result.output.contains("installed: tikz-cd pgfplots"), "{}", result.output);
            let calls = std::fs::read_to_string(base.join("calls.txt")).unwrap();
            assert!(calls.lines().any(|l| l == "install tikz-cd pgfplots booktabs"), "one install call with unique names: {calls}");
            let _ = std::fs::remove_dir_all(&base);
        }

        #[test]
        fn unknown_files_and_tlmgr_failures_are_reported() {
            let (d, base) = fake_distribution(
                "fail",
                "echo 'tlmgr install: package booktabs not present in repository.'; exit 1",
            );
            let missing = install_with(&d, &["nothing.sty".into()]);
            assert!(!missing.ok);
            assert!(missing.output.contains("No TeX Live package provides nothing.sty."), "{}", missing.output);
            let failed = install_with(&d, &["booktabs".into()]);
            assert!(!failed.ok);
            assert!(failed.output.contains("No TeX Live package provides booktabs."), "{}", failed.output);
            let invalid = install_with(&d, &["a;b".into()]);
            assert!(!invalid.ok && invalid.output.contains("not a valid"), "{}", invalid.output);
            let _ = std::fs::remove_dir_all(&base);
        }
    }
}

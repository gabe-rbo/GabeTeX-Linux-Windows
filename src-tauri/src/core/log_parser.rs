//! TeX log parser: a faithful port of `TeXLogParser.swift`, `TeXLogParser+Tools.swift` and `ByteScan.swift`
//! (plus the log-path mapping of `ShadowPathMapper` / `LaTeXCompiler.projectFile(forLogPath:)`).
//!
//! The log is expected to be unwrapped (`max_print_line=10000`) and produced with `-file-line-error`.
//! The parser keeps a **file stack** from the `(file` / `)` tokens TeX prints when it opens and closes
//! input files, so messages without a file name are attributed to the innermost project file. Lines that
//! belong to an error block, a warning, a bad-box display or a runaway argument are not scanned for
//! parentheses (they contain source text that is not balanced).
//!
//! Everything is a single pass over the log's lines (`&str` slices, ASCII needles, no regexes).
//! Paths are plain `String`s (absolute, `/`-separated; drive-letter paths are tolerated).
//! Lines and columns are 1-based; columns/lengths are UTF-16 units.

use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------------------------------

/// Same ordering as Swift's `DiagnosticSeverity` (raw values 0...3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    /// Informational (e.g. "Label(s) may have changed. Rerun").
    Info = 0,
    /// Overfull/underfull boxes.
    BadBox = 1,
    Warning = 2,
    Error = 3,
}

/// Same cases as Swift's `DiagnosticSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Tex,
    Latexmk,
    Bibtex,
    Biber,
    Makeindex,
    Chktex,
    Structure,
}

/// Rich diagnostic as produced by the parser (Swift `Diagnostic` minus the random id). Convert it to the
/// IPC type with [`LogDiagnostic::to_ipc`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogDiagnostic {
    pub severity: Severity,
    pub source: Source,
    /// One-line human readable message.
    pub message: String,
    /// Optional multi-line context (the `l.12 ...` lines of a TeX error).
    pub detail: Option<String>,
    /// Absolute path in project space (never a shadow/build path). `None` if unknown.
    pub path: Option<String>,
    /// 1-based line.
    pub line: Option<u32>,
    /// 1-based UTF-16 column.
    pub column: Option<u32>,
    /// Length in UTF-16 units of the offending span.
    pub length: Option<u32>,
    /// Name of a file TeX could not find (e.g. "tikz-cd.sty"), used to offer a package install.
    pub missing_file: Option<String>,
}

impl LogDiagnostic {
    pub fn new(severity: Severity, source: Source, message: impl Into<String>) -> Self {
        LogDiagnostic {
            severity,
            source,
            message: message.into(),
            detail: None,
            path: None,
            line: None,
            column: None,
            length: None,
            missing_file: None,
        }
    }

    /// Converts to the IPC `Diagnostic` of `commands.rs`. `detail`, `length` and `missing_file` have no
    /// IPC counterpart. Severity: bad boxes become `"info"` (the IPC has no bad-box level); source:
    /// tex/latexmk/makeindex -> `"latex"`, bibtex/biber -> `"bibtex"`. A missing line becomes 0.
    pub fn to_ipc(&self) -> crate::commands::Diagnostic {
        crate::commands::Diagnostic {
            severity: match self.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::BadBox | Severity::Info => "info",
            }
            .to_string(),
            message: self.message.clone(),
            path: self.path.clone().unwrap_or_default(),
            line: self.line.unwrap_or(0),
            column: self.column,
            detail: self.detail.clone(),
            length: self.length,
            missing_file: self.missing_file.clone(),
            source: match self.source {
                Source::Tex | Source::Latexmk | Source::Makeindex => "latex",
                Source::Bibtex | Source::Biber => "bibtex",
                Source::Chktex => "chktex",
                Source::Structure => "structure",
            }
            .to_string(),
        }
    }
}

/// What `latexmk`'s own output (stdout/stderr) reports beyond the TeX log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LatexmkOutputSummary {
    /// Files latexmk reported as missing ("Latexmk: Missing input file 'x.sty' ..."), in order, unique.
    pub missing_files: Vec<String>,
    /// Problems not visible in the TeX log, e.g. a bibliography tool that could not run at all.
    pub diagnostics: Vec<LogDiagnostic>,
}

// ---------------------------------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------------------------------

/// Parses a TeX `.log`.
///
/// `map_path` maps a path as written in the log (relative to the compile directory, or absolute) to a
/// project path; return `None` for files outside the project (they are then attributed to the nearest
/// project file on the file stack, or left without a file). Returns deduplicated diagnostics, errors
/// first, then by file and line.
pub fn parse<F: Fn(&str) -> Option<String>>(log: &str, map_path: F) -> Vec<LogDiagnostic> {
    let mut parser = LogParser::new(log, map_path);
    parser.run();
    normalized(parser.diagnostics)
}

/// Removes duplicates and sorts: errors first (severity descending), then by file path, line, column.
/// Drops TeX's follow-up "Emergency stop." when a more specific error exists.
pub fn normalized(diagnostics: Vec<LogDiagnostic>) -> Vec<LogDiagnostic> {
    let has_specific_error = diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error && !is_follow_up_error(&d.message));
    let mut keep = vec![false; diagnostics.len()];
    {
        type Key<'a> = (Severity, Source, Option<&'a str>, Option<u32>, Option<u32>, &'a str);
        let mut seen: HashSet<Key> = HashSet::with_capacity(diagnostics.len().min(1024));
        for (idx, d) in diagnostics.iter().enumerate() {
            if has_specific_error && d.severity == Severity::Error && is_follow_up_error(&d.message) {
                continue;
            }
            let key = (d.severity, d.source, d.path.as_deref(), d.line, d.column, d.message.as_str());
            keep[idx] = seen.insert(key);
        }
    }
    let mut result: Vec<LogDiagnostic> = diagnostics
        .into_iter()
        .zip(keep)
        .filter_map(|(d, k)| k.then_some(d))
        .collect();
    // Stable sort: severity descending, path (none last), line (none last), column (none = 0).
    result.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| match (&a.path, &b.path) {
                (Some(x), Some(y)) => x.as_bytes().cmp(y.as_bytes()),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.line.unwrap_or(u32::MAX).cmp(&b.line.unwrap_or(u32::MAX)))
            .then_with(|| a.column.unwrap_or(0).cmp(&b.column.unwrap_or(0)))
    });
    result
}

fn is_follow_up_error(message: &str) -> bool {
    message == "Emergency stop." || message.starts_with("==> Fatal error occurred")
}

/// Extensions that mark a `(token` in the log as a file being opened.
const FILE_EXTENSIONS: &[&str] = &[
    "tex", "sty", "cls", "cfg", "def", "clo", "fd", "aux", "toc", "lof", "lot", "bbl", "out", "nav", "snm", "vrb",
    "ind", "idx", "ltx", "dtx", "ins", "bib", "bst", "mkii", "mkiv", "lua", "ldf", "bbx", "cbx", "lbx", "dbx", "cnf",
    "dict", "enc", "fdd", "pgf", "tikz", "pdf_tex", "eps_tex", "mtx", "gls", "glo", "tdo", "lol", "loa", "thm",
    "xmpdata", "w18",
];

/// User-facing explanation for a biber step that could not run (no .blg was produced).
pub const BIBER_FAILURE_MESSAGE: &str = "Biber failed to run. Your TeX distribution’s biber appears to be broken — run “sudo tlmgr update biber” or reinstall MacTeX, or use “backend=bibtex” with biblatex.";

/// Names a TeX Live package can provide (for the installer): no directory part and a package-ish extension.
/// (`LaTeXCompiler.isPackageFile`.)
pub fn is_package_file(name: &str) -> bool {
    if name.contains('/') {
        return false;
    }
    let ext = path_extension(name).to_ascii_lowercase();
    const EXTS: &[&str] = &[
        "sty", "cls", "def", "cfg", "clo", "fd", "ldf", "bst", "bbx", "cbx", "lbx", "dbx", "tfm", "otf", "ttf", "pfb",
        "map", "enc", "lua",
    ];
    EXTS.contains(&ext.as_str()) && name.chars().count() > ext.chars().count() + 1
}

/// The `missingPackages` input of a compile: the `missing_file` of every diagnostic plus latexmk's own
/// missing files, restricted to names a TeX Live package can provide ([`is_package_file`]), unique, in
/// order. The mapping file name -> TeX Live package name is not done here (Swift asks
/// `tlmgr search --global --file /<name>`; see `PackageManager.swift`).
pub fn missing_package_files(diagnostics: &[LogDiagnostic], latexmk: &LatexmkOutputSummary) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    let names = diagnostics
        .iter()
        .filter_map(|d| d.missing_file.as_deref())
        .chain(latexmk.missing_files.iter().map(|s| s.as_str()));
    for name in names {
        if is_package_file(name) && !missing.iter().any(|m| m == name) {
            missing.push(name.to_string());
        }
    }
    missing
}

// ---------------------------------------------------------------------------------------------------
// String helpers (Swift `trimmingCharacters(in: .whitespaces)` etc.)
// ---------------------------------------------------------------------------------------------------

/// Unicode "Space separator" (Zs) plus tab: `CharacterSet.whitespaces`.
fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

fn trim_ws(s: &str) -> &str {
    s.trim_matches(is_ws)
}

fn trim_leading_spaces(s: &str) -> &str {
    s.trim_start_matches([' ', '\t'])
}

fn trim_trailing_spaces(s: &str) -> &str {
    s.trim_end_matches([' ', '\t'])
}

fn is_blank(line: &str) -> bool {
    line.bytes().all(|b| b == b' ' || b == b'\t')
}

fn leading_digits(s: &str) -> &str {
    let n = s.bytes().take_while(|b| b.is_ascii_digit()).count();
    &s[..n]
}

/// Appends `part` to `text` separated by a space (no space when `text` is empty).
fn append_spaced(text: &mut String, part: &str) {
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(part);
}

/// Last path component and its extension (`NSString.pathExtension`).
fn path_extension(name: &str) -> &str {
    let last = name.rsplit('/').next().unwrap_or(name);
    match last.rfind('.') {
        Some(i) if i > 0 => &last[i + 1..],
        _ => "",
    }
}

// ---------------------------------------------------------------------------------------------------
// The parser
// ---------------------------------------------------------------------------------------------------

/// One entry of the TeX file stack.
enum StackEntry {
    File(String),
    Other,
}

struct MissingChars {
    font: String,
    chars: Vec<String>,
    file: Option<String>,
}

struct LogParser<'a, F: Fn(&str) -> Option<String>> {
    lines: Vec<&'a str>,
    map_path: F,
    stack: Vec<StackEntry>,
    diagnostics: Vec<LogDiagnostic>,
    map_cache: HashMap<String, Option<String>>,
    pending_runaway: Option<String>,
    last_closed_project_file: Option<String>,
    missing_characters: Vec<MissingChars>,
}

impl<'a, F: Fn(&str) -> Option<String>> LogParser<'a, F> {
    fn new(log: &'a str, map_path: F) -> Self {
        // Same lines as `split("\n", omittingEmptySubsequences: false)` with a trailing "\r" excluded.
        let lines: Vec<&'a str> = log.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
        LogParser {
            lines,
            map_path,
            stack: Vec::new(),
            diagnostics: Vec::new(),
            map_cache: HashMap::new(),
            pending_runaway: None,
            last_closed_project_file: None,
            missing_characters: Vec::new(),
        }
    }

    // MARK: Driver

    fn run(&mut self) {
        let mut i = 0;
        while i < self.lines.len() {
            i = self.handle_line(i);
        }
        self.flush_missing_characters();
    }

    /// Handles the line at `i` and returns the index of the next line to process.
    fn handle_line(&mut self, i: usize) -> usize {
        let line: &'a str = self.lines[i];
        let bytes = line.as_bytes();
        let Some(&first) = bytes.first() else { return i + 1 };

        if first == b'!' {
            return self.parse_error(i, None, None, trim_leading_spaces(&line[1..]));
        }
        if let Some((path, number, message)) = match_file_line_error(line) {
            return self.parse_error(i, Some(path), Some(number), message);
        }
        if first == b'L' || first == b'P' || first == b'C' {
            if let Some(header) = warning_header(line) {
                return self.parse_warning(i, header);
            }
        }
        if (first == b'O' || first == b'U') && (line.starts_with("Overfull \\") || line.starts_with("Underfull \\")) {
            return self.parse_bad_box(i);
        }
        if first == b'R' && line.starts_with("Runaway ") && bytes[bytes.len() - 1] == b'?' {
            // "Runaway argument?" / "Runaway definition?" / "Runaway text?" + the runaway tokens on the next line.
            if i + 1 < self.lines.len() {
                self.pending_runaway = Some(trim_ws(self.lines[i + 1]).to_string());
            }
            return i + 2;
        }
        if first == b'M' && line.starts_with("Missing character: There is no ") {
            self.record_missing_character(line);
            return i + 1;
        }
        // "pdfTeX warning", "LuaTeX warning", "XeTeX warning".
        if let Some(offset) = line.find("TeX warning") {
            let before = &line[..offset];
            let engine = ["pdf", "Lua", "Xe"].iter().find(|e| before.ends_with(**e));
            if let Some(engine) = engine {
                let engine_start = offset - engine.len();
                self.process_parentheses(&line[..engine_start]);
                return self.parse_engine_warning(i, &line[engine_start..]);
            }
        }
        self.process_parentheses(line);
        i + 1
    }

    // MARK: File stack

    fn mapped(&mut self, path: &str) -> Option<String> {
        if let Some(cached) = self.map_cache.get(path) {
            return cached.clone();
        }
        let url = (self.map_path)(path);
        self.map_cache.insert(path.to_string(), url.clone());
        url
    }

    /// The innermost file on the stack that belongs to the project; when no project file is open (e.g.
    /// "File ended while scanning use of ..." after the file was closed), the project file closed last.
    fn current_project_file(&mut self) -> Option<String> {
        for idx in (0..self.stack.len()).rev() {
            let path = match &self.stack[idx] {
                StackEntry::File(p) => p.clone(),
                StackEntry::Other => continue,
            };
            if let Some(url) = self.mapped(&path) {
                return Some(url);
            }
        }
        self.last_closed_project_file.clone()
    }

    /// The innermost file on the stack (project or not).
    fn current_file_path(&self) -> Option<String> {
        self.stack.iter().rev().find_map(|e| match e {
            StackEntry::File(p) => Some(p.clone()),
            StackEntry::Other => None,
        })
    }

    /// Updates the file stack from the `(` and `)` in an ordinary log line.
    fn process_parentheses(&mut self, line: &str) {
        let bytes = line.as_bytes();
        let mut k = 0;
        while k < bytes.len() {
            let c = bytes[k];
            if c == b'(' {
                if let Some((name, end)) = file_name_in(line, k + 1) {
                    self.stack.push(StackEntry::File(name));
                    k = end;
                    continue;
                }
                self.stack.push(StackEntry::Other);
            } else if c == b')' {
                if let Some(StackEntry::File(path)) = self.stack.pop() {
                    if let Some(url) = self.mapped(&path) {
                        self.last_closed_project_file = Some(url);
                    }
                }
            }
            k += 1;
        }
    }

    // MARK: Errors

    fn parse_error(&mut self, i: usize, file: Option<String>, line_number: Option<u32>, raw_message: &str) -> usize {
        let n = self.lines.len();
        let mut message = trim_ws(raw_message).to_string();
        let mut next = i + 1;
        // Multi-line package errors: "(fontspec)    The font ... cannot be found;"
        while next < n {
            let Some(cont) = continuation_text(self.lines[next]) else { break };
            append_spaced(&mut message, trim_ws(cont));
            next += 1;
        }
        if message.starts_with("==> Fatal error occurred") || message.is_empty() {
            self.pending_runaway = None;
            return next;
        }
        if message.starts_with("LaTeX Error: ") {
            message.drain(.."LaTeX Error: ".len());
        }
        // "Package fontspec Error: msg" -> "fontspec: msg" (same style as package warnings).
        if message.starts_with("Package ") || message.starts_with("Class ") {
            if let Some(pos) = message.find(" Error: ") {
                let words: Vec<&str> = message[..pos].split(' ').filter(|w| !w.is_empty()).collect();
                if words.len() == 2 {
                    message = format!("{}: {}", words[1], &message[pos + " Error: ".len()..]);
                }
            } else if message.ends_with(" Error:") {
                let words: Vec<&str> = message[..message.len() - " Error:".len()]
                    .split(' ')
                    .filter(|w| !w.is_empty())
                    .collect();
                if words.len() == 2 {
                    message = format!("{}:", words[1]);
                }
            }
        }
        let mut message_tail: Option<String> = None;
        if message.chars().count() > 120 {
            if let Some(cut) = sentence_break(&message) {
                message_tail = Some(trim_ws(&message[cut..]).to_string());
                message.truncate(cut);
            }
        }

        // Find the `l.N` context line before the next message.
        let mut context_line: Option<usize> = None;
        let mut j = next;
        while j < n && j < i + 40 {
            let l = self.lines[j];
            if l.starts_with("l.") && line_context_number(l).is_some() {
                context_line = Some(j);
                break;
            }
            if starts_message(l) {
                break;
            }
            j += 1;
        }
        let top_line: Option<&str> = if next < n && !is_blank(self.lines[next]) { Some(self.lines[next]) } else { None };

        let mut detail_lines: Vec<String> = Vec::new();
        if let Some(tail) = &message_tail {
            if !tail.is_empty() {
                detail_lines.push(tail.clone());
            }
        }
        if let Some(runaway) = &self.pending_runaway {
            detail_lines.push(format!("Runaway argument: {runaway}"));
        }
        let mut end: usize;
        let mut line_from_context: Option<u32> = None;
        let mut source_text: Option<&str> = None;
        if let Some(c) = context_line {
            let l = self.lines[c];
            line_from_context = line_context_number(l);
            source_text = line_context_text(l);
            // Context = from the first context line (skipping LaTeX's help preamble) to l.N + its continuation.
            let mut first_context = c;
            while first_context > next && !is_blank(self.lines[first_context - 1]) {
                first_context -= 1;
            }
            end = c + 1;
            // The continuation (rest of the source line) is indented; it may be all spaces.
            if end < n && self.lines[end].starts_with(' ') {
                end += 1;
            }
            for k in first_context..end {
                if !is_blank(self.lines[k]) {
                    detail_lines.push(trim_trailing_spaces(self.lines[k]).to_string());
                }
            }
        } else {
            end = next;
            while end < n && end < next + 8 && !is_blank(self.lines[end]) && !starts_message(self.lines[end]) {
                detail_lines.push(trim_trailing_spaces(self.lines[end]).to_string());
                end += 1;
            }
        }
        // Skip TeX's help text (ends with a blank line) so its parentheses do not touch the file stack.
        // TeX always writes that blank line; a line starting with `)` (a file being closed) also ends the
        // skip, so a truncated or hand-written log cannot lose a file-stack pop.
        if context_line.is_some() {
            let mut k = end;
            while k < n
                && k < end + 12
                && !self.lines[k].is_empty()
                && self.lines[k].as_bytes()[0] != b')'
                && !starts_message(self.lines[k])
            {
                k += 1;
            }
            end = k;
        }

        // Undefined control sequence: append the offending token (end of the top context line).
        if message.starts_with("Undefined control sequence") {
            let candidate = top_line
                .and_then(trailing_control_sequence)
                .or_else(|| source_text.and_then(trailing_control_sequence));
            message = "Undefined control sequence".to_string();
            if let Some(cs) = candidate {
                message.push(' ');
                message.push_str(&cs);
            }
        }

        // Where: file:line -> that file; else the innermost project file with the l.N line.
        let url: Option<String>;
        let mut line_no: Option<u32> = None;
        let mut detail_prefix: Option<String> = None;
        if let Some(f) = &file {
            if let Some(mapped_url) = self.mapped(f) {
                url = Some(mapped_url);
                line_no = line_number;
            } else {
                url = self.current_project_file();
                let shown = line_number.map(|v| v.to_string()).unwrap_or_else(|| "?".to_string());
                detail_prefix = Some(format!("{f}:{shown}"));
            }
        } else {
            let top = self.current_file_path();
            let top_mapped = top.as_deref().and_then(|t| self.mapped(t));
            if let Some(mapped_url) = top_mapped {
                url = Some(mapped_url);
                line_no = line_from_context;
            } else {
                url = self.current_project_file();
                if let (Some(t), Some(num)) = (&top, line_from_context) {
                    detail_prefix = Some(format!("{t}:{num}"));
                }
            }
        }
        let mut column = None;
        let mut length = None;
        if line_no.is_some() {
            if let Some((c, len)) = source_text.and_then(error_span) {
                column = Some(c);
                length = Some(len);
            }
        }
        if let Some(prefix) = detail_prefix {
            detail_lines.insert(0, prefix);
        }

        let missing = missing_file(&message);
        self.diagnostics.push(LogDiagnostic {
            severity: Severity::Error,
            source: Source::Tex,
            message,
            detail: if detail_lines.is_empty() { None } else { Some(detail_lines.join("\n")) },
            path: url,
            line: line_no,
            column,
            length,
            missing_file: missing,
        });
        self.pending_runaway = None;
        end.max(i + 1)
    }

    // MARK: Warnings

    fn parse_warning(&mut self, i: usize, header_length: usize) -> usize {
        let n = self.lines.len();
        let line = self.lines[i];
        let header = trim_ws(&line[..header_length]);
        let mut text = trim_ws(&line[header_length..]).to_string();
        let mut next = i + 1;
        while next < n && next < i + 20 {
            let l = self.lines[next];
            if is_blank(l) || starts_message(l) {
                break;
            }
            if let Some(cont) = continuation_text(l) {
                append_spaced(&mut text, trim_ws(cont));
            } else if l.starts_with(' ') && is_warning_continuation(trim_leading_spaces(l)) {
                append_spaced(&mut text, trim_ws(l));
            } else {
                break;
            }
            next += 1;
        }
        let mut line_no: Option<u32> = None;
        if let Some(pos) = text.find(" on input line ") {
            let after = pos + " on input line ".len();
            let digits = leading_digits(&text[after..]);
            if let Ok(num) = digits.parse::<u32>() {
                line_no = Some(num);
                let end = after + digits.len();
                text.replace_range(pos..end, "");
            }
        }
        let mut message = text.clone();
        if header.starts_with("Package ") || header.starts_with("Class ") {
            // Keep the package name: "hyperref: Token not allowed in a PDF string (Unicode): removing `math shift'."
            let name = header.split(' ').filter(|w| !w.is_empty()).nth(1).unwrap_or("");
            if !name.is_empty() {
                message = format!("{name}: {text}");
            }
        }
        let severity = if is_informational(&text) { Severity::Info } else { Severity::Warning };
        let mut d = LogDiagnostic::new(severity, Source::Tex, message);
        d.path = self.current_project_file();
        d.line = line_no;
        self.diagnostics.push(d);
        next
    }

    /// pdfTeX/LuaTeX/XeTeX warnings: one line, optionally followed by `<...>` / `l.N` context.
    fn parse_engine_warning(&mut self, i: usize, text: &str) -> usize {
        let n = self.lines.len();
        let mut next = i + 1;
        let mut line_no: Option<u32> = None;
        let mut detail: Vec<String> = Vec::new();
        let mut j = next;
        while j < n && j < next + 6 {
            let l = self.lines[j];
            if l.starts_with('<') || l.starts_with(' ') {
                if !is_blank(l) {
                    detail.push(trim_trailing_spaces(l).to_string());
                }
                j += 1;
                continue;
            }
            if let Some(num) = line_context_number(l) {
                line_no = Some(num);
                detail.push(trim_trailing_spaces(l).to_string());
                next = j + 1;
                if next < n && self.lines[next].starts_with(' ') && !is_blank(self.lines[next]) {
                    next += 1;
                }
            }
            break;
        }
        let top_url = self.current_file_path().and_then(|top| self.mapped(&top));
        let file_url = match &top_url {
            Some(u) => Some(u.clone()),
            None => self.current_project_file(),
        };
        let use_line = line_no.is_some() && top_url.is_some();
        let mut d = LogDiagnostic::new(Severity::Warning, Source::Tex, trim_ws(text));
        d.detail = if detail.is_empty() { None } else { Some(detail.join("\n")) };
        d.path = file_url;
        d.line = if use_line { line_no } else { None };
        self.diagnostics.push(d);
        next
    }

    // MARK: Bad boxes

    fn parse_bad_box(&mut self, i: usize) -> usize {
        let n = self.lines.len();
        let mut text = trim_ws(self.lines[i]).to_string();
        let line_no: Option<u32> = if let Some(digits) = bad_box_lines_digits(&text) {
            digits.parse().ok()
        } else {
            bad_box_line_digits(&text).and_then(|d| d.parse().ok())
        };
        let inline_box = text.ends_with("[]");
        if inline_box {
            text = trim_ws(&text[..text.len() - 2]).to_string();
        }
        // Box display: optional blank line, then the box contents up to the next blank line.
        let mut next = i + 1;
        let mut detail: Vec<String> = Vec::new();
        if !inline_box {
            if next < n && is_blank(self.lines[next]) {
                next += 1;
            }
            let mut count = 0;
            while next < n && count < 6 && !is_blank(self.lines[next]) && !starts_message(self.lines[next]) {
                let l = self.lines[next];
                let trimmed = trim_leading_spaces(l);
                // Box displays start with "[]" or a font identifier ("\OT1/cmr/m/n/10 ...").
                if !(trimmed.starts_with("[]") || trimmed.starts_with('\\') || l.contains("[]")) {
                    break;
                }
                let t = trim_ws(l);
                if t != "[]" && !t.is_empty() {
                    detail.push(t.to_string());
                }
                next += 1;
                count += 1;
            }
        }
        let mut d = LogDiagnostic::new(Severity::BadBox, Source::Tex, text);
        d.detail = if detail.is_empty() { None } else { Some(detail.join("\n")) };
        d.path = self.current_project_file();
        d.line = line_no;
        self.diagnostics.push(d);
        next
    }

    // MARK: Missing characters

    fn record_missing_character(&mut self, line: &str) {
        // Missing character: There is no a ("E1) in font nullfont!
        // Missing character: There is no (U+2202) in font cmr10!
        let body = &line["Missing character: There is no ".len()..];
        let Some(in_font) = body.rfind(" in font ") else { return };
        let mut char_part = &body[..in_font];
        if let Some(paren) = char_part.rfind(" (") {
            char_part = &char_part[..paren];
        }
        let mut font = body[in_font + " in font ".len()..].to_string();
        if font.ends_with('!') {
            font.pop();
        }
        let file = self.current_project_file();
        let ch = char_part.to_string();
        if let Some(entry) = self.missing_characters.iter_mut().find(|e| e.font == font) {
            if !entry.chars.contains(&ch) {
                entry.chars.push(ch);
            }
        } else {
            self.missing_characters.push(MissingChars { font, chars: vec![ch], file });
        }
    }

    fn flush_missing_characters(&mut self) {
        for entry in std::mem::take(&mut self.missing_characters) {
            let list = entry.chars.iter().take(20).cloned().collect::<Vec<_>>().join(" ")
                + if entry.chars.len() > 20 { " …" } else { "" };
            let mut d = LogDiagnostic::new(
                Severity::Warning,
                Source::Tex,
                format!("Missing character(s) in font {}: {}", entry.font, list),
            );
            d.path = entry.file;
            self.diagnostics.push(d);
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// Token / line classification
// ---------------------------------------------------------------------------------------------------

/// Reads a file name that TeX printed right after `(` (at byte offset `start` of a line). Returns the
/// name and the offset just past it, or `None` when the token does not look like a file.
fn file_name_in(line: &str, start: usize) -> Option<(String, usize)> {
    let bytes = line.as_bytes();
    let count = bytes.len();
    if start >= count {
        return None;
    }
    if bytes[start] == b'"' {
        // ("./my file.tex"
        let close = start + 1 + bytes[start + 1..].iter().position(|&b| b == b'"')?;
        if !looks_like_file_bytes(bytes, start + 1, close) {
            return None;
        }
        return Some((line[start + 1..close].to_string(), close + 1));
    }
    // The run up to the next parenthesis (or end of line).
    let mut run_end = start;
    while run_end < count && bytes[run_end] != b'(' && bytes[run_end] != b')' {
        run_end += 1;
    }
    if run_end <= start || bytes[start] == b' ' {
        return None;
    }
    // TeX does not quote names containing spaces: take the longest prefix that ends (at a space or at
    // the end of the run) with a known file extension; otherwise the first word.
    let mut best: Option<usize> = None;
    let mut first_break: Option<usize> = None;
    for cursor in (start + 1)..=run_end {
        let is_break = cursor == run_end || bytes[cursor] == b' ' || bytes[cursor] == b'\t';
        if !is_break {
            continue;
        }
        if first_break.is_none() {
            first_break = Some(cursor);
        }
        if has_known_extension_bytes(bytes, start, cursor) {
            best = Some(cursor);
        }
    }
    let end = best.or(first_break).unwrap_or(run_end);
    if !looks_like_file_bytes(bytes, start, end) {
        return None;
    }
    Some((line[start..end].to_string(), end))
}

/// `looksLikeFile` on the bytes `[start, end)`.
fn looks_like_file_bytes(bytes: &[u8], start: usize, end: usize) -> bool {
    let length = end - start;
    if length == 0 {
        return false;
    }
    let first = bytes[start];
    let second = if length > 1 { bytes[start + 1] } else { 0 };
    let third = if length > 2 { bytes[start + 2] } else { 0 };
    let path_like = first == b'/'
        || (first == b'.' && second == b'/')
        || (first == b'.' && second == b'.' && third == b'/')
        || (first == b'~' && second == b'/');
    if path_like {
        return length > 2;
    }
    has_known_extension_bytes(bytes, start, end)
}

/// `hasKnownExtension` on the bytes `[start, end)`: the part after the last dot (1-8 ASCII characters,
/// no slash) is one of `FILE_EXTENSIONS` (case-insensitive).
fn has_known_extension_bytes(bytes: &[u8], start: usize, end: usize) -> bool {
    let mut dot = end as isize - 1;
    let limit = (start as isize + 1).max(end as isize - 9);
    while dot >= limit && bytes[dot as usize] != b'.' {
        if bytes[dot as usize] == b'/' {
            return false;
        }
        dot -= 1;
    }
    if dot < limit || (end as isize) - dot - 1 < 1 {
        return false;
    }
    let ext = &bytes[dot as usize + 1..end];
    let mut buf = [0u8; 9];
    if ext.len() > buf.len() {
        return false;
    }
    for (k, &c) in ext.iter().enumerate() {
        if c >= 0x80 {
            return false;
        }
        buf[k] = c.to_ascii_lowercase();
    }
    let lowered = &buf[..ext.len()];
    FILE_EXTENSIONS.iter().any(|e| e.as_bytes() == lowered)
}

fn looks_like_file(name: &str) -> bool {
    if name.starts_with("./") || name.starts_with("../") || name.starts_with('/') || name.starts_with("~/") {
        return name.chars().count() > 2;
    }
    has_known_extension(name)
}

fn has_known_extension(name: &str) -> bool {
    let Some(dot) = name.rfind('.') else { return false };
    if dot == 0 {
        return false;
    }
    let ext = &name[dot + 1..];
    if ext.is_empty() || ext.chars().count() > 8 || ext.contains('/') {
        return false;
    }
    let lowered = ext.to_lowercase();
    FILE_EXTENSIONS.contains(&lowered.as_str())
}

/// `path:123: message` (from -file-line-error). The path must look like a file.
fn match_file_line_error(line: &str) -> Option<(String, u32, &str)> {
    let bytes = line.as_bytes();
    let f = *bytes.first()?;
    if f == b' ' || f == b'(' || f == b'\\' {
        return None;
    }
    // Byte offsets of the first ":<digits>:" followed by a space or the end of the line.
    let mut from = 0;
    let mut found: Option<(usize, usize, u32)> = None;
    while from < bytes.len() {
        let Some(rel) = bytes[from..].iter().position(|&b| b == b':') else { break };
        let colon = from + rel;
        let mut d = colon + 1;
        let mut value: u32 = 0;
        let mut digits = 0;
        while d < bytes.len() && bytes[d].is_ascii_digit() {
            if digits < 9 {
                value = value * 10 + (bytes[d] - b'0') as u32;
            }
            digits += 1;
            d += 1;
        }
        if digits > 0 && d < bytes.len() && bytes[d] == b':' && (d + 1 == bytes.len() || bytes[d + 1] == b' ') {
            found = Some((colon, d + 1, value));
            break;
        }
        from = colon + 1;
    }
    let (colon, after_colon, value) = found?;
    let path = &line[..colon];
    if !(looks_like_file(path) || path.contains('/')) {
        return None;
    }
    let message = if after_colon >= bytes.len() { "" } else { &line[after_colon + 1..] };
    Some((path.to_string(), value, trim_leading_spaces(message)))
}

/// Length of the warning header ("LaTeX Warning: ", "Package hyperref Warning: ", ...) or `None`.
fn warning_header(line: &str) -> Option<usize> {
    for h in ["LaTeX Warning: ", "LaTeX Font Warning: ", "LaTeX3 Warning: "] {
        if line.starts_with(h) {
            return Some(h.len());
        }
    }
    if !(line.starts_with("Package ") || line.starts_with("Class ") || line.starts_with("LaTeX ")) {
        return None;
    }
    let head_end;
    let header_end;
    if let Some(offset) = line.find(" Warning: ") {
        head_end = offset;
        header_end = offset + " Warning: ".len();
    } else if line.ends_with(" Warning:") {
        head_end = line.len() - " Warning:".len();
        header_end = line.len();
    } else {
        return None;
    }
    // "Package <name> Warning:" - the name is a single word (may contain dots/dashes).
    let words = line[..head_end].split(' ').filter(|w| !w.is_empty()).count();
    if words != 2 {
        return None;
    }
    Some(header_end)
}

/// Lines that start a new message (used to bound look-ahead).
fn starts_message(line: &str) -> bool {
    let Some(&f) = line.as_bytes().first() else { return false };
    if f == b'!' {
        return true;
    }
    if match_file_line_error(line).is_some() {
        return true;
    }
    if warning_header(line).is_some() {
        return true;
    }
    line.starts_with("Overfull \\") || line.starts_with("Underfull \\") || line.starts_with("Runaway ")
}

/// `(fontspec)     text` -> "text"; `None` if the line is not a `(name)`-prefixed continuation.
fn continuation_text(line: &str) -> Option<&str> {
    if !line.starts_with('(') {
        return None;
    }
    let close = line.find(')')?;
    let name = &line[1..close];
    if name.is_empty() || name.contains(' ') || name.chars().count() > 40 {
        return None;
    }
    Some(trim_leading_spaces(&line[close + 1..]))
}

/// An indented line after a warning that continues it (`^^J\@spaces...` in LaTeX's warnings), as opposed
/// to TeX's own output resuming: a file being opened `(...` or a page number `[12`.
fn is_warning_continuation(trimmed: &str) -> bool {
    let Some(first) = trimmed.chars().next() else { return false };
    if first == '(' {
        return false;
    }
    if first == '[' {
        let next = trimmed[1..].chars().next();
        return !next.is_some_and(|c| c.is_ascii_digit());
    }
    true
}

fn is_informational(text: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "Label(s) may have changed",
        "Rerun",
        "rerun",
        "There were undefined references",
        "There were undefined citations",
        "There were multiply-defined labels",
        "Some font shapes were not available",
        "Size substitutions with differences",
        "Citation(s) may have changed",
        "Please (re)run",
        "Writing file",
        "Tab has been converted",
    ];
    PATTERNS.iter().any(|p| text.contains(p))
}

/// Byte index just past the first "; " or ". " after the first 20 characters (to split long messages).
fn sentence_break(message: &str) -> Option<usize> {
    let (search_start, _) = message.char_indices().nth(20)?;
    let hay = &message[search_start..];
    let a = hay.find("; ");
    let b = hay.find(". ");
    let first = match (a, b) {
        (Some(x), Some(y)) => x.min(y),
        (Some(x), None) => x,
        (None, Some(y)) => y,
        (None, None) => return None,
    };
    Some(search_start + first + 1)
}

fn line_context_number(line: &str) -> Option<u32> {
    let rest = line.strip_prefix("l.")?;
    let digits = leading_digits(rest);
    if digits.is_empty() {
        return None;
    }
    let after = &rest[digits.len()..];
    if !(after.is_empty() || after.starts_with(' ')) {
        return None;
    }
    digits.parse().ok()
}

/// The source text TeX shows in `l.N <text>` (up to the error point).
fn line_context_text(line: &str) -> Option<&str> {
    line_context_number(line)?;
    let rest = &line[2..];
    let digits = leading_digits(rest);
    let mut rest = &rest[digits.len()..];
    if let Some(r) = rest.strip_prefix(' ') {
        rest = r;
    }
    if let Some(r) = rest.strip_suffix("^^M") {
        rest = r;
    }
    Some(rest)
}

/// `\foo` at the end of `text` (ignoring trailing spaces), or `None`.
fn trailing_control_sequence(text: &str) -> Option<String> {
    let t = text.trim_end_matches(' ');
    if t.is_empty() {
        return None;
    }
    let chars: Vec<char> = t.chars().collect();
    let is_letter = |c: char| c.is_ascii_alphabetic() || c == '@';
    let mut k = chars.len() as isize - 1;
    if is_letter(chars[k as usize]) {
        while k >= 0 && is_letter(chars[k as usize]) {
            k -= 1;
        }
        if k < 0 || chars[k as usize] != '\\' {
            return None;
        }
        return Some(chars[k as usize..].iter().collect());
    }
    if k >= 1 && chars[k as usize - 1] == '\\' {
        return Some(format!("\\{}", chars[k as usize]));
    }
    None
}

/// 1-based UTF-16 column and length of the last token TeX read on the error line: a control word, a
/// whole `\cmd[..]{..}` group when the text ends with `}`, a word, or a single character. `None` when
/// TeX truncated the context ("...").
fn error_span(text: &str) -> Option<(u32, u32)> {
    if text.starts_with("...") || text.is_empty() {
        return None;
    }
    let u: Vec<u16> = text.encode_utf16().collect();
    let mut end = u.len();
    while end > 0 && u[end - 1] == 0x20 {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    let is_letter = |c: u16| (0x61..=0x7A).contains(&c) || (0x41..=0x5A).contains(&c) || c == 0x40;
    const BACKSLASH: u16 = 0x5C;
    let mut start = end - 1;
    let last = u[end - 1];
    if last == 0x7D {
        // "}"
        let mut depth = 0i32;
        let mut k = end as isize - 1;
        let mut open: Option<usize> = None;
        while k >= 0 {
            let ku = k as usize;
            let escaped = ku > 0 && u[ku - 1] == BACKSLASH;
            if u[ku] == 0x7D && !escaped {
                depth += 1;
            } else if u[ku] == 0x7B && !escaped {
                depth -= 1;
                if depth == 0 {
                    open = Some(ku);
                    break;
                }
            }
            k -= 1;
        }
        if let Some(open) = open {
            start = open;
            // Optional arguments and a star between the command and the brace.
            let mut p = open;
            while p > 0 && u[p - 1] == 0x5D {
                // "]"
                let Some(o) = u[..p - 1].iter().rposition(|&c| c == 0x5B) else { break };
                p = o;
            }
            if p > 0 && u[p - 1] == 0x2A {
                p -= 1; // "*"
            }
            let mut q = p;
            while q > 0 && is_letter(u[q - 1]) {
                q -= 1;
            }
            if q < p && q > 0 && u[q - 1] == BACKSLASH {
                start = q - 1;
            }
        }
    } else if is_letter(last) {
        let mut k = end - 1;
        while k > 0 && is_letter(u[k - 1]) {
            k -= 1;
        }
        start = if k > 0 && u[k - 1] == BACKSLASH { k - 1 } else { k };
    } else if end >= 2 && u[end - 2] == BACKSLASH {
        start = end - 2;
    }
    Some(((start + 1) as u32, (end - start) as u32))
}

/// The missing file named by an error message, if any.
fn missing_file(message: &str) -> Option<String> {
    // LaTeX Error: File `x.sty' not found.  /  Package pdftex.def Error: File `a.pdf' not found: ...
    if let Some(r) = message.find("File `") {
        let after = r + "File `".len();
        if let Some(rel) = message[after..].find('\'') {
            let close = after + rel;
            if message[close..].starts_with("' not found") {
                return Some(message[after..close].to_string());
            }
        }
    }
    // I can't find file `x'.
    if let Some(r) = message.find("I can't find file `") {
        let after = r + "I can't find file `".len();
        if let Some(rel) = message[after..].find('\'') {
            return Some(message[after..after + rel].to_string());
        }
    }
    // Font \x=ecrm1000 at 10.0pt not loadable: Metric (TFM) file not found.
    if message.ends_with("Metric (TFM) file not found.") {
        if let Some(eq) = message.find('=') {
            let rest = &message[eq + 1..];
            let name = &rest[..rest.find(' ').unwrap_or(rest.len())];
            if !name.is_empty() {
                return Some(format!("{name}.tfm"));
            }
        }
    }
    None
}

/// Digits of the first `at lines (\d+)--(\d+)` match.
fn bad_box_lines_digits(text: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(p) = text[from..].find("at lines ") {
        let s = from + p + "at lines ".len();
        let d1 = leading_digits(&text[s..]);
        if !d1.is_empty() {
            if let Some(r2) = text[s + d1.len()..].strip_prefix("--") {
                if !leading_digits(r2).is_empty() {
                    return Some(d1);
                }
            }
        }
        from += p + 1;
    }
    None
}

/// Digits of the first `at line (\d+)` match.
fn bad_box_line_digits(text: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(p) = text[from..].find("at line ") {
        let s = from + p + "at line ".len();
        let d = leading_digits(&text[s..]);
        if !d.is_empty() {
            return Some(d);
        }
        from += p + 1;
    }
    None
}

// ---------------------------------------------------------------------------------------------------
// BibTeX / biber logs (.blg)
// ---------------------------------------------------------------------------------------------------

fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect()
}

/// Parses BibTeX `.blg` / biber `.blg` output.
///
/// BibTeX: `... ---line N of file F` (errors), `Warning--...` optionally followed by `--line N of file F`,
/// `I couldn't open database file F`, `I couldn't open style file F`, `... ---while reading file F`.
/// Biber: `[ms] Module.pm:N> WARN|ERROR - message`, with `BibTeX subsystem: <tmp>/refs.bib_123.utf8, line N, ...`
/// mapped back to the project's `refs.bib`. `source` is used for every diagnostic (pass `Bibtex` or `Biber`;
/// the format is detected from the content either way).
pub fn parse_bibliography_log<F: Fn(&str) -> Option<String>>(text: &str, source: Source, map_path: F) -> Vec<LogDiagnostic> {
    let lines = split_lines(text);
    let is_biber = lines
        .iter()
        .take(5)
        .any(|l| l.contains("> INFO - This is Biber") || l.starts_with("INFO - This is Biber"))
        || (source == Source::Biber && !lines.iter().take(2).any(|l| l.starts_with("This is BibTeX")));
    let result = if is_biber {
        parse_biber(&lines, source, &map_path)
    } else {
        parse_bibtex(&lines, source, &map_path)
    };
    normalized(result)
}

fn parse_bibtex<F: Fn(&str) -> Option<String>>(lines: &[&str], source: Source, map_path: &F) -> Vec<LogDiagnostic> {
    let mut result: Vec<LogDiagnostic> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(r) = line.find("---line ") {
            // I was expecting a `,' or a `}'---line 11 of file refs.bib
            let message = trim_ws(&line[..r]);
            let (line_no, file) = parse_line_of_file(&line[r + "---line ".len()..]);
            let mut detail: Vec<String> = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && j < i + 8 {
                let l = lines[j];
                if l.starts_with(" : ")
                    || l == " :"
                    || l.starts_with("(Error may have been")
                    || l.starts_with("I'm skipping whatever remains")
                {
                    detail.push(trim_trailing_spaces(l).to_string());
                    j += 1;
                } else {
                    break;
                }
            }
            let mut d = LogDiagnostic::new(Severity::Error, source, if message.is_empty() { line } else { message });
            d.detail = if detail.is_empty() { None } else { Some(detail.join("\n")) };
            d.path = file.and_then(|f| map_path(&f));
            d.line = line_no;
            result.push(d);
            i = j;
            continue;
        }
        if let Some(rest) = line.strip_prefix("Warning--") {
            let message = trim_ws(rest);
            let mut line_no = None;
            let mut file = None;
            if i + 1 < lines.len() {
                if let Some(rest) = lines[i + 1].strip_prefix("--line ") {
                    (line_no, file) = parse_line_of_file(rest);
                    i += 1;
                }
            }
            let mut d = LogDiagnostic::new(Severity::Warning, source, message);
            d.path = file.and_then(|f| map_path(&f));
            d.line = line_no;
            result.push(d);
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("I couldn't open database file ") {
            let mut d = LogDiagnostic::new(Severity::Error, source, line);
            d.missing_file = Some(trim_ws(rest).to_string());
            result.push(d);
        } else if let Some(rest) = line.strip_prefix("I couldn't open style file ") {
            let mut d = LogDiagnostic::new(Severity::Error, source, line);
            d.missing_file = Some(trim_ws(rest).to_string());
            result.push(d);
        } else if let Some(r) = line.find("---while reading file ") {
            // I found no \citation commands---while reading file main.aux
            let message = &line[..r];
            let file = trim_ws(&line[r + "---while reading file ".len()..]);
            let mut d = LogDiagnostic::new(Severity::Error, source, message);
            d.path = map_path(file);
            result.push(d);
        } else if line.starts_with("I couldn't open file name ") || line.starts_with("I couldn't open auxiliary file") {
            result.push(LogDiagnostic::new(Severity::Error, source, line));
        }
        i += 1;
    }
    result
}

/// "11 of file refs.bib" -> (11, "refs.bib").
fn parse_line_of_file(text: &str) -> (Option<u32>, Option<String>) {
    let line_no = leading_digits(text).parse::<u32>().ok();
    let Some(r) = text.find(" of file ") else { return (line_no, None) };
    let file = trim_ws(&text[r + " of file ".len()..]);
    (line_no, if file.is_empty() { None } else { Some(file.to_string()) })
}

fn parse_biber<F: Fn(&str) -> Option<String>>(lines: &[&str], source: Source, map_path: &F) -> Vec<LogDiagnostic> {
    let mut result: Vec<LogDiagnostic> = Vec::new();
    for line in lines {
        // [123] Utils.pm:410> WARN - message
        let body = match line.find("> ") {
            Some(r) => &line[r + 2..],
            None => line,
        };
        let (severity, message) = if let Some(m) = body.strip_prefix("WARN - ") {
            (Severity::Warning, m)
        } else if let Some(m) = body.strip_prefix("ERROR - ") {
            (Severity::Error, m)
        } else {
            continue;
        };
        let mut text = trim_ws(message).to_string();
        let mut file_url: Option<String> = None;
        let mut line_no: Option<u32> = None;
        // BibTeX subsystem: /tmp/biber_tmp_x/refs.bib_12345.utf8, line 8, syntax error: ...
        if let Some(r) = text.find("BibTeX subsystem: ") {
            let rest = text[r + "BibTeX subsystem: ".len()..].to_string();
            if let Some(comma) = rest.find(',') {
                let tmp_path = &rest[..comma];
                file_url = map_path(&original_bib_name_from_biber_temp(tmp_path));
                let after_path = trim_leading_spaces(&rest[comma + 1..]);
                if let Some(after_line) = after_path.strip_prefix("line ") {
                    line_no = leading_digits(after_line).parse::<u32>().ok();
                    if let Some(comma2) = after_path.find(',') {
                        text = format!("BibTeX subsystem: {}", trim_ws(&after_path[comma2 + 1..]));
                    }
                }
            }
        } else if let Some(name) = bib_name_in_parentheses(&text) {
            // Datamodel: article entry 'x' (refs.bib): Missing mandatory field 'journal'
            file_url = map_path(name);
        }
        let mut d = LogDiagnostic::new(severity, source, text);
        d.path = file_url;
        d.line = line_no;
        result.push(d);
    }
    result
}

/// First match of `\(([^()]+\.bib)\)`: the text between the parentheses.
fn bib_name_in_parentheses(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut s = 0;
    while s < bytes.len() {
        if bytes[s] == b'(' {
            let e = bytes[s + 1..].iter().position(|&b| b == b'(' || b == b')').map(|p| s + 1 + p);
            if let Some(e) = e {
                if bytes[e] == b')' {
                    let inner = &text[s + 1..e];
                    if inner.len() > ".bib".len() && inner.ends_with(".bib") {
                        return Some(inner);
                    }
                }
            }
        }
        s += 1;
    }
    None
}

/// biber copies data sources to a temporary "<name>_<pid>.utf8" file; recover "<name>".
fn original_bib_name_from_biber_temp(path: &str) -> String {
    let mut name = path.trim_end_matches('/').rsplit('/').next().unwrap_or(path);
    if let Some(n) = name.strip_suffix(".utf8") {
        name = n;
    }
    if let Some(u) = name.rfind('_') {
        if name[u + 1..].bytes().all(|b| b.is_ascii_digit()) && u != 0 {
            name = &name[..u];
        }
    }
    name.to_string()
}

// ---------------------------------------------------------------------------------------------------
// latexmk output
// ---------------------------------------------------------------------------------------------------

/// Parses latexmk's stdout/stderr: missing input files and bibliography/index tools that failed to run
/// (no `.blg`/`.ilg` to parse), plus fatal `xdvipdfmx` errors.
pub fn parse_latexmk_output(text: &str) -> LatexmkOutputSummary {
    const TOOLS: [&str; 3] = ["biber", "bibtex", "makeindex"];
    let mut summary = LatexmkOutputSummary::default();
    let mut tool_lines: HashMap<&str, Vec<String>> = HashMap::new();
    let mut failed_tools: Vec<(&str, String)> = Vec::new();
    let mut in_error_summary = false;
    for line in split_lines(text) {
        if let Some(name) = missing_input_file_name(line) {
            if !summary.missing_files.iter().any(|m| m == name) {
                summary.missing_files.push(name.to_string());
            }
            continue;
        }
        for tool in TOOLS {
            if line.strip_prefix(tool).is_some_and(|r| r.starts_with(": ") || r.starts_with(" error")) {
                tool_lines.entry(tool).or_default().push(line.to_string());
            }
        }
        if line.starts_with("Collected error summary") {
            in_error_summary = true;
            continue;
        }
        if in_error_summary {
            // "  biber /path/main: Could not open biber log file '/path/main.blg'"
            let trimmed = trim_leading_spaces(line);
            if trimmed.is_empty() {
                continue;
            }
            for tool in TOOLS {
                if trimmed.strip_prefix(tool).is_some_and(|r| r.starts_with(' ')) {
                    if let Some(colon) = trimmed.find(": ") {
                        let message = &trimmed[colon + 2..];
                        if message.contains("Could not open")
                            || (message.contains("gave return code") && !message.contains("Bibtex errors"))
                        {
                            failed_tools.push((tool, message.to_string()));
                        }
                    }
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("xdvipdfmx:fatal:") {
            summary.diagnostics.push(LogDiagnostic::new(Severity::Error, Source::Latexmk, trim_ws(rest)));
        }
    }
    for (tool, message) in failed_tools {
        let source = match tool {
            "biber" => Source::Biber,
            "bibtex" => Source::Bibtex,
            _ => Source::Makeindex,
        };
        let mut detail = tool_lines.get(tool).cloned().unwrap_or_default();
        detail.push(message);
        let text = match tool {
            "biber" => BIBER_FAILURE_MESSAGE,
            "bibtex" => "BibTeX failed to run; the bibliography was not generated.",
            _ => "makeindex failed to run; the index was not generated.",
        };
        let mut d = LogDiagnostic::new(Severity::Error, source, text);
        d.detail = Some(detail.join("\n"));
        summary.diagnostics.push(d);
    }
    summary
}

/// The quoted name of the first `Missing input file:? '([^']+)'` in the line.
fn missing_input_file_name(line: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(p) = line[from..].find("Missing input file") {
        let mut s = from + p + "Missing input file".len();
        if line[s..].starts_with(':') {
            s += 1;
        }
        if let Some(rest) = line[s..].strip_prefix(" '") {
            if let Some(close) = rest.find('\'') {
                if close > 0 {
                    return Some(&rest[..close]);
                }
            }
        }
        from += p + 1;
    }
    None
}

// ---------------------------------------------------------------------------------------------------
// Missing file -> line of the request (LaTeXCompiler.missingFileLine)
// ---------------------------------------------------------------------------------------------------

/// 1-based line of the `\usepackage{...}` / `\input{...}` (any brace group) that requests `name`
/// ("thispackagedoesnotexist.sty"; a `.sty`/`.cls`/`.tex` name also matches without its extension).
/// Comments (`%`) are ignored.
pub fn missing_file_line(text: &str, name: &str) -> Option<u32> {
    let mut candidates: Vec<String> = vec![name.to_string()];
    let ext = path_extension(name).to_ascii_lowercase();
    if ["sty", "cls", "tex"].contains(&ext.as_str()) {
        let stem = &name[..name.len() - ext.len() - 1];
        candidates.push(stem.to_string());
    }
    let mut line_number = 0u32;
    for line in split_newlines(text) {
        line_number += 1;
        let code = uncommented(line);
        let mut index = 0;
        while let Some(rel) = code[index..].find('{') {
            let open = index + rel;
            let Some(rel_close) = code[open..].find('}') else { break };
            let close = open + rel_close;
            let hit = code[open + 1..close]
                .split(',')
                .filter(|s| !s.is_empty())
                .map(trim_ws)
                .any(|item| candidates.iter().any(|c| c == item));
            if hit {
                return Some(line_number);
            }
            index = close + 1;
        }
    }
    None
}

/// Splits at Unicode newlines (`\n`, `\r`, `\r\n`, VT, FF, NEL, LS, PS), keeping empty lines.
fn split_newlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(c, '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            out.push(&text[start..i]);
            start = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(_, '\n')) = chars.peek() {
                    chars.next();
                    start += 1;
                }
            }
        }
    }
    out.push(&text[start..]);
    out
}

/// The part of a source line before its first unescaped `%`.
fn uncommented(line: &str) -> &str {
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if c == '%' && !escaped {
            return &line[..i];
        }
        escaped = if c == '\\' { !escaped } else { false };
    }
    line
}

// ---------------------------------------------------------------------------------------------------
// Shadow workspace -> project path mapping (ShadowPathMapper, LaTeXCompiler.projectFile(forLogPath:))
// ---------------------------------------------------------------------------------------------------

fn has_drive_prefix(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'/' || b[2] == b'\\')
}

fn is_absolute(p: &str) -> bool {
    p.starts_with('/') || has_drive_prefix(p)
}

/// An absolute path with `.`, `..` and empty components removed purely textually (no symlink resolution).
/// Drive-letter paths (`C:\x\y`) keep their drive and are returned with `/` separators.
pub fn lexically_standardized(path: &str) -> String {
    let (prefix, rest, backslashes) = if has_drive_prefix(path) {
        (&path[..2], &path[2..], true)
    } else {
        ("", path, false)
    };
    let mut parts: Vec<&str> = Vec::new();
    let rest_owned;
    let rest = if backslashes {
        rest_owned = rest.replace('\\', "/");
        rest_owned.as_str()
    } else {
        rest
    };
    for component in rest.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    format!("{}/{}", prefix, parts.join("/"))
}

/// `path` relative to `root` ("" for the root itself), tolerating the /private/var <-> /var alias.
fn relative_path_under(path: &str, root: &str) -> Option<String> {
    let candidates = [root.to_string(), format!("/private{root}"), root.replace("/private/", "/")];
    for candidate in &candidates {
        if path == candidate || path.strip_prefix(candidate.as_str()).is_some_and(|r| r.starts_with('/')) {
            return Some(path[candidate.len()..].trim_matches('/').to_string());
        }
    }
    None
}

fn join_path(base: &str, rel: &str) -> String {
    if rel.is_empty() {
        base.to_string()
    } else if base.ends_with('/') {
        format!("{base}{rel}")
    } else {
        format!("{base}/{rel}")
    }
}

/// Maps paths between the user's project directory and the private shadow workspace the compiler runs in.
/// The shadow also mirrors a few folders *above* the project (the ancestor layer), so that
/// `\input{../macros}` compiles; paths in that layer map back to the real files outside the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowPathMapper {
    pub project_directory: String,
    pub shadow_directory: String,
    /// The real folder mirrored by the shadow's ancestor layer (an ancestor of `project_directory`).
    pub ancestor_directory: Option<String>,
    /// Where `ancestor_directory` is mirrored in the shadow (an ancestor of `shadow_directory`).
    pub ancestor_shadow_directory: Option<String>,
}

impl ShadowPathMapper {
    pub fn new(project_directory: &str, shadow_directory: &str) -> Self {
        ShadowPathMapper {
            project_directory: lexically_standardized(project_directory),
            shadow_directory: lexically_standardized(shadow_directory),
            ancestor_directory: None,
            ancestor_shadow_directory: None,
        }
    }

    /// A mapper that also translates the ancestor layer: `ancestor_shadow/<rel>` <-> `ancestor/<rel>`.
    pub fn with_ancestor(project_directory: &str, shadow_directory: &str, ancestor: &str, ancestor_shadow: &str) -> Self {
        let mut m = Self::new(project_directory, shadow_directory);
        m.ancestor_directory = Some(lexically_standardized(ancestor));
        m.ancestor_shadow_directory = Some(lexically_standardized(ancestor_shadow));
        m
    }

    /// Maps a path as written by TeX/SyncTeX (absolute, or relative to the shadow directory, possibly
    /// starting with "./") back to the project — or, for a file reached with `../` (ancestor layer), to
    /// the real file outside the project. `None` for files outside the shadow (e.g. TeX Live files).
    pub fn project_path(&self, shadow_path: &str) -> Option<String> {
        let shadow_root = &self.shadow_directory;
        let p = if is_absolute(shadow_path) {
            shadow_path.to_string()
        } else {
            let rel = shadow_path.strip_prefix("./").unwrap_or(shadow_path);
            format!("{shadow_root}/{rel}")
        };
        // Lexical: resolving `..` through the file system would walk through the mirror's symlinks.
        let standardized = lexically_standardized(&p);
        if let Some(rel) = relative_path_under(&standardized, shadow_root) {
            return Some(lexically_standardized(&join_path(&self.project_directory, &rel)));
        }
        if let (Some(anc), Some(anc_shadow)) = (&self.ancestor_directory, &self.ancestor_shadow_directory) {
            if let Some(rel) = relative_path_under(&standardized, anc_shadow) {
                return Some(lexically_standardized(&join_path(anc, &rel)));
            }
        }
        None
    }

    /// The shadow path corresponding to a project file (or to a file of the ancestor layer).
    pub fn shadow_path(&self, project_path: &str) -> String {
        let std = lexically_standardized(project_path);
        let root = &self.project_directory;
        if let Some(rel) = std.strip_prefix(&format!("{root}/")) {
            return join_path(&self.shadow_directory, rel);
        }
        if let (Some(anc), Some(anc_shadow)) = (&self.ancestor_directory, &self.ancestor_shadow_directory) {
            let base = if anc == "/" { "" } else { anc.as_str() };
            if let Some(rel) = std.strip_prefix(&format!("{base}/")) {
                return join_path(anc_shadow, rel);
            }
        }
        project_path.to_string()
    }
}

/// The project's files, for mapping log paths to their on-disk spelling: relative path (with `/`) ->
/// absolute project path. Lookups fall back to a case-insensitive match (TeX on a case-insensitive
/// volume opens `\input{Chapters/Intro}` as `Chapters/Intro.tex` although the file is
/// `chapters/intro.tex`). Spellings that collide case-insensitively are left out of the folded index.
#[derive(Debug, Clone, Default)]
pub struct ProjectFiles {
    exact: HashMap<String, String>,
    folded: HashMap<String, String>,
}

impl ProjectFiles {
    /// `files` are `(relative path, absolute path)` pairs.
    pub fn new<I: IntoIterator<Item = (String, String)>>(files: I) -> Self {
        let exact: HashMap<String, String> = files.into_iter().collect();
        let mut folded: HashMap<String, String> = HashMap::new();
        let mut ambiguous: HashSet<String> = HashSet::new();
        for (key, url) in &exact {
            let f = key.to_lowercase();
            if folded.insert(f.clone(), url.clone()).is_some() {
                ambiguous.insert(f);
            }
        }
        for f in ambiguous {
            folded.remove(&f);
        }
        ProjectFiles { exact, folded }
    }
}

fn is_existing_file(path: &str) -> bool {
    std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

/// Maps a path from the TeX log (relative to the compile cwd, or absolute) to the project file with its
/// on-disk spelling — or to the real file outside the project for inputs reached with `../`. `None` for
/// TeX Live files, files in out/ and anything that does not exist. (`LaTeXCompiler.projectFile(forLogPath:)`;
/// without Unicode NFC normalisation of names.)
pub fn project_file_for_log_path(
    raw: &str,
    cwd: &str,
    mapper: &ShadowPathMapper,
    project_files: &ProjectFiles,
) -> Option<String> {
    let mut path = raw;
    if path.len() >= 2 && path.starts_with('"') && path.ends_with('"') {
        path = &path[1..path.len() - 1];
    }
    let path = if is_absolute(path) { path.to_string() } else { format!("{cwd}/{path}") };
    let full = mapper.project_path(&path)?;
    let project_path = &mapper.project_directory;
    if let Some(relative) = full.strip_prefix(&format!("{project_path}/")) {
        if let Some(known) = project_files.exact.get(relative) {
            return Some(known.clone());
        }
        if let Some(m) = project_files.folded.get(&relative.to_lowercase()) {
            return Some(m.clone());
        }
        // Below a symlinked project folder (mirrored as a single link, so not listed).
        return is_existing_file(&full).then_some(full);
    }
    // Outside the project: the ancestor layer (`\input{../macros}`).
    is_existing_file(&full).then_some(full)
}

/// Convenience: a `map_path` closure for [`parse`] / [`parse_bibliography_log`] given the shadow
/// directory (the compile cwd, where relative log paths start) and the project root. Files are accepted
/// when they exist on disk in project space (no snapshot of unsaved buffers; use
/// [`project_file_for_log_path`] with a [`ProjectFiles`] for that).
pub fn shadow_path_mapper(shadow_dir: &str, project_root: &str) -> impl Fn(&str) -> Option<String> {
    let mapper = ShadowPathMapper::new(project_root, shadow_dir);
    let files = ProjectFiles::default();
    let cwd = mapper.shadow_directory.clone();
    move |raw: &str| project_file_for_log_path(raw, &cwd, &mapper, &files)
}

// ---------------------------------------------------------------------------------------------------
// Tests (ports of Sources/TeXToolchainChecks/ParserChecks.swift, UnitChecks.swift, RegressionChecks.swift)
// ---------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/TeXToolchain")
    }

    fn fixture_log(name: &str) -> String {
        let path = fixtures_root().join("logs").join(name);
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("missing fixture {}: {e}", path.display()));
        String::from_utf8_lossy(&data).into_owned()
    }

    fn project(name: &str) -> String {
        lexically_standardized(&fixtures_root().join("projects").join(name).to_string_lossy())
    }

    /// Fixtures.mapper: relative paths map into the fixture project when the file exists.
    fn mapper(project_name: &str) -> impl Fn(&str) -> Option<String> {
        let dir = project(project_name);
        move |path: &str| {
            if path.starts_with('/') {
                return None;
            }
            let p = path.strip_prefix("./").unwrap_or(path);
            let full = lexically_standardized(&format!("{dir}/{p}"));
            std::path::Path::new(&full).exists().then_some(full)
        }
    }

    fn describe_one(d: &LogDiagnostic) -> String {
        let file = d.path.as_deref().map(|p| p.rsplit('/').next().unwrap_or(p)).unwrap_or("-");
        format!(
            "[{:?} {:?}] {}:{}:{} {}",
            d.severity,
            d.source,
            file,
            d.line.map(|l| l.to_string()).unwrap_or("?".into()),
            d.column.map(|l| l.to_string()).unwrap_or("?".into()),
            d.message
        )
    }

    fn describe(ds: &[LogDiagnostic]) -> String {
        ds.iter().map(describe_one).collect::<Vec<_>>().join("\n      ")
    }

    /// `expectDiagnostic`: exactly one diagnostic with this message; checks severity, source, file, line
    /// and (when given) column, length and missing file.
    struct Expect<'a> {
        message: &'a str,
        severity: Severity,
        file: Option<&'a str>,
        line: Option<u32>,
        column: Option<Option<u32>>,
        length: Option<Option<u32>>,
        source: Source,
        missing_file: Option<Option<&'a str>>,
    }

    fn expect<'a>(message: &'a str, severity: Severity, file: Option<&'a str>, line: Option<u32>) -> Expect<'a> {
        Expect { message, severity, file, line, column: None, length: None, source: Source::Tex, missing_file: None }
    }

    impl<'a> Expect<'a> {
        fn col(mut self, column: u32, length: u32) -> Self {
            self.column = Some(Some(column));
            self.length = Some(Some(length));
            self
        }
        fn source(mut self, s: Source) -> Self {
            self.source = s;
            self
        }
        fn missing(mut self, f: &'a str) -> Self {
            self.missing_file = Some(Some(f));
            self
        }
        fn check(self, ds: &[LogDiagnostic]) -> &LogDiagnostic {
            let matches: Vec<&LogDiagnostic> = ds.iter().filter(|d| d.message == self.message).collect();
            assert_eq!(
                matches.len(),
                1,
                "expected exactly one diagnostic “{}”, found {} in:\n      {}",
                self.message,
                matches.len(),
                describe(ds)
            );
            let d = matches[0];
            assert_eq!(d.severity, self.severity, "severity of “{}”", self.message);
            assert_eq!(d.source, self.source, "source of “{}”", self.message);
            assert_eq!(d.path.as_deref(), self.file, "file of “{}”", self.message);
            assert_eq!(d.line, self.line, "line of “{}”", self.message);
            if let Some(c) = self.column {
                assert_eq!(d.column, c, "column of “{}”", self.message);
            }
            if let Some(l) = self.length {
                assert_eq!(d.length, l, "length of “{}”", self.message);
            }
            if let Some(m) = self.missing_file {
                assert_eq!(d.missing_file.as_deref(), m, "missingFile of “{}”", self.message);
            }
            d
        }
    }

    fn count_errors(ds: &[LogDiagnostic]) -> usize {
        ds.iter().filter(|d| d.severity == Severity::Error).count()
    }

    #[test]
    fn multi_file_project() {
        let dir = project("multifile");
        let main = format!("{dir}/main.tex");
        let intro = format!("{dir}/sections/intro.tex");
        let theory = format!("{dir}/sections/theory.tex");
        let lemma = format!("{dir}/sections/sub/lemma.tex");
        let ds = parse(&fixture_log("multifile.log"), mapper("multifile"));

        let undefined = expect("Undefined control sequence \\undefinedmacro", Severity::Error, Some(&intro), Some(3))
            .col(19, 15)
            .check(&ds);
        assert!(
            undefined.detail.as_deref().is_some_and(|d| d.contains("l.3 More text with an \\undefinedmacro")),
            "detail has the l.N context"
        );
        expect("Environment lemma undefined.", Severity::Error, Some(&lemma), Some(1)).col(1, 13).check(&ds);
        expect("\\begin{document} ended by \\end{lemma}.", Severity::Error, Some(&lemma), Some(3)).col(1, 11).check(&ds);
        let missing_dollar: Vec<&LogDiagnostic> = ds.iter().filter(|d| d.message == "Missing $ inserted.").collect();
        assert_eq!(missing_dollar.iter().map(|d| d.line).collect::<Vec<_>>(), vec![Some(4), Some(5)]);
        assert!(missing_dollar.iter().all(|d| d.path.as_deref() == Some(theory.as_str()) && d.severity == Severity::Error));
        assert_eq!(missing_dollar[0].column, Some(18), "\\alpha column");
        assert_eq!(missing_dollar[0].length, Some(6), "\\alpha length");
        assert_eq!(count_errors(&ds), 5, "exactly five errors:\n      {}", describe(&ds));

        expect("Unused global option(s): [nonexistentoption].", Severity::Warning, Some(&main), None).check(&ds);
        expect("Label `sec:space' multiply defined.", Severity::Warning, Some(&main), None).check(&ds);
        expect(
            "hyperref: Token not allowed in a PDF string (Unicode): removing `math shift'.",
            Severity::Warning,
            Some(&main),
            Some(11),
        )
        .check(&ds);
        expect("Reference `sec:missing' on page 1 undefined.", Severity::Warning, Some(&main), Some(12)).check(&ds);
        expect("Citation `nokey' on page 1 undefined.", Severity::Warning, Some(&main), Some(12)).check(&ds);
        expect(
            "Font shape `OT1/cmr/bx/sc' undefined using `OT1/cmr/bx/n' instead.",
            Severity::Warning,
            Some(&main),
            Some(13),
        )
        .check(&ds);
        expect("Some font shapes were not available, defaults substituted.", Severity::Info, Some(&main), None).check(&ds);
        expect("There were undefined references.", Severity::Info, Some(&main), None).check(&ds);
        expect("There were multiply-defined labels.", Severity::Info, Some(&main), None).check(&ds);
        assert_eq!(ds.len(), 14, "no unexpected diagnostics:\n      {}", describe(&ds));
        // Sorted: errors first, then by path and line.
        let order: Vec<&str> =
            ds.iter().take(5).map(|d| d.path.as_deref().and_then(|p| p.rsplit('/').next()).unwrap_or("-")).collect();
        assert_eq!(order, vec!["intro.tex", "lemma.tex", "lemma.tex", "theory.tex", "theory.tex"], "error order");
        assert!(ds.iter().all(|d| d.path.is_some()), "every diagnostic is attributed to a project file");
    }

    #[test]
    fn runaway_argument() {
        let main = format!("{}/main.tex", project("runaway"));
        let ds = parse(&fixture_log("runaway.log"), mapper("runaway"));
        let d = expect("File ended while scanning use of \\textbf .", Severity::Error, Some(&main), None).check(&ds);
        assert!(
            d.detail.as_deref().is_some_and(|x| x.contains("Runaway argument: {world \\par Next paragraph.")),
            "runaway text in detail: {:?}",
            d.detail
        );
        assert_eq!(count_errors(&ds), 1, "Emergency stop / fatal error are dropped:\n      {}", describe(&ds));
    }

    #[test]
    fn missing_package() {
        let main = format!("{}/main.tex", project("missingpkg"));
        let ds = parse(&fixture_log("missingpkg.log"), mapper("missingpkg"));
        expect("File `thispackagedoesnotexist.sty' not found.", Severity::Error, Some(&main), None)
            .missing("thispackagedoesnotexist.sty")
            .check(&ds);
        assert_eq!(count_errors(&ds), 1, "{}", describe(&ds));
        let summary = parse_latexmk_output(&fixture_log("missingpkg.latexmk"));
        assert_eq!(summary.missing_files, vec!["thispackagedoesnotexist.sty"]);
        assert!(summary.diagnostics.is_empty(), "no tool failure in missingpkg.latexmk");
        // missingPackages = package-ish missing files of the diagnostics and latexmk, unique.
        assert_eq!(missing_package_files(&ds, &summary), vec!["thispackagedoesnotexist.sty"]);
    }

    #[test]
    fn missing_graphics_and_input() {
        let main = format!("{}/main.tex", project("missinginput"));
        let ds = parse(&fixture_log("missinginput.log"), mapper("missinginput"));
        expect("File `figs/plot' not found.", Severity::Error, Some(&main), Some(5))
            .col(1, 27)
            .missing("figs/plot")
            .check(&ds);
        expect("File `chapters/nothere.tex' not found.", Severity::Error, Some(&main), None)
            .missing("chapters/nothere.tex")
            .check(&ds);
        assert_eq!(count_errors(&ds), 2, "{}", describe(&ds));
        assert_eq!(
            parse_latexmk_output(&fixture_log("missinginput.latexmk")).missing_files,
            vec!["figs/plot", "chapters/nothere.tex"]
        );
    }

    #[test]
    fn over_and_underfull_boxes() {
        let main = format!("{}/main.tex", project("badboxes"));
        let ds = parse(&fixture_log("badboxes.log"), mapper("badboxes"));
        expect("Underfull \\hbox (badness 10000) in paragraph at lines 6--8", Severity::BadBox, Some(&main), Some(6))
            .check(&ds);
        let narrow =
            expect("Overfull \\hbox (146.60309pt too wide) detected at line 9", Severity::BadBox, Some(&main), Some(9))
                .check(&ds);
        assert_eq!(narrow.detail.as_deref(), Some("\\OT1/cmr/m/n/10 This box is far too narrow for its contents"));
        expect(
            "Overfull \\vbox (109.99832pt too high) has occurred while \\output is active",
            Severity::BadBox,
            Some(&main),
            None,
        )
        .check(&ds);
        expect("Overfull \\hbox (186.91743pt too wide) in paragraph at lines 13--16", Severity::BadBox, Some(&main), Some(13))
            .check(&ds);
        expect(
            "Font shape `OT1/cmr/m/n' in size <5.5> not available size <5> substituted.",
            Severity::Warning,
            Some(&main),
            Some(17),
        )
        .check(&ds);
        expect("Size substitutions with differences up to 0.5pt have occurred.", Severity::Info, Some(&main), None)
            .check(&ds);
        assert_eq!(ds.len(), 6, "{}", describe(&ds));
    }

    #[test]
    fn non_ascii_file_names_and_columns() {
        let chapter = format!("{}/Seções/capítulo 1.tex", project("unicode"));
        let ds = parse(&fixture_log("unicode.log"), mapper("unicode"));
        expect("Undefined control sequence \\comandoinexistente", Severity::Error, Some(&chapter), Some(2))
            .col(33, 19)
            .check(&ds);
        assert_eq!(ds.len(), 1, "{}", describe(&ds));
    }

    #[test]
    fn xelatex_fontspec_error_and_missing_characters() {
        let main = format!("{}/main.tex", project("xefont"));
        let ds = parse(&fixture_log("xefont.log"), mapper("xefont"));
        let font = expect("fontspec: The font \"NoSuchFontAnywhere\" cannot be found;", Severity::Error, Some(&main), Some(5))
            .check(&ds);
        assert!(
            font.detail.as_deref().is_some_and(|d| d.contains("this may be but usually is not a fontspec bug")),
            "long message continues in detail"
        );
        assert!(
            ds.iter().any(|d| d.message.starts_with("Missing character(s) in font nullfont:") && d.severity == Severity::Warning),
            "missing characters are summarized:\n      {}",
            describe(&ds)
        );
    }

    #[test]
    fn bibtex_log_and_blg() {
        let dir = project("bibtex");
        let main = format!("{dir}/main.tex");
        let ds = parse(&fixture_log("bibtex.log"), mapper("bibtex"));
        expect("Citation `nokey' on page 1 undefined.", Severity::Warning, Some(&main), Some(3)).check(&ds);
        expect("Reference `eq:nolabel' on page 1 undefined.", Severity::Warning, Some(&main), Some(4)).check(&ds);
        expect("There were undefined references.", Severity::Info, Some(&main), None).check(&ds);
        assert_eq!(ds.len(), 3, "{}", describe(&ds));

        let blg = parse_bibliography_log(&fixture_log("bibtex.blg"), Source::Bibtex, mapper("bibtex"));
        let refs = format!("{dir}/refs.bib");
        let syntax = expect("I was expecting a `,' or a `}'", Severity::Error, Some(&refs), Some(11))
            .source(Source::Bibtex)
            .check(&blg);
        assert!(
            syntax.detail.as_deref().is_some_and(|d| d.contains("(Error may have been on previous line)")),
            "bibtex context in detail"
        );
        expect("I didn't find a database entry for \"nokey\"", Severity::Warning, None, None)
            .source(Source::Bibtex)
            .check(&blg);
        assert_eq!(blg.len(), 2, "{}", describe(&blg));
        assert!(
            parse_latexmk_output(&fixture_log("bibtex.latexmk")).diagnostics.is_empty(),
            "“Bibtex errors: See file” is not a tool failure (the .blg has the details)"
        );
    }

    #[test]
    fn biblatex_biber() {
        let dir = project("biber");
        let main = format!("{dir}/main.tex");
        let ds = parse(&fixture_log("biber.log"), mapper("biber"));
        expect("Citation 'hatcher' on page 1 undefined.", Severity::Warning, Some(&main), Some(5)).check(&ds);
        expect("Empty bibliography.", Severity::Warning, Some(&main), Some(6)).check(&ds);
        expect(
            "biblatex: Please (re)run Biber on the file: main and rerun LaTeX afterwards.",
            Severity::Info,
            Some(&main),
            None,
        )
        .check(&ds);

        // biber's .blg format.
        let blg = parse_bibliography_log(&fixture_log("biber-format.blg"), Source::Biber, mapper("biber"));
        let refs = format!("{dir}/refs.bib");
        expect(
            "BibTeX subsystem: syntax error: found \"entry\", expected end of entry (\"}\" or \")\") (skipping to next \"@\")",
            Severity::Error,
            Some(&refs),
            Some(8),
        )
        .source(Source::Biber)
        .check(&blg);
        expect(
            "Datamodel: book entry 'hatcher' (refs.bib): Invalid field 'publisher' for entrytype 'book'",
            Severity::Warning,
            Some(&refs),
            None,
        )
        .source(Source::Biber)
        .check(&blg);
        expect("I didn't find a database entry for 'nokey' (section 0)", Severity::Warning, None, None)
            .source(Source::Biber)
            .check(&blg);

        let summary = parse_latexmk_output(&fixture_log("biber.latexmk"));
        assert_eq!(summary.diagnostics.len(), 1, "one biber failure");
        let failure = &summary.diagnostics[0];
        assert_eq!(failure.source, Source::Biber);
        assert_eq!(failure.severity, Severity::Error);
        assert_eq!(failure.message, BIBER_FAILURE_MESSAGE);
        assert!(
            failure.detail.as_deref().is_some_and(|d| d.contains("extracting arm64 binary with lipo failed")),
            "biber's own error in detail"
        );
    }

    #[test]
    fn edge_cases() {
        let map = |p: &str| p.strip_prefix("./").map(|rest| format!("/p/{rest}"));
        // Unbalanced parentheses inside error context must not corrupt the file stack.
        let log = "(./main.tex (./a.tex\n./a.tex:2: Undefined control sequence.\nl.2 text ( \\foo\n                 ) more\n)\nLaTeX Warning: Reference `x' on page 1 undefined on input line 7.\n\n)";
        let ds = parse(log, map);
        expect("Undefined control sequence \\foo", Severity::Error, Some("/p/a.tex"), Some(2)).check(&ds);
        expect("Reference `x' on page 1 undefined.", Severity::Warning, Some("/p/main.tex"), Some(7)).check(&ds);
        // CRLF logs and \MessageBreak continuations.
        let crlf = "(./main.tex\r\nPackage foo Warning: first part\r\n(foo)                second part on input line 3.\r\n\r\n)\r\n";
        let w = parse(crlf, map);
        expect("foo: first part second part.", Severity::Warning, Some("/p/main.tex"), Some(3)).check(&w);
        // Duplicates collapse.
        let mut d = LogDiagnostic::new(Severity::Warning, Source::Tex, "m");
        d.line = Some(1);
        let dup = normalized(vec![d.clone(), d]);
        assert_eq!(dup.len(), 1);
    }

    // ---- Further checks of behaviour the Swift code has but ParserChecks does not pin down. ----

    #[test]
    fn emergency_stop_dropped_only_with_specific_error() {
        let mut stop = LogDiagnostic::new(Severity::Error, Source::Tex, "Emergency stop.");
        stop.line = Some(1);
        let only = normalized(vec![stop.clone()]);
        assert_eq!(only.len(), 1, "kept when it is the only error");
        let mut other = LogDiagnostic::new(Severity::Error, Source::Tex, "Missing $ inserted.");
        other.line = Some(2);
        let both = normalized(vec![stop, other]);
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].message, "Missing $ inserted.");
    }

    #[test]
    fn sorting_none_last_and_severity_first() {
        let mk = |sev, path: Option<&str>, line: Option<u32>, msg: &str| {
            let mut d = LogDiagnostic::new(sev, Source::Tex, msg);
            d.path = path.map(String::from);
            d.line = line;
            d
        };
        let sorted = normalized(vec![
            mk(Severity::Warning, Some("/a"), Some(1), "w"),
            mk(Severity::Error, None, None, "e-none"),
            mk(Severity::Error, Some("/b"), None, "e-b-none"),
            mk(Severity::Error, Some("/b"), Some(2), "e-b-2"),
            mk(Severity::Error, Some("/a"), Some(9), "e-a-9"),
            mk(Severity::BadBox, Some("/a"), Some(1), "bb"),
            mk(Severity::Info, Some("/a"), Some(1), "i"),
        ]);
        let order: Vec<&str> = sorted.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(order, vec!["e-a-9", "e-b-2", "e-b-none", "e-none", "w", "bb", "i"]);
    }

    #[test]
    fn file_line_error_outside_project_uses_detail_prefix() {
        let map = |p: &str| p.strip_prefix("./").map(|rest| format!("/p/{rest}"));
        let log = "(./main.tex\n/usr/share/texlive/foo.sty:12: Undefined control sequence.\nl.4 \\foo\n\n)";
        let ds = parse(log, map);
        let d = expect("Undefined control sequence \\foo", Severity::Error, Some("/p/main.tex"), None).check(&ds);
        assert!(d.detail.as_deref().unwrap().starts_with("/usr/share/texlive/foo.sty:12\n"), "{:?}", d.detail);
    }

    #[test]
    fn package_error_headers_and_long_messages() {
        let map = |p: &str| p.strip_prefix("./").map(|rest| format!("/p/{rest}"));
        let log = "(./main.tex\n./main.tex:3: Package foo Error: Something broke.\n\nSee the foo package documentation for explanation.\nType  H <return>  for immediate help.\n ...                                              \n                                                  \nl.3 \\usepackage{foo}\n                  \n\n)";
        let ds = parse(log, map);
        assert_eq!(ds.len(), 1, "{}", describe(&ds));
        assert_eq!(ds[0].message, "foo: Something broke.");
        assert_eq!(ds[0].line, Some(3));
        // Long messages are cut after the first sentence break past 20 characters; the tail goes to the detail.
        let long = format!("! LaTeX Error: Unknown option for package. {}\n\nl.7 x\n\n", "z".repeat(130));
        let ds = parse(&format!("(./main.tex\n{long})"), map);
        assert_eq!(ds[0].message, "Unknown option for package.");
        assert!(ds[0].detail.as_deref().unwrap().starts_with("zzzz"), "{:?}", ds[0].detail);
    }

    #[test]
    fn engine_warning_with_context_line() {
        let map = |p: &str| p.strip_prefix("./").map(|rest| format!("/p/{rest}"));
        let log = "(./main.tex\npdfTeX warning (ext4): destination with the same identifier (name{page.1}) has been already used\n<to be read again> \nl.9 text\n\n)";
        let ds = parse(log, map);
        assert_eq!(ds.len(), 1, "{}", describe(&ds));
        assert_eq!(ds[0].severity, Severity::Warning);
        assert_eq!(ds[0].path.as_deref(), Some("/p/main.tex"));
        assert!(ds[0].message.starts_with("pdfTeX warning (ext4)"));
        let ds = parse("(./main.tex\n(\"./x.tex\" pdfTeX warning: hi\n)\n)", map);
        assert_eq!(ds[0].message, "pdfTeX warning: hi");
    }

    #[test]
    fn missing_characters_are_grouped_per_font() {
        let map = |p: &str| p.strip_prefix("./").map(|rest| format!("/p/{rest}"));
        let mut log = String::from("(./main.tex\n");
        for k in 0..25 {
            log.push_str(&format!("Missing character: There is no {} (U+{:04X}) in font cmr10!\n", char::from_u32(0x2200 + k).unwrap(), 0x2200 + k));
        }
        log.push_str("Missing character: There is no ∀ (U+2200) in font cmr10!\n)");
        let ds = parse(&log, map);
        assert_eq!(ds.len(), 1);
        assert!(ds[0].message.starts_with("Missing character(s) in font cmr10: ∀ ∁"), "{}", ds[0].message);
        assert!(ds[0].message.ends_with(" …"));
        assert_eq!(ds[0].path.as_deref(), Some("/p/main.tex"));
    }

    #[test]
    fn file_names_with_spaces_and_quotes_and_wrapped_stack() {
        let map = |p: &str| p.strip_prefix("./").map(|rest| format!("/p/{rest}"));
        let log = "(./main.tex (./my chapter.tex\nLaTeX Warning: Label `a' multiply defined.\n\n) (\"./quoted file.tex\"\nLaTeX Warning: Label `b' multiply defined.\n\n))";
        let ds = parse(log, map);
        let a = expect("Label `a' multiply defined.", Severity::Warning, Some("/p/my chapter.tex"), None).check(&ds);
        let _ = a;
        expect("Label `b' multiply defined.", Severity::Warning, Some("/p/quoted file.tex"), None).check(&ds);
    }

    #[test]
    fn error_span_cases() {
        assert_eq!(error_span("More text with an \\undefinedmacro"), Some((19, 15)));
        assert_eq!(error_span("...truncated"), None);
        assert_eq!(error_span("x \\cmd*[opt]{arg {nested}}"), Some((3, 24)));
        // A star after the brackets stops the scan (the Swift order of checks).
        assert_eq!(error_span("x \\cmd[opt]*{arg}"), Some((13, 5)));
        assert_eq!(error_span("price 5\\$"), Some((8, 2)));
        assert_eq!(error_span("plain word"), Some((7, 4)));
        assert_eq!(error_span("ação \\foo "), Some((6, 4)));
        assert_eq!(trailing_control_sequence("text \\foo@bar  "), Some("\\foo@bar".into()));
        assert_eq!(trailing_control_sequence("text foo"), None);
        assert_eq!(trailing_control_sequence("a \\%"), Some("\\%".into()));
    }

    #[test]
    fn helper_matchers() {
        assert_eq!(missing_file("File `x.sty' not found."), Some("x.sty".into()));
        assert_eq!(missing_file("I can't find file `foo'."), Some("foo".into()));
        assert_eq!(
            missing_file("Font \\x=ecrm1000 at 10.0pt not loadable: Metric (TFM) file not found."),
            Some("ecrm1000.tfm".into())
        );
        assert_eq!(missing_file("Missing $ inserted."), None);
        assert_eq!(line_context_number("l.12 abc"), Some(12));
        assert_eq!(line_context_number("l.12abc"), None);
        assert_eq!(line_context_text("l.3 text^^M"), Some("text"));
        assert!(warning_header("Package hyperref Warning: x").is_some());
        assert!(warning_header("Package a b Warning: x").is_none());
        assert!(warning_header("Class memoir Warning:").is_some());
        assert_eq!(warning_header("LaTeX Font Warning: x"), Some("LaTeX Font Warning: ".len()));
        let m = match_file_line_error("./a.tex:2: Undefined control sequence.").unwrap();
        assert_eq!((m.0.as_str(), m.1, m.2), ("./a.tex", 2, "Undefined control sequence."));
        assert!(match_file_line_error("Package: amsmath 2025/07/09 v2.17z AMS math features").is_none());
        assert!(match_file_line_error("foo:3: x").is_none(), "not a file");
        assert_eq!(original_bib_name_from_biber_temp("/tmp/biber_tmp_x/refs.bib_12345.utf8"), "refs.bib");
        assert_eq!(bib_name_in_parentheses("entry 'x' (refs.bib): Missing"), Some("refs.bib"));
        assert_eq!(bib_name_in_parentheses("(a (b.bib)"), Some("b.bib"));
        assert_eq!(bib_name_in_parentheses("(.bib)"), None);
    }

    // ---- LaTeXCompiler helpers and ShadowPathMapper (UnitChecks, RegressionChecks #1/#3/#5) ----

    #[test]
    fn package_files_and_missing_file_line() {
        assert!(is_package_file("tikz-cd.sty") && is_package_file("ecrm1000.tfm"));
        assert!(!is_package_file("figs/plot") && !is_package_file("chapters/nothere.tex") && !is_package_file("nothere.tex"));
        assert_eq!(
            missing_file_line(
                "\\documentclass{article}\n\\usepackage{amsmath}\n\\usepackage{thispackagedoesnotexist}\n",
                "thispackagedoesnotexist.sty"
            ),
            Some(3)
        );
        assert_eq!(missing_file_line("a\n\\input{chapters/nothere}\n", "chapters/nothere.tex"), Some(2));
        assert_eq!(missing_file_line("\\usepackage{amsmath}\n", "amsmath-other.sty"), None);
        assert_eq!(missing_file_line("% \\usepackage{foo}\n\\usepackage{a, foo}\n", "foo.sty"), Some(2));
    }

    #[test]
    fn shadow_path_mapper() {
        let mapper = ShadowPathMapper::new("/Users/g/Proj", "/Users/g/Library/Caches/S/abc/src");
        assert_eq!(
            mapper.project_path("/Users/g/Library/Caches/S/abc/src/./sections/a.tex").as_deref(),
            Some("/Users/g/Proj/sections/a.tex")
        );
        assert_eq!(mapper.project_path("./main.tex").as_deref(), Some("/Users/g/Proj/main.tex"));
        assert_eq!(mapper.project_path("/usr/local/texlive/2026/texmf-dist/tex/latex/base/article.cls"), None);
        assert_eq!(mapper.shadow_path("/Users/g/Proj/sections/a.tex"), "/Users/g/Library/Caches/S/abc/src/sections/a.tex");
        assert_eq!(mapper.ancestor_directory, None, "the two-argument constructor has no ancestor layer");
        assert_eq!(lexically_standardized("/a/./b/../c//d/"), "/a/c/d");
        assert_eq!(lexically_standardized("/../x"), "/x");
        let layered = ShadowPathMapper::with_ancestor(
            "/Users/g/Math/papers/p1",
            "/C/abc/tree/Math/papers/p1",
            "/Users/g",
            "/C/abc/tree",
        );
        assert_eq!(layered.project_path("/C/abc/tree/Math/papers/p1/./a.tex").as_deref(), Some("/Users/g/Math/papers/p1/a.tex"));
        assert_eq!(
            layered.project_path("/C/abc/tree/Math/papers/p1/../macros.tex").as_deref(),
            Some("/Users/g/Math/papers/macros.tex")
        );
        assert_eq!(layered.project_path("/private/C/abc/tree/shared/refs.bib").as_deref(), Some("/Users/g/shared/refs.bib"));
        assert_eq!(layered.project_path("/C/abc/out/main.aux"), None);
        assert_eq!(layered.shadow_path("/Users/g/Math/macros.tex"), "/C/abc/tree/Math/macros.tex");
        assert_eq!(layered.shadow_path("/Users/g/Math/papers/p1/a.tex"), "/C/abc/tree/Math/papers/p1/a.tex");
        assert_eq!(layered.shadow_path("/etc/hosts"), "/etc/hosts");
        // Windows drive letters.
        let win = ShadowPathMapper::new("C:\\Users\\g\\Proj", "C:\\Cache\\abc\\src");
        assert_eq!(win.project_path("C:\\Cache\\abc\\src\\.\\a.tex").as_deref(), Some("C:/Users/g/Proj/a.tex"));
        assert_eq!(win.project_path("./b.tex").as_deref(), Some("C:/Users/g/Proj/b.tex"));
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("gabetex-logparser-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
        fn path(&self) -> String {
            lexically_standardized(&self.0.to_string_lossy())
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn log_path_mapping_through_the_shadow() {
        let s = Scratch::new("mapping");
        let base = s.path();
        let project = format!("{base}/papers/paper1");
        let papers = format!("{base}/papers");
        let shadow = format!("{base}/shadow/tree/papers/paper1");
        std::fs::create_dir_all(format!("{project}/chapters")).unwrap();
        std::fs::write(format!("{project}/main.tex"), "x").unwrap();
        std::fs::write(format!("{project}/chapters/intro.tex"), "x").unwrap();
        std::fs::write(format!("{papers}/macros.tex"), "x").unwrap();
        let mapper = ShadowPathMapper::with_ancestor(&project, &shadow, &papers, &format!("{base}/shadow/tree/papers"));
        let files = ProjectFiles::new([
            ("main.tex".to_string(), format!("{project}/main.tex")),
            ("chapters/intro.tex".to_string(), format!("{project}/chapters/intro.tex")),
        ]);
        let map = |raw: &str| project_file_for_log_path(raw, &shadow, &mapper, &files);
        assert_eq!(map("./main.tex"), Some(format!("{project}/main.tex")));
        assert_eq!(map("\"./chapters/intro.tex\""), Some(format!("{project}/chapters/intro.tex")));
        // Case-insensitive spelling maps to the on-disk spelling (#5).
        assert_eq!(map("./Chapters/Intro.tex"), Some(format!("{project}/chapters/intro.tex")));
        // Ancestor layer: \input{../macros} maps to the real file outside the project (#3).
        assert_eq!(map("../macros.tex"), Some(format!("{papers}/macros.tex")));
        assert_eq!(map("../nothere.tex"), None, "must exist");
        // TeX Live files and out/ files are not project files.
        assert_eq!(map("/usr/local/texlive/2026/texmf-dist/tex/latex/base/article.cls"), None);
        assert_eq!(map(&format!("{base}/shadow/out/main.aux")), None);
        // Absolute shadow paths too; a symlink-less on-disk file below the project that is not listed.
        std::fs::write(format!("{project}/chapters/extra.tex"), "x").unwrap();
        assert_eq!(map(&format!("{shadow}/chapters/extra.tex")), Some(format!("{project}/chapters/extra.tex")));

        // The convenience mapper drives the parser end to end (diagnostic in a file below the project).
        let mapper_fn = super::shadow_path_mapper(&shadow, &project);
        let log = "(./main.tex (./chapters/intro.tex\n./chapters/intro.tex:2: Undefined control sequence.\nl.2 \\oopsondisk\n\n))";
        let ds = parse(log, mapper_fn);
        assert_eq!(ds.len(), 1, "{}", describe(&ds));
        assert_eq!(ds[0].path.as_deref(), Some(format!("{project}/chapters/intro.tex").as_str()));
        assert_eq!(ds[0].line, Some(2));
        assert_eq!(ds[0].message, "Undefined control sequence \\oopsondisk");
    }

    #[test]
    fn ipc_conversion() {
        let mut d = LogDiagnostic::new(Severity::BadBox, Source::Biber, "m");
        d.path = Some("/p/a.tex".into());
        d.line = Some(4);
        d.column = Some(2);
        let ipc = d.to_ipc();
        assert_eq!((ipc.severity.as_str(), ipc.source.as_str(), ipc.line, ipc.column), ("info", "bibtex", 4, Some(2)));
        let json = serde_json::to_value(&ipc).unwrap();
        assert_eq!(json["path"], "/p/a.tex");
        assert!(json.get("column").is_some());
        let e = LogDiagnostic::new(Severity::Error, Source::Tex, "e").to_ipc();
        assert_eq!((e.severity.as_str(), e.source.as_str(), e.path.as_str(), e.line), ("error", "latex", "", 0));
    }

    // ---- Performance ----

    #[test]
    fn performance_5mb_log() {
        let base = fixture_log("multifile.log");
        let mut big = String::new();
        while big.len() < 5 * 1_048_576 {
            big.push_str(&base);
        }
        let map = mapper("multifile");
        let _ = parse(&base, &map); // warm up
        let start = Instant::now();
        let result = parse(&big, &map);
        let elapsed = start.elapsed();
        println!(
            "5 MB log ({} bytes) parsed in {} ms, {} diagnostics (release={})",
            big.len(),
            elapsed.as_millis(),
            result.len(),
            !cfg!(debug_assertions)
        );
        // The 200 ms budget is for optimised builds; debug builds are 10-50x slower.
        let budget_ms = if cfg!(debug_assertions) { 5000 } else { 200 };
        assert!(elapsed.as_millis() < budget_ms, "parse took {} ms (budget {budget_ms} ms)", elapsed.as_millis());
        assert_eq!(result.len(), 14, "duplicates across repeated runs collapse");
    }

    #[test]
    fn performance_5mb_noisy_log() {
        // Many errors, warnings, boxes and file open/close tokens (all distinct -> nothing collapses).
        let mut big = String::from("(./main.tex\n");
        let mut k = 0u32;
        while big.len() < 5 * 1_048_576 {
            k += 1;
            big.push_str(&format!(
                "(./sec{k}.tex\n./sec{k}.tex:{k}: Undefined control sequence.\nl.{k} text \\foo{k}\n\nLaTeX Warning: Reference `r{k}' on page 1 undefined on input line {k}.\n\nOverfull \\hbox (1.5pt too wide) in paragraph at lines {k}--{k}\n[]\\OT1/cmr/m/n/10 abc\n\n)\n",
            ));
        }
        big.push_str(")\n");
        let map = |p: &str| p.strip_prefix("./").map(|rest| format!("/p/{rest}"));
        let start = Instant::now();
        let result = parse(&big, map);
        let elapsed = start.elapsed();
        println!(
            "5 MB noisy log ({} bytes) parsed in {} ms, {} diagnostics (release={})",
            big.len(),
            elapsed.as_millis(),
            result.len(),
            !cfg!(debug_assertions)
        );
        assert_eq!(result.len(), k as usize * 3);
        let budget_ms = if cfg!(debug_assertions) { 10000 } else { 400 };
        assert!(elapsed.as_millis() < budget_ms, "parse took {} ms (budget {budget_ms} ms)", elapsed.as_millis());
    }
}

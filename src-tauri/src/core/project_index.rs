//! Cross-file view of a LaTeX project, built from a root document by following includes (port of
//! `ProjectIndex.swift` plus the root-document picking of the app's `IndexBuilder`).
//!
//! The core is pure: all file contents come from a `read` closure and existence checks from `file_exists`
//! (the IPC entry point [`build_project_index`] passes unsaved buffer texts first, then the disk).
//!
//! Paths are plain strings. Internally they are *standardized*: `/`-separated (also on Windows), `.` / `..`
//! resolved. [`to_native`] converts back for the IPC (backslashes on Windows). On Windows, identity of a
//! file is case-insensitive (see [`path_key`]).

use crate::commands as ipc;
use crate::core::scanner::statistics::{self, DocumentStatistics};
use crate::core::scanner::{
    bib, latex_scanner as ls, masked::MaskedSource, models::*, references, walker,
};
use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

pub type ReadFn<'a> = &'a dyn Fn(&str) -> Option<String>;
pub type ExistsFn<'a> = &'a dyn Fn(&str) -> bool;

// ---------------------------------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------------------------------

fn slashes(path: &str) -> Cow<'_, str> {
    if cfg!(windows) && path.contains('\\') {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        Cow::Borrowed(path)
    }
}

/// Converts an internal (`/`) path to the platform's native spelling.
pub fn to_native(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.to_string()
    }
}

/// Key under which a path is compared: case-insensitive on Windows.
pub fn path_key(path: &str) -> Cow<'_, str> {
    if cfg!(windows) {
        Cow::Owned(path.to_lowercase())
    } else {
        Cow::Borrowed(path)
    }
}

fn drive_prefix_len(p: &str) -> usize {
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        2
    } else {
        0
    }
}

/// True for `/x`, and on Windows also `C:/x` and `//server/x`.
pub fn is_absolute(path: &str) -> bool {
    let p = slashes(path);
    if p.starts_with('/') {
        return true;
    }
    cfg!(windows) && drive_prefix_len(&p) == 2 && p[2..].starts_with('/')
}

/// Lexical standardization (like `URL.standardizedFileURL`): `/` separators, `.` and `..` resolved,
/// duplicate and trailing slashes removed.
pub fn standardize(path: &str) -> String {
    let p = slashes(path);
    // Windows: `\\?\C:\x` (verbatim prefix) is plain `C:/x`; `\\server\share` keeps its double slash.
    let p: Cow<str> = if cfg!(windows) && p.starts_with("//?/") { Cow::Owned(p[4..].to_string()) } else { p };
    let unc = cfg!(windows) && p.starts_with("//") && !p.starts_with("///");
    let drive = if cfg!(windows) { drive_prefix_len(&p) } else { 0 };
    let rest = &p[drive..];
    let absolute = rest.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for comp in rest.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if let Some(last) = parts.last() {
                    if *last != ".." {
                        parts.pop();
                        continue;
                    }
                }
                if !absolute {
                    parts.push("..");
                }
            }
            c => parts.push(c),
        }
    }
    let mut out = String::with_capacity(p.len());
    out.push_str(&p[..drive]);
    if unc {
        out.push('/');
    }
    if absolute {
        out.push('/');
    }
    out.push_str(&parts.join("/"));
    if out.is_empty() {
        out.push('.');
    }
    out
}

/// `base` + `path` standardized; an absolute `path` ignores `base`.
pub fn join(base: &str, path: &str) -> String {
    if is_absolute(path) {
        return standardize(path);
    }
    if base.is_empty() {
        return standardize(path);
    }
    standardize(&format!("{}/{}", base.trim_end_matches('/'), path))
}

/// Directory part of a standardized path (`/` for `/x`, `C:/` for `C:/x`).
pub fn parent_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => {
            let d = &path[..idx];
            if cfg!(windows) && d.len() == 2 && d.ends_with(':') {
                format!("{d}/")
            } else {
                d.to_string()
            }
        }
        None => String::new(),
    }
}

/// Last path component.
pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Lowercased extension without the dot (`""` when none; a leading dot is not an extension).
pub fn extension_lower(path: &str) -> String {
    let name = file_name(path);
    match name.rfind('.') {
        Some(idx) if idx > 0 && idx + 1 < name.len() => name[idx + 1..].to_lowercase(),
        _ => String::new(),
    }
}

fn is_tex_source(path: &str) -> bool {
    extension_lower(path) == "tex"
}

/// Path relative to `base` using `/`, or `None` when `path` is not inside `base`.
pub fn relative_path(path: &str, base: &str) -> Option<String> {
    let (p, b) = (standardize(path), standardize(base));
    let prefix = if b.ends_with('/') { b } else { format!("{b}/") };
    let (pk, bk) = (path_key(&p), path_key(&prefix));
    if pk.starts_with(bk.as_ref()) {
        Some(p[prefix.len()..].to_string())
    } else {
        None
    }
}

fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
        if !home.is_empty() {
            return format!("{}{}", home, &path[1..]);
        }
    }
    path.to_string()
}

/// Natural, case-insensitive ordering (like `localizedStandardCompare`): digit runs compare numerically.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => {
                if x.is_ascii_digit() && y.is_ascii_digit() {
                    let mut na = String::new();
                    while let Some(&c) = ai.peek() {
                        if c.is_ascii_digit() {
                            na.push(c);
                            ai.next();
                        } else {
                            break;
                        }
                    }
                    let mut nb = String::new();
                    while let Some(&c) = bi.peek() {
                        if c.is_ascii_digit() {
                            nb.push(c);
                            bi.next();
                        } else {
                            break;
                        }
                    }
                    let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                    let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                } else {
                    let (lx, ly) = (x.to_lowercase().next().unwrap_or(x), y.to_lowercase().next().unwrap_or(y));
                    if lx != ly {
                        return lx.cmp(&ly);
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// Text decoding (UTF-8 first, ISO Latin-1 for legacy files — like `TextFileIO`)
// ---------------------------------------------------------------------------------------------------

/// Files above this size are not read.
pub const MAX_TEXT_BYTES: usize = 24 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedText {
    pub text: String,
    /// "utf-8", "latin-1", "utf-16le" or "utf-16be".
    pub encoding: &'static str,
    pub has_bom: bool,
    /// Bytes that were not valid UTF-8 and were replaced with U+FFFD.
    pub invalid_byte_count: usize,
}

/// Counts well-formed multi-byte UTF-8 sequences and bytes that belong to no well-formed sequence.
pub fn scan_utf8(bytes: &[u8]) -> (usize, usize) {
    let (mut multi, mut invalid) = (0usize, 0usize);
    let mut i = 0;
    let count = bytes.len();
    while i < count {
        let lead = bytes[i];
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
            let byte = bytes[i + offset];
            let lower = if offset == 1 { low } else { 0x80 };
            let upper = if offset == 1 { high } else { 0xBF };
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

/// `None` when the data is binary (a NUL byte in the first 8 KB outside UTF-16) or cannot be decoded.
///
/// UTF-8 is tried first. Data that is not valid UTF-8 is read as ISO Latin-1 only when it contains no
/// well-formed multi-byte UTF-8 sequence at all (a real legacy file); a mostly-UTF-8 file with a few stray
/// bytes stays UTF-8, the bad bytes become U+FFFD and `invalid_byte_count` says how many.
pub fn decode_text(data: &[u8]) -> Option<DecodedText> {
    if data.is_empty() {
        return Some(DecodedText { text: String::new(), encoding: "utf-8", has_bom: false, invalid_byte_count: 0 });
    }
    let utf16 = |body: &[u8], le: bool| -> Option<String> {
        if body.len() % 2 != 0 {
            return None;
        }
        let units: Vec<u16> = body
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        let s = String::from_utf16(&units).ok()?;
        Some(s.trim_start_matches('\u{FEFF}').to_string())
    };
    if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xFE {
        if let Some(text) = utf16(&data[2..], true) {
            return Some(DecodedText { text, encoding: "utf-16le", has_bom: true, invalid_byte_count: 0 });
        }
    }
    if data.len() >= 2 && data[0] == 0xFE && data[1] == 0xFF {
        if let Some(text) = utf16(&data[2..], false) {
            return Some(DecodedText { text, encoding: "utf-16be", has_bom: true, invalid_byte_count: 0 });
        }
    }
    if data[..data.len().min(8192)].contains(&0) {
        return None;
    }
    let (body, bom) = if data.len() >= 3 && data[0] == 0xEF && data[1] == 0xBB && data[2] == 0xBF { (&data[3..], true) } else { (data, false) };
    if let Ok(s) = std::str::from_utf8(body) {
        return Some(DecodedText { text: s.to_string(), encoding: "utf-8", has_bom: bom, invalid_byte_count: 0 });
    }
    let (multi, invalid) = scan_utf8(body);
    if multi > 0 {
        return Some(DecodedText {
            text: String::from_utf8_lossy(body).into_owned(),
            encoding: "utf-8",
            has_bom: bom,
            invalid_byte_count: invalid.max(1),
        });
    }
    Some(DecodedText { text: data.iter().map(|&b| b as char).collect(), encoding: "latin-1", has_bom: false, invalid_byte_count: 0 })
}

/// Reads a text file from disk (UTF-8 / Latin-1 detection as in the macOS app). `None` when it cannot be
/// read, is binary or is too large.
pub fn read_text_file(path: &str) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() as usize > MAX_TEXT_BYTES {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    decode_text(&data).map(|d| d.text)
}

/// First 64 KB of a file as text (enough to find `\documentclass`).
fn read_head(path: &str) -> String {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else { return String::new() };
    let mut buf = Vec::new();
    if file.take(64 * 1024).read_to_end(&mut buf).is_err() {
        return String::new();
    }
    decode_text(&buf).map(|d| d.text).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------------------------------

/// Cross-file view of a LaTeX project (Swift `ProjectIndex`).
#[derive(Debug, Clone)]
pub struct ProjectModel {
    pub root_file: FilePath,
    /// .tex files reachable from the root, in document order (depth-first, root first). Cycles are ignored.
    pub files: Vec<FilePath>,
    /// Parent file -> directly included .tex files.
    pub include_graph: HashMap<FilePath, Vec<FilePath>>,
    /// Document outline across all files (headings nested by level; labels as leaves under the nearest heading).
    pub outline: Vec<OutlineNode>,
    /// The same entries as `outline`, flat and in document order (includes the labels).
    pub flat_outline: Vec<OutlineEntry>,
    pub labels: Vec<LabelDefinition>,
    pub bib_files: Vec<FilePath>,
    pub bib_entries: Vec<BibRecord>,
    pub custom_commands: Vec<ipc::CompletionItem>,
    pub custom_environments: Vec<String>,
    /// Magic comments of the root file.
    pub magic_comments: MagicComments,

    // Reference & citation tracking (input of the reference linter)
    pub label_sites: Vec<references::KeyOccurrence>,
    pub option_labels: Vec<references::KeyOccurrence>,
    pub reference_uses: Vec<references::KeyOccurrence>,
    pub citation_uses: Vec<references::KeyOccurrence>,
    pub bib_items: Vec<references::KeyOccurrence>,
    /// Prefixes of label keys that macros build at run time, sorted and unique.
    pub dynamic_label_prefixes: Vec<String>,
    pub uses_external_documents: bool,
    pub uses_external_citations: bool,
    /// A `\bibliography` / `\addbibresource` file could not be found or read, or is not a `.bib` file.
    pub has_unknown_bibliography_source: bool,
}

impl ProjectModel {
    pub fn new(root_file: &str) -> Self {
        let root: FilePath = FilePath::from(root_file);
        ProjectModel {
            root_file: root.clone(),
            files: vec![root],
            include_graph: HashMap::new(),
            outline: Vec::new(),
            flat_outline: Vec::new(),
            labels: Vec::new(),
            bib_files: Vec::new(),
            bib_entries: Vec::new(),
            custom_commands: Vec::new(),
            custom_environments: Vec::new(),
            magic_comments: MagicComments::default(),
            label_sites: Vec::new(),
            option_labels: Vec::new(),
            reference_uses: Vec::new(),
            citation_uses: Vec::new(),
            bib_items: Vec::new(),
            dynamic_label_prefixes: Vec::new(),
            uses_external_documents: false,
            uses_external_citations: false,
            has_unknown_bibliography_source: false,
        }
    }

    /// Symbols for completion. `project_files` = all files under the project directory (relative to the root
    /// file's dir).
    pub fn symbols(&self, project_files: Vec<String>) -> ProjectSymbols {
        ProjectSymbols {
            labels: self.labels.clone(),
            bib_entries: self.bib_entries.clone(),
            custom_commands: self.custom_commands.clone(),
            custom_environments: self.custom_environments.clone(),
            project_files,
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------------------------------

const IMAGE_EXTENSIONS: [&str; 5] = [".pdf", ".png", ".jpg", ".jpeg", ".eps"];

fn trim_ws(s: &str) -> &str {
    s.trim()
}

/// Resolves a reference found in `including_file` to an existing file path, applying TeX's rules.
/// `file_exists` should answer true for *regular files only*: `\input{chapter1}` tries `chapter1` before
/// `chapter1.tex`, so a directory with the same name would otherwise win.
pub fn resolve(reference: &IncludeReference, including_file: &str, root_file: &str, file_exists: ExistsFn) -> Option<String> {
    let root_dir = parent_dir(root_file);
    let including_dir = parent_dir(including_file);
    let argument = trim_ws(&reference.argument);
    if argument.is_empty() {
        return None;
    }

    let directory = reference.directory.as_deref().map(trim_ws).filter(|d| !d.is_empty());
    let base = match reference.kind {
        IncludeKind::Subfile => including_dir.clone(),
        IncludeKind::Subimport => match directory {
            Some(d) => join(&including_dir, d),
            None => including_dir.clone(),
        },
        IncludeKind::ImportFile => match directory {
            Some(d) => join(&root_dir, d),
            None => root_dir.clone(),
        },
        _ => root_dir.clone(),
    };
    let mut path: &str = argument;
    if path.starts_with("./") && path.chars().count() > 2 {
        path = &path[2..];
    }

    let candidates: Vec<String> = match reference.kind {
        IncludeKind::Input | IncludeKind::Subfile | IncludeKind::ImportFile | IncludeKind::Subimport => {
            vec![path.to_string(), format!("{path}.tex")]
        }
        IncludeKind::Include => {
            let mut c = vec![format!("{path}.tex")];
            if path.ends_with(".tex") {
                c.push(path.to_string());
            }
            c
        }
        IncludeKind::Bibliography => {
            let mut c = vec![format!("{path}.bib")];
            if path.ends_with(".bib") {
                c.insert(0, path.to_string());
            }
            c
        }
        IncludeKind::Addbibresource | IncludeKind::Lstinputlisting | IncludeKind::Inputminted => vec![path.to_string()],
        IncludeKind::Includegraphics | IncludeKind::Includepdf => {
            let mut c = vec![path.to_string()];
            c.extend(IMAGE_EXTENSIONS.iter().map(|e| format!("{path}{e}")));
            c
        }
        IncludeKind::Usepackage => vec![format!("{path}.sty")],
        IncludeKind::Documentclass => vec![format!("{path}.cls")],
    };

    for candidate in candidates {
        let candidate_path = join(&base, &candidate);
        if file_exists(&candidate_path) {
            return Some(candidate_path);
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------------------------------

struct Builder<'a> {
    read: ReadFn<'a>,
    file_exists: ExistsFn<'a>,
    root: String,
    model: ProjectModel,
    visited: HashSet<String>,
    files: Vec<FilePath>,
    graph: HashMap<FilePath, Vec<FilePath>>,
    flat_outline: Vec<OutlineEntry>,
    labels: Vec<LabelDefinition>,
    commands: Vec<ipc::CompletionItem>,
    seen_commands: HashSet<String>,
    environments: Vec<String>,
    seen_environments: HashSet<String>,
    bib_files: Vec<FilePath>,
    seen_bib: HashSet<String>,
    seen_local_packages: HashSet<String>,
    refs: references::ReferenceScan,
    dynamic_prefixes: BTreeSet<String>,
    unknown_bibliography: bool,
    /// Structure-lint diagnostics of every visited file (only when `lint` is set).
    lint: bool,
    structure: Vec<crate::core::log_parser::LogDiagnostic>,
}

impl Builder<'_> {
    fn absorb_definitions(&mut self, scan: ls::FileScan) {
        for item in scan.custom_commands {
            if self.seen_commands.insert(item.label.clone()) {
                self.commands.push(item);
            }
        }
        for name in scan.custom_environments {
            if self.seen_environments.insert(name.clone()) {
                self.environments.push(name);
            }
        }
    }

    fn visit(&mut self, file: &str) {
        self.visited.insert(path_key(file).into_owned());
        let file_path: FilePath = FilePath::from(file);
        self.files.push(file_path.clone());
        let text = (self.read)(file).unwrap_or_default();
        let source = MaskedSource::new(&text);
        let scan = ls::scan(&source, file, ls::ScanParts::ALL);
        if path_key(file) == path_key(&self.root) {
            self.model.magic_comments = ls::magic_comments(&text);
        }
        self.labels.extend(scan.labels.iter().cloned());
        let found = references::scan(&source, file);
        self.refs.labels.extend(found.labels);
        self.refs.option_labels.extend(found.option_labels);
        self.refs.references.extend(found.references);
        self.refs.citations.extend(found.citations);
        self.refs.bib_items.extend(found.bib_items);
        self.dynamic_prefixes.extend(found.dynamic_label_prefixes);
        if found.uses_external_documents {
            self.refs.uses_external_documents = true;
        }
        if found.uses_external_citations {
            self.refs.uses_external_citations = true;
        }
        if self.lint {
            let mut d = walker::lint(&source, file);
            self.structure.append(&mut d);
        }

        // Interleave outline entries and includes by position.
        let mut entry_index = 0usize;
        let mut children: Vec<FilePath> = Vec::new();
        let includes = scan.includes.clone();
        let outline = scan.outline.clone();
        self.absorb_definitions(scan);

        for reference in &includes {
            match reference.kind {
                IncludeKind::Bibliography | IncludeKind::Addbibresource => {
                    if let Some(bib) = resolve(reference, file, &self.root, self.file_exists) {
                        if self.seen_bib.insert(path_key(&bib).into_owned()) {
                            self.bib_files.push(FilePath::from(bib));
                        }
                    } else {
                        self.unknown_bibliography = true; // e.g. a file TeX finds through BIBINPUTS, or one not created yet
                    }
                }
                IncludeKind::Usepackage | IncludeKind::Documentclass => {
                    if let Some(local) = resolve(reference, file, &self.root, self.file_exists) {
                        if self.seen_local_packages.insert(path_key(&local).into_owned()) {
                            if let Some(sty) = (self.read)(&local) {
                                let s = ls::scan(&MaskedSource::new(&sty), &local, ls::ScanParts::DEFINITIONS);
                                self.absorb_definitions(s);
                            }
                        }
                    }
                }
                IncludeKind::Input | IncludeKind::Include | IncludeKind::Subfile | IncludeKind::ImportFile | IncludeKind::Subimport => {
                    while entry_index < outline.len() && outline[entry_index].offset < reference.range.start {
                        self.flat_outline.push(outline[entry_index].clone());
                        entry_index += 1;
                    }
                    let Some(target) = resolve(reference, file, &self.root, self.file_exists) else { continue };
                    if !is_tex_source(&target) {
                        continue;
                    }
                    let target_path: FilePath = FilePath::from(target.as_str());
                    if !children.contains(&target_path) {
                        children.push(target_path);
                    }
                    if !self.visited.contains(path_key(&target).as_ref()) {
                        self.visit(&target);
                    }
                }
                _ => {}
            }
        }
        while entry_index < outline.len() {
            self.flat_outline.push(outline[entry_index].clone());
            entry_index += 1;
        }
        if !children.is_empty() {
            self.graph.insert(file_path, children);
        }
    }
}

fn build_inner(root_file: &str, read: ReadFn, file_exists: ExistsFn, lint: bool) -> (ProjectModel, Vec<crate::core::log_parser::LogDiagnostic>) {
    let root = standardize(root_file);
    let mut b = Builder {
        read,
        file_exists,
        root: root.clone(),
        model: ProjectModel::new(&root),
        visited: HashSet::new(),
        files: Vec::new(),
        graph: HashMap::new(),
        flat_outline: Vec::new(),
        labels: Vec::new(),
        commands: Vec::new(),
        seen_commands: HashSet::new(),
        environments: Vec::new(),
        seen_environments: HashSet::new(),
        bib_files: Vec::new(),
        seen_bib: HashSet::new(),
        seen_local_packages: HashSet::new(),
        refs: references::ReferenceScan::default(),
        dynamic_prefixes: BTreeSet::new(),
        unknown_bibliography: false,
        lint,
        structure: Vec::new(),
    };
    b.visit(&root);

    let mut entries: Vec<BibRecord> = Vec::new();
    let bib_files = b.bib_files.clone();
    for bib_file in &bib_files {
        if let Some(text) = (b.read)(bib_file) {
            entries.extend(bib::entries(&text, bib_file));
        } else {
            b.unknown_bibliography = true;
        }
        // biblatex also reads .ris / .json / .xml sources, which the BibTeX parser does not understand.
        if extension_lower(bib_file) != "bib" {
            b.unknown_bibliography = true;
        }
    }

    let mut model = b.model;
    model.label_sites = b.refs.labels;
    model.option_labels = b.refs.option_labels;
    model.reference_uses = b.refs.references;
    model.citation_uses = b.refs.citations;
    model.bib_items = b.refs.bib_items;
    model.dynamic_label_prefixes = b.dynamic_prefixes.into_iter().collect();
    model.uses_external_documents = b.refs.uses_external_documents;
    model.uses_external_citations = b.refs.uses_external_citations;
    model.has_unknown_bibliography_source = b.unknown_bibliography;
    model.files = b.files;
    model.include_graph = b.graph;
    model.outline = build_tree(&b.flat_outline);
    model.flat_outline = b.flat_outline;
    model.labels = b.labels;
    model.bib_files = b.bib_files;
    model.bib_entries = entries;
    model.custom_commands = b.commands;
    model.custom_environments = b.environments;
    (model, b.structure)
}

/// Builds the index starting at `root_file`.
pub fn build(root_file: &str, read: ReadFn, file_exists: ExistsFn) -> ProjectModel {
    build_inner(root_file, read, file_exists, false).0
}

/// Nests a flat, ordered list of entries: a deeper level becomes a child of the nearest shallower heading;
/// labels are leaves under the nearest heading (top level when there is none).
pub fn build_tree(entries: &[OutlineEntry]) -> Vec<OutlineNode> {
    // Arena of nodes; children are indices.
    struct N {
        entry: OutlineEntry,
        children: Vec<usize>,
    }
    let mut arena: Vec<N> = Vec::with_capacity(entries.len());
    let mut roots: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for entry in entries {
        let id = arena.len();
        arena.push(N { entry: entry.clone(), children: Vec::new() });
        if entry.kind == OutlineKind::Label {
            if let Some(&parent) = stack.last() {
                arena[parent].children.push(id);
            } else {
                roots.push(id);
            }
            continue;
        }
        while let Some(&top) = stack.last() {
            if arena[top].entry.kind.level() >= entry.kind.level() {
                stack.pop();
            } else {
                break;
            }
        }
        if let Some(&parent) = stack.last() {
            arena[parent].children.push(id);
        } else {
            roots.push(id);
        }
        stack.push(id);
    }
    fn freeze(arena: &[N], id: usize) -> OutlineNode {
        let n = &arena[id];
        OutlineNode {
            entry: n.entry.clone(),
            children: if n.children.is_empty() { None } else { Some(n.children.iter().map(|&c| freeze(arena, c)).collect()) },
        }
    }
    roots.iter().map(|&r| freeze(&arena, r)).collect()
}

// ---------------------------------------------------------------------------------------------------
// Root detection
// ---------------------------------------------------------------------------------------------------

/// Picks the root document for `file`:
/// 1. `% !TEX root = …` magic comment in `file`; 2. `file` itself if it is a root document (a `subfiles`
///    document whose `\documentclass[main.tex]{subfiles}` names an existing file resolves to that main file
///    instead); 3. a root document among `candidates` whose include closure contains `file` (prefer
///    "main.tex"); 4. `None`.
pub fn find_root(file: &str, candidates: &[String], read: ReadFn, file_exists: ExistsFn) -> Option<String> {
    let target = standardize(file);
    let text = read(file).unwrap_or_default();

    // 1. Magic comment.
    if let Some(root_path) = ls::magic_comments(&text).root.filter(|r| !r.is_empty()) {
        let expanded = expand_tilde(&root_path);
        let url = if is_absolute(&expanded) { standardize(&expanded) } else { join(&parent_dir(&target), &expanded) };
        if file_exists(&url) {
            return Some(url);
        }
    }

    // 2. The file itself.
    let scan = ls::scan(&MaskedSource::new(&text), &target, ls::ScanParts::ROOT_INFO.union(ls::ScanParts::INCLUDES));
    if scan.is_root {
        if scan.document_class.as_deref() == Some("subfiles") {
            if let Some(mut main) = scan.document_class_options.clone().filter(|m| !m.is_empty()) {
                if !main.ends_with(".tex") {
                    main.push_str(".tex");
                }
                let url = join(&parent_dir(&target), &main);
                if path_key(&url) != path_key(&target) && file_exists(&url) {
                    return Some(url);
                }
            }
        }
        return Some(file.to_string());
    }

    // 3. A candidate root whose closure contains the file.
    let mut closure_cache: HashMap<String, Vec<IncludeReference>> = HashMap::new();
    let mut includes_of = |url: &str| -> Vec<IncludeReference> {
        if let Some(c) = closure_cache.get(url) {
            return c.clone();
        }
        let found: Vec<IncludeReference> = ls::scan(&MaskedSource::new(&read(url).unwrap_or_default()), url, ls::ScanParts::INCLUDES)
            .includes
            .into_iter()
            .filter(|r| r.kind.follows_include())
            .collect();
        closure_cache.insert(url.to_string(), found.clone());
        found
    };
    let mut closure_contains = |root: &str, target: &str| -> bool {
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(path_key(root).into_owned());
        let mut stack: Vec<String> = vec![root.to_string()];
        let target_key = path_key(target).into_owned();
        while let Some(current) = stack.pop() {
            if path_key(&current).as_ref() == target_key {
                return true;
            }
            for reference in includes_of(&current) {
                let Some(resolved) = resolve(&reference, &current, root, file_exists) else { continue };
                if !is_tex_source(&resolved) || !visited.insert(path_key(&resolved).into_owned()) {
                    continue;
                }
                stack.push(resolved);
            }
        }
        false
    };

    let is_main = |p: &String| file_name(p).to_lowercase() == "main.tex";
    let mut ordered: Vec<&String> = candidates.iter().filter(|c| is_main(c)).collect();
    ordered.extend(candidates.iter().filter(|c| !is_main(c)));
    for candidate in ordered {
        let standardized = standardize(candidate);
        if path_key(&standardized) == path_key(&target) {
            continue;
        }
        let Some(candidate_text) = read(candidate) else { continue };
        if !ls::scan(&MaskedSource::new(&candidate_text), &standardized, ls::ScanParts::ROOT_INFO).is_root {
            continue;
        }
        if closure_contains(&standardized, &target) {
            return Some(candidate.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------------
// Whole-project entry point
// ---------------------------------------------------------------------------------------------------

const SKIPPED_FOLDERS: [&str; 5] = ["node_modules", "__pycache__", ".git", ".build", ".svn"];
const MAX_ENTRIES: usize = 6000;
const MAX_DEPTH: usize = 10;

/// All `.tex` / `.ltx` files under `dir` (hidden entries and a few tool folders skipped), standardized,
/// in a stable natural order.
pub fn list_tex_files(dir: &str) -> Vec<String> {
    fn walk(dir: &str, depth: usize, count: &mut usize, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<(String, bool)> = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = format!("{}/{}", dir.trim_end_matches('/'), name);
            let is_dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
            entries.push((name, is_dir));
        }
        entries.sort_by(|a, b| natural_cmp(&a.0, &b.0));
        for (name, is_dir) in entries {
            if *count >= MAX_ENTRIES {
                return;
            }
            let path = format!("{}/{}", dir.trim_end_matches('/'), name);
            if is_dir {
                if SKIPPED_FOLDERS.contains(&name.as_str()) || depth >= MAX_DEPTH {
                    continue;
                }
                *count += 1;
                walk(&path, depth + 1, count, out);
            } else {
                *count += 1;
                let ext = extension_lower(&name);
                if ext == "tex" || ext == "ltx" {
                    out.push(path);
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut count = 0;
    walk(&standardize(dir), 0, &mut count, &mut out);
    out
}

/// Unsaved buffer texts keyed by absolute path; lookups ignore separator style (and case on Windows).
pub struct OverrideTable {
    map: HashMap<String, String>,
}

impl OverrideTable {
    pub fn new(overrides: &HashMap<String, String>) -> Self {
        let map = overrides.iter().map(|(k, v)| (path_key(&standardize(k)).into_owned(), v.clone())).collect();
        OverrideTable { map }
    }
    pub fn get(&self, path: &str) -> Option<&String> {
        self.map.get(path_key(&standardize(path)).as_ref())
    }
    pub fn keys_under<'a>(&'a self, _dir: &str) -> impl Iterator<Item = &'a String> {
        self.map.keys()
    }
}

/// Options of [`build_project_analysis`].
#[derive(Debug, Clone, Default)]
pub struct ProjectOptions {
    /// Explicit "main document" choice (wins when the file exists).
    pub root_file: Option<String>,
    /// The file being edited: its root is found through magic comments / include closures.
    pub current_file: Option<String>,
    /// The previously chosen root, kept when nothing better is found.
    pub sticky_root: Option<String>,
}

/// Everything known about a project: the index, the live warnings and the statistics.
#[derive(Debug, Clone)]
pub struct ProjectAnalysis {
    pub model: ProjectModel,
    /// Root-document candidates (files with `\documentclass`), standardized.
    pub root_candidates: Vec<String>,
    /// Reference + citation warnings (`ReferenceLinter`).
    pub reference_diagnostics: Vec<crate::core::log_parser::LogDiagnostic>,
    /// Structure diagnostics (`StructureLinter`) of every file of the project.
    pub structure_diagnostics: Vec<crate::core::log_parser::LogDiagnostic>,
    pub statistics: DocumentStatistics,
}

/// Index of a project directory (or of the project whose root file is given): finds the root document
/// (`% !TEX root`, `\documentclass`, `main.tex`), follows the includes and computes warnings and statistics.
/// `overrides` are unsaved buffer texts keyed by absolute path; they win over the disk. Returns `None` when
/// no root document exists.
pub fn build_project_analysis(root: &Path, overrides: &HashMap<String, String>, options: &ProjectOptions) -> Option<ProjectAnalysis> {
    let table = OverrideTable::new(overrides);
    let read = |p: &str| -> Option<String> {
        if let Some(t) = table.get(p) {
            return Some(t.clone());
        }
        read_text_file(p)
    };
    let exists = |p: &str| -> bool {
        table.get(p).is_some() || std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
    };
    let root_str = standardize(&root.to_string_lossy());

    // A root given as a file is the root document itself.
    let root_is_file = !root.is_dir() && (exists(&root_str) || table.get(&root_str).is_some());
    let folder = if root_is_file { parent_dir(&root_str) } else { root_str.clone() };

    // Root-document candidates.
    let mut tex_files = list_tex_files(&folder);
    let folder_prefix = format!("{}/", folder.trim_end_matches('/'));
    let mut present: HashSet<String> = tex_files.iter().map(|f| path_key(f).into_owned()).collect();
    let mut extra: Vec<String> = table
        .keys_under(&folder)
        .filter(|k| {
            let ext = extension_lower(k);
            (ext == "tex" || ext == "ltx") && path_key(k).starts_with(path_key(&folder_prefix).as_ref())
        })
        .cloned()
        .collect();
    extra.sort_by(|a, b| natural_cmp(a, b));
    for k in extra {
        // Override keys are lowercased on Windows; recover a displayable path from the original keys.
        let original = overrides.keys().find(|o| path_key(&standardize(o)) == path_key(&k)).map(|o| standardize(o)).unwrap_or(k);
        if present.insert(path_key(&original).into_owned()) {
            tex_files.push(original);
        }
    }
    let mut candidates: Vec<String> = Vec::new();
    for f in &tex_files {
        let is_root = match table.get(f) {
            Some(t) => ls::is_root_document(t),
            None => ls::is_root_document(&read_head(f)),
        };
        if is_root {
            candidates.push(f.clone());
        }
    }

    // Pick the root.
    let forced = options.root_file.as_deref().map(standardize).filter(|f| exists(f));
    let root_file: Option<String> = if root_is_file {
        Some(root_str.clone())
    } else if let Some(f) = forced {
        Some(f)
    } else if let Some(found) = options.current_file.as_deref().and_then(|c| find_root(&standardize(c), &candidates, &read, &exists)) {
        Some(standardize(&found))
    } else if let Some(s) = options.sticky_root.as_deref().map(standardize).filter(|s| exists(s) && candidates.iter().any(|c| path_key(c) == path_key(s))) {
        Some(s)
    } else {
        candidates
            .iter()
            .find(|c| file_name(c).to_lowercase() == "main.tex")
            .cloned()
            .or_else(|| candidates.iter().min_by(|a, b| natural_cmp(a, b)).cloned())
    };
    let root_file = root_file?;

    // Cache texts: the index, the linters and the statistics all read the same files.
    let cache: std::cell::RefCell<HashMap<String, Option<String>>> = std::cell::RefCell::new(HashMap::new());
    let cached_read = |p: &str| -> Option<String> {
        let key = path_key(p).into_owned();
        if let Some(v) = cache.borrow().get(&key) {
            return v.clone();
        }
        let v = read(p);
        cache.borrow_mut().insert(key, v.clone());
        v
    };

    let (model, structure) = build_inner(&root_file, &cached_read, &exists, true);
    let reference_diagnostics = references::diagnostics(&model);
    let statistics = statistics::compute(&root_file, &cached_read, &exists);
    Some(ProjectAnalysis { model, root_candidates: candidates, reference_diagnostics, structure_diagnostics: structure, statistics })
}

fn ipc_diagnostic(d: &crate::core::log_parser::LogDiagnostic, source: &str) -> ipc::Diagnostic {
    let mut out = d.to_ipc();
    out.path = to_native(&out.path);
    out.source = source.to_string();
    out
}

impl ProjectAnalysis {
    /// Converts to the IPC type of `commands.rs` (paths in native spelling).
    ///
    /// - `outline`: headings and frames of all files in include order; `level` = part -1, chapter 0,
    ///   section 1 …; a heading's `label` is the first `\label` right after it.
    /// - `citations`: distinct cited keys in order of first use. `includes`: the included `.tex` files in
    ///   include order (the root file excluded).
    /// - `warnings`: reference warnings (`source: "reference"`) and structure diagnostics (`"structure"`).
    pub fn to_ipc(&self) -> ipc::ProjectIndex {
        let m = &self.model;
        let mut outline: Vec<ipc::OutlineItem> = Vec::new();
        for entry in &m.flat_outline {
            if entry.kind == OutlineKind::Label {
                if let Some(last) = outline.last_mut() {
                    if last.label.is_none() && last.path == to_native(&entry.file) {
                        last.label = Some(entry.title.clone());
                    }
                }
                continue;
            }
            outline.push(ipc::OutlineItem {
                level: entry.kind.level() - 1,
                title: entry.title.clone(),
                path: to_native(&entry.file),
                line: entry.line as u32,
                label: None,
            });
        }
        let labels = m
            .labels
            .iter()
            .map(|l| ipc::Label { name: l.key.clone(), path: to_native(&l.file), line: l.line as u32 })
            .collect();
        let mut seen: HashSet<&str> = HashSet::new();
        let citations = m
            .citation_uses
            .iter()
            .filter(|c| seen.insert(c.key.as_str()))
            .map(|c| c.key.clone())
            .collect();
        let bib_keys = m
            .bib_entries
            .iter()
            .map(|b| ipc::BibEntry {
                key: b.key.clone(),
                entry_type: Some(b.entry_type.clone()),
                path: to_native(&b.file),
                line: b.line as u32,
                title: b.title.clone(),
                author: b.author.clone(),
            })
            .collect();
        let includes = m.files.iter().skip(1).map(|f| to_native(f)).collect();
        let mut warnings: Vec<ipc::Diagnostic> = Vec::new();
        warnings.extend(self.structure_diagnostics.iter().map(|d| ipc_diagnostic(d, "structure")));
        warnings.extend(self.reference_diagnostics.iter().map(|d| ipc_diagnostic(d, "reference")));
        ipc::ProjectIndex {
            outline,
            labels,
            citations,
            bib_keys,
            includes,
            root_file: Some(to_native(&m.root_file)),
            warnings,
            stats: ipc::ProjectStats { words: self.statistics.totals.words() as u32 },
        }
    }
}

/// IPC entry point: the index of the project in `root` with unsaved buffers in `overrides` (absolute path ->
/// text). When no root document exists the result is empty (no `root_file`).
pub fn build_project_index(root: &Path, overrides: &HashMap<String, String>) -> ipc::ProjectIndex {
    build_project_index_with(root, overrides, &ProjectOptions::default())
}

/// Like [`build_project_index`] with an explicit main document / current file (see [`ProjectOptions`]).
pub fn build_project_index_with(root: &Path, overrides: &HashMap<String, String>, options: &ProjectOptions) -> ipc::ProjectIndex {
    match build_project_analysis(root, overrides, options) {
        Some(a) => a.to_ipc(),
        None => ipc::ProjectIndex::default(),
    }
}

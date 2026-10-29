//! Context-aware completion: a port of `Sources/TeXCore/Completion/*` (CompletionContext, CompletionEngine,
//! CompletionRanking, Snippet) and the logic of `Catalog/CompletionCatalog.swift`.
//!
//! The module is self-contained: it carries its own port of the pieces of `MaskedSource` (comment / verbatim /
//! `\verb` / `\url` / `\iffalse` masking) and `StructureWalker` (open environments and math mode up to the
//! cursor) that the Swift engine relies on, so it does not depend on other core files being finished.
//!
//! All offsets are UTF-16 code units (CodeMirror positions). Lines and columns taken by [`complete_at`] are 1-based.
//!
//! # Snippet syntax
//!
//! The Swift app stores snippets as `${1:default}` / `$1` (mirror) / `$0` (final cursor) with real `\n` / `\t`
//! characters (see [`Snippet`]). The IPC contract uses the **CodeMirror** snippet syntax, so every
//! `insertText` leaving this module is converted by [`to_codemirror`]:
//!
//! | Swift source            | CodeMirror output                                              |
//! |-------------------------|----------------------------------------------------------------|
//! | `${1:text}`             | `${1:text}`                                                    |
//! | `${1}` / `$1`           | `${1}` (the default of the index, if any, is repeated: `${1:text}`) |
//! | repeated `$1` (mirror)  | `${1:text}` again: CodeMirror links fields with the same number |
//! | `$0` (final cursor)     | `${}` (unnumbered fields are visited after all numbered ones)  |
//! | real `\n`, `\t`         | unchanged (CodeMirror re-indents lines and turns leading tabs into the indent unit) |
//! | `\x`, `\$`, `$x$`       | unchanged (backslashes are never escapes)                      |
//!
//! Limits of CodeMirror's syntax, handled as follows:
//! * a default that contains `}`, `\{`, or a line break cannot be written (`${1:...}` ends at the first `}`):
//!   the field is emitted empty (`${1}`); five catalog entries are affected (`\tikzset`, `\logo`, `\sisetup`,
//!   `\ifthenelse`, `\AtBeginSection`);
//! * nested placeholders (`${1:a ${2:b}}`) are flattened: only the outer field remains, with the expanded text
//!   as its default;
//! * CodeMirror has no escape for a literal `${...}` / `#{...}` in the text; the catalog contains none.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

// ===================================================================================================
// Public types
// ===================================================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Command,
    Environment,
    Label,
    Citation,
    File,
    Package,
    Snippet,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Command => "command",
            Kind::Environment => "environment",
            Kind::Label => "label",
            Kind::Citation => "citation",
            Kind::File => "file",
            Kind::Package => "package",
            Kind::Snippet => "snippet",
        }
    }
}

/// A completion candidate. `insert_text` is in CodeMirror snippet syntax when it leaves this module
/// (inputs such as `ProjectSymbols::custom_commands` may use the Swift syntax; both are accepted).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub kind: Kind,
    pub label: String,
    pub insert_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Providing package (`amsmath`...), if not in the LaTeX kernel.
    #[serde(default, rename = "package", skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
}

impl CompletionItem {
    pub fn new(kind: Kind, label: &str, insert_text: Option<&str>, detail: Option<&str>, package: Option<&str>) -> Self {
        CompletionItem {
            kind,
            label: label.to_string(),
            insert_text: insert_text.unwrap_or(label).to_string(),
            detail: detail.map(str::to_string),
            package: package.map(str::to_string),
        }
    }
}

/// A `\label{key}` definition.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelInfo {
    /// `name` is accepted so that `commands::Label` JSON deserializes directly.
    #[serde(alias = "name")]
    pub key: String,
    pub path: String,
    pub line: u32,
    /// Short context ("Introduction", "equation"); shown instead of `file:line` when present.
    #[serde(default)]
    pub context: Option<String>,
}

/// A `.bib` entry.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BibInfo {
    pub key: String,
    #[serde(default, rename = "type")]
    pub entry_type: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub year: Option<String>,
}

/// Everything project-specific the engine needs (Swift `ProjectSymbols`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSymbols {
    pub labels: Vec<LabelInfo>,
    pub bib_entries: Vec<BibInfo>,
    /// Project-defined commands (`\newcommand`...), shown before the catalog.
    pub custom_commands: Vec<CompletionItem>,
    /// Project-defined environments (`\newenvironment`, `\newtheorem`...).
    pub custom_environments: Vec<String>,
    /// Project files relative to the root file's directory, `/`-separated (backslashes are tolerated).
    pub project_files: Vec<String>,
}

/// Borrowed form of [`ProjectSymbols`] so callers holding slices need not clone them.
#[derive(Debug, Clone, Copy, Default)]
pub struct SymbolsRef<'a> {
    pub labels: &'a [LabelInfo],
    pub bib_entries: &'a [BibInfo],
    pub custom_commands: &'a [CompletionItem],
    pub custom_environments: &'a [String],
    pub project_files: &'a [String],
}

impl ProjectSymbols {
    pub fn as_ref(&self) -> SymbolsRef<'_> {
        SymbolsRef {
            labels: &self.labels,
            bib_entries: &self.bib_entries,
            custom_commands: &self.custom_commands,
            custom_environments: &self.custom_environments,
            project_files: &self.project_files,
        }
    }
}

/// What the editor should show at a cursor position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionResult {
    /// UTF-16 offset where the replacement starts (document position).
    pub from: usize,
    /// UTF-16 offset where it ends (may extend past the cursor, e.g. over an auto-inserted `}`).
    pub to: usize,
    /// The text typed so far that the items were filtered with (`\fr`, `ite`, `eq:`).
    pub prefix: String,
    /// "command" | "environment" | "endEnvironment" | "label" | "citation" | "package" | "file".
    pub context: String,
    /// Cursor is in math mode (only computed for command and `\begin` contexts, false otherwise).
    pub in_math: bool,
    /// Ranked best first, at most [`MAX_ITEMS`].
    pub items: Vec<CompletionItem>,
}

pub const MAX_ITEMS: usize = 200;
/// How many commands a bare `\` shows.
pub const BARE_BACKSLASH_LIMIT: usize = 80;

// ===================================================================================================
// Catalog
// ===================================================================================================

#[derive(Deserialize)]
struct RawCatalog {
    commands: Vec<CompletionItem>,
    #[serde(rename = "mathCommands")]
    math_commands: Vec<CompletionItem>,
    environments: Vec<CompletionItem>,
    packages: Vec<CompletionItem>,
}

struct Catalog {
    commands: Vec<CompletionItem>,
    math_commands: Vec<CompletionItem>,
    environments: Vec<CompletionItem>,
    packages: Vec<CompletionItem>,
    all_commands: Vec<CompletionItem>,
    math_labels: HashSet<String>,
    text_labels: HashSet<String>,
}

const CATALOG_JSON: &str = include_str!("data/catalog.json");

fn parse_raw_catalog() -> RawCatalog {
    serde_json::from_str(CATALOG_JSON).expect("core/data/catalog.json is valid")
}

fn convert_all(items: &mut [CompletionItem]) {
    for item in items {
        if let Cow::Owned(converted) = to_codemirror(&item.insert_text) {
            item.insert_text = converted;
        }
    }
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut raw = parse_raw_catalog();
        convert_all(&mut raw.commands);
        convert_all(&mut raw.math_commands);
        convert_all(&mut raw.environments);
        convert_all(&mut raw.packages);
        // allCommands: text first then math, de-duplicated by label (first wins), duplicates inside one
        // table collapse as well.
        let mut seen = HashSet::new();
        let all_commands: Vec<CompletionItem> = raw
            .commands
            .iter()
            .chain(raw.math_commands.iter())
            .filter(|item| seen.insert(item.label.as_str()))
            .cloned()
            .collect();
        let math_labels = raw.math_commands.iter().map(|i| i.label.clone()).collect();
        let text_labels = raw.commands.iter().map(|i| i.label.clone()).collect();
        Catalog {
            commands: raw.commands,
            math_commands: raw.math_commands,
            environments: raw.environments,
            packages: raw.packages,
            all_commands,
            math_labels,
            text_labels,
        }
    })
}

/// Text-mode and general commands (insert texts in CodeMirror syntax).
pub fn catalog_commands() -> &'static [CompletionItem] {
    &catalog().commands
}
/// Math-mode commands.
pub fn catalog_math_commands() -> &'static [CompletionItem] {
    &catalog().math_commands
}
/// Environments; `insert_text` replaces the name typed after `\begin{` (`name}` + args + body + `\end{name}`).
pub fn catalog_environments() -> &'static [CompletionItem] {
    &catalog().environments
}
pub fn catalog_packages() -> &'static [CompletionItem] {
    &catalog().packages
}
/// Swift `CompletionCatalog.allCommands`: commands followed by math commands, de-duplicated by label
/// (the first occurrence wins, so the text-mode entry shadows a math entry with the same label).
pub fn all_commands() -> &'static [CompletionItem] {
    &catalog().all_commands
}

// ===================================================================================================
// Snippet (port of Snippet.swift) and conversion to CodeMirror syntax
// ===================================================================================================

/// A UTF-16 range inside [`Snippet::text`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub location: usize,
    pub length: usize,
}

/// A parsed snippet in the Swift syntax: `${1:default}` (default may contain balanced braces, backslashes and
/// nested placeholders), `${1}`, `$1`, `$0`. Backslash sequences are never escapes; a `$` preceded by a
/// backslash and a `$` not followed by a digit or `{` stay literal; an unterminated `${` is literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    /// Expanded text: placeholders replaced by their default text.
    pub text: String,
    /// Tab stops in visiting order (increasing index); only the first occurrence of each index.
    pub tab_stops: Vec<Span>,
    /// The index of each entry of `tab_stops`.
    pub tab_stop_indices: Vec<usize>,
    /// `$0`, relative to the start of `text`; `None` means end of text.
    pub final_cursor: Option<usize>,
    /// For each tab stop, its later occurrences (mirrors). Empty when the snippet has none.
    pub mirrors: Vec<Vec<Span>>,
    units: Vec<u16>,
}

struct SnippetParser<'a> {
    u: &'a [u16],
    defaults: HashMap<usize, Vec<u16>>,
    output: Vec<u16>,
    occurrences: BTreeMap<usize, Vec<Span>>,
    final_cursor: Option<usize>,
    depth: u32,
}

#[inline]
fn is_digit(c: u16) -> bool {
    c.wrapping_sub(48) < 10
}

impl<'a> SnippetParser<'a> {
    fn new(u: &'a [u16], defaults: HashMap<usize, Vec<u16>>) -> Self {
        SnippetParser { u, defaults, output: Vec::with_capacity(u.len()), occurrences: BTreeMap::new(), final_cursor: None, depth: 0 }
    }

    fn parse(&mut self) {
        self.parse_sequence(0, self.u.len());
    }

    /// Backslashes are ordinary characters; a `$` right after a backslash is LaTeX's dollar sign.
    fn parse_sequence(&mut self, start: usize, end: usize) {
        let mut i = start;
        while i < end {
            let c = self.u[i];
            if c == 36 && !(i > 0 && self.u[i - 1] == 92) {
                if let Some(next) = self.placeholder(i, end) {
                    i = next;
                    continue;
                }
            }
            self.output.push(c);
            i += 1;
        }
    }

    fn number(&self, mut k: usize, end: usize) -> (usize, usize) {
        let mut index = 0usize;
        while k < end && is_digit(self.u[k]) {
            index = index.saturating_mul(10).saturating_add((self.u[k] - 48) as usize);
            k += 1;
        }
        (index, k)
    }

    fn placeholder(&mut self, i: usize, end: usize) -> Option<usize> {
        if i + 1 >= end {
            return None;
        }
        let d = self.u[i + 1];
        if is_digit(d) {
            let (index, k) = self.number(i + 1, end);
            self.insert(index, None);
            return Some(k);
        }
        if d != 123 {
            return None;
        }
        let k = i + 2;
        if !(k < end && is_digit(self.u[k])) {
            return None;
        }
        let (index, k) = self.number(k, end);
        if k >= end {
            return None;
        }
        if self.u[k] == 125 {
            self.insert(index, None);
            return Some(k + 1);
        }
        if self.u[k] != 58 {
            return None;
        }
        // Default: up to the matching close brace (escaped characters are copied verbatim).
        let body_start = k + 1;
        let mut depth = 1;
        let mut m = body_start;
        while m < end {
            let c = self.u[m];
            if c == 92 {
                m += 2;
                continue;
            }
            if c == 123 {
                depth += 1;
            } else if c == 125 {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            m += 1;
        }
        if !(m < end && self.u[m] == 125) {
            return None;
        }
        if self.depth >= 64 {
            return None; // pathological nesting: treat as literal text
        }
        self.insert(index, Some((body_start, m)));
        Some(m + 1)
    }

    fn insert(&mut self, index: usize, default_text: Option<(usize, usize)>) {
        let location = self.output.len();
        if let Some((a, b)) = default_text {
            self.depth += 1;
            self.parse_sequence(a, b);
            self.depth -= 1;
            if !self.defaults.contains_key(&index) {
                self.defaults.insert(index, self.output[location..].to_vec());
            }
        } else if let Some(known) = self.defaults.get(&index) {
            self.output.extend_from_slice(known);
        }
        if index == 0 {
            if self.final_cursor.is_none() {
                self.final_cursor = Some(location);
            }
        } else {
            let length = self.output.len() - location;
            self.occurrences.entry(index).or_default().push(Span { location, length });
        }
    }
}

impl Snippet {
    pub fn parse(source: &str) -> Snippet {
        let units: Vec<u16> = source.encode_utf16().collect();
        // Pass 1 learns the default text of each index (mirrors written as plain `$1` reuse it).
        let mut first = SnippetParser::new(&units, HashMap::new());
        first.parse();
        let defaults = std::mem::take(&mut first.defaults);
        let mut second = SnippetParser::new(&units, defaults);
        second.parse();
        let mut tab_stops = Vec::new();
        let mut tab_stop_indices = Vec::new();
        let mut mirrors: Vec<Vec<Span>> = Vec::new();
        for (index, ranges) in &second.occurrences {
            if let Some((head, rest)) = ranges.split_first() {
                tab_stops.push(*head);
                tab_stop_indices.push(*index);
                mirrors.push(rest.to_vec());
            }
        }
        if !mirrors.iter().any(|m| !m.is_empty()) {
            mirrors.clear();
        }
        Snippet {
            text: String::from_utf16_lossy(&second.output),
            tab_stops,
            tab_stop_indices,
            final_cursor: second.final_cursor,
            mirrors,
            units: std::mem::take(&mut second.output),
        }
    }

    /// The snippet in CodeMirror syntax (see the module documentation for the mapping).
    pub fn to_codemirror(&self) -> String {
        // (start, length, index) with index 0 = final cursor; every occurrence of a numbered field is
        // emitted as `${n:default}` so that CodeMirror links them and the text matches Swift's expansion.
        let mut spans: Vec<(usize, usize, usize)> = Vec::new();
        for (k, head) in self.tab_stops.iter().enumerate() {
            let index = self.tab_stop_indices[k];
            spans.push((head.location, head.length, index));
            if let Some(list) = self.mirrors.get(k) {
                for m in list {
                    spans.push((m.location, m.length, index));
                }
            }
        }
        if let Some(c) = self.final_cursor {
            spans.push((c, 0, 0));
        }
        // Zero-length spans first at a given start (siblings), then the longest (outer) one; nested spans
        // start inside an emitted one and are dropped.
        spans.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then((a.1 != 0).cmp(&(b.1 != 0)))
                .then(b.1.cmp(&a.1))
                .then((a.2 == 0).cmp(&(b.2 == 0)))
        });
        let u = &self.units;
        let mut out = String::with_capacity(self.text.len() + 16);
        let mut p = 0usize;
        for (start, length, index) in spans {
            if start < p || start > u.len() {
                continue;
            }
            out.push_str(&String::from_utf16_lossy(&u[p..start]));
            let end = (start + length).min(u.len());
            if index == 0 {
                out.push_str("${}");
            } else {
                let body = &u[start..end];
                let representable = !body.iter().any(|&c| c == 125 || c == 10 || c == 13)
                    && !body.windows(2).any(|w| w[0] == 92 && w[1] == 123);
                out.push_str("${");
                out.push_str(&index.to_string());
                if !body.is_empty() && representable {
                    out.push(':');
                    out.push_str(&String::from_utf16_lossy(body));
                }
                out.push('}');
            }
            p = end;
        }
        out.push_str(&String::from_utf16_lossy(&u[p.min(u.len())..]));
        out
    }
}

/// Converts a Swift-syntax snippet to CodeMirror syntax. Text without a `$` is returned unchanged
/// (no allocation). The conversion is idempotent.
pub fn to_codemirror(source: &str) -> Cow<'_, str> {
    if !source.contains('$') {
        return Cow::Borrowed(source);
    }
    let converted = Snippet::parse(source).to_codemirror();
    if converted == source {
        Cow::Borrowed(source)
    } else {
        Cow::Owned(converted)
    }
}

// ===================================================================================================
// UTF-16 helpers (Syntax/LaTeXSyntax.swift)
// ===================================================================================================

#[inline]
fn is_ascii_letter(c: u16) -> bool {
    c.wrapping_sub(65) < 26 || c.wrapping_sub(97) < 26
}
/// Letters allowed in a control word (`@` is accepted everywhere).
#[inline]
fn is_command_letter(c: u16) -> bool {
    is_ascii_letter(c) || c == 64
}
#[inline]
fn is_hspace(c: u16) -> bool {
    c == 32 || c == 9
}
#[inline]
fn is_line_break(c: u16) -> bool {
    c == 10 || c == 13
}
#[inline]
fn is_tex_whitespace(c: u16) -> bool {
    c == 32 || c == 9 || c == 10 || c == 13
}
#[inline]
fn is_high_surrogate(c: u16) -> bool {
    (c & 0xFC00) == 0xD800
}
#[inline]
fn is_low_surrogate(c: u16) -> bool {
    (c & 0xFC00) == 0xDC00
}

fn u16_eq(a: &[u16], s: &str) -> bool {
    a.len() == s.len() && a.iter().zip(s.bytes()).all(|(&x, y)| x == y as u16)
}

/// ASCII view of a command name without allocating; `None` when too long or not ASCII.
fn ascii_name<'b>(u: &[u16], buf: &'b mut [u8; 48]) -> Option<&'b str> {
    if u.len() > buf.len() {
        return None;
    }
    for (k, &c) in u.iter().enumerate() {
        if c > 127 {
            return None;
        }
        buf[k] = c as u8;
    }
    std::str::from_utf8(&buf[..u.len()]).ok()
}

/// `\cite`, `\citep`, `\Citet`, `\parencite`, `\textcite`, `\autocite`, `\footcite`, `\nocite`... but not `\citestyle`.
fn is_cite_like(name: &[u16]) -> bool {
    let n = name.len();
    if n < 4 {
        return false;
    }
    let lower = |c: u16| if c.wrapping_sub(65) < 26 { c + 32 } else { c };
    let first = name[0];
    if first == 99 || first == 67 {
        let starts = name[..4].iter().zip(b"cite").all(|(&c, &t)| lower(c) == t as u16);
        if !starts {
            return false;
        }
        let citestyle = n == 9 && name.iter().zip(b"citestyle").all(|(&c, &t)| lower(c) == t as u16);
        return !citestyle;
    }
    if name[n - 1] == 101 {
        return name[n - 4..].iter().zip(b"cite").all(|(&c, &t)| lower(c) == t as u16);
    }
    false
}

// ===================================================================================================
// TeXArguments (Scanning/ArgumentParser.swift) – only what the masker and the walker need
// ===================================================================================================

const MAX_ARGUMENT_LENGTH: usize = 60_000;

fn skip_whitespace(u: &[u16], from: usize) -> usize {
    let mut i = from;
    while i < u.len() && is_tex_whitespace(u[i]) {
        i += 1;
    }
    i
}

/// Index just past the `}` of the group opening at `open`.
fn braced(u: &[u16], open: usize, limit: usize) -> Option<usize> {
    let n = u.len();
    if !(open < n && u[open] == 123) {
        return None;
    }
    let mut depth = 0;
    let mut k = open;
    let stop = n.min(open.saturating_add(limit));
    while k < stop {
        let c = u[k];
        if c == 92 {
            k += 2;
            continue;
        }
        if c == 123 {
            depth += 1;
        } else if c == 125 {
            depth -= 1;
            if depth == 0 {
                return Some(k + 1);
            }
        }
        k += 1;
    }
    None
}

/// Index just past the `]` of the group opening at `open` (braces inside are skipped).
fn bracketed(u: &[u16], open: usize, limit: usize) -> Option<usize> {
    let n = u.len();
    if !(open < n && u[open] == 91) {
        return None;
    }
    let mut braces = 0;
    let mut k = open + 1;
    let stop = n.min(open.saturating_add(limit));
    while k < stop {
        let c = u[k];
        if c == 92 {
            k += 2;
            continue;
        }
        if c == 123 {
            braces += 1;
        } else if c == 125 {
            braces -= 1;
        } else if c == 93 && braces <= 0 {
            return Some(k + 1);
        }
        k += 1;
    }
    None
}

/// End of the control sequence at `i` (`\foo` or `\,`).
fn control_sequence_end(u: &[u16], i: usize) -> Option<usize> {
    let n = u.len();
    if !(i + 1 < n && u[i] == 92) {
        return None;
    }
    if is_command_letter(u[i + 1]) {
        let mut j = i + 2;
        while j < n && is_command_letter(u[j]) {
            j += 1;
        }
        return Some(j);
    }
    Some(i + 2)
}

// ===================================================================================================
// MaskedSource (Scanning/MaskedSource.swift)
// ===================================================================================================

const VERBATIM_ENVIRONMENTS: [&str; 14] = [
    "verbatim", "verbatim*", "Verbatim", "Verbatim*", "BVerbatim", "BVerbatim*", "LVerbatim", "LVerbatim*", "lstlisting",
    "minted", "comment", "filecontents", "filecontents*", "spverbatim",
];

fn is_verbatim_environment(name: &[u16]) -> Option<&'static str> {
    if u16_eq(name, "verbatimtab") {
        return Some("verbatimtab");
    }
    VERBATIM_ENVIRONMENTS.iter().copied().find(|s| u16_eq(name, s))
}

fn verbatim_with_options(name: &str) -> bool {
    matches!(
        name,
        "Verbatim" | "Verbatim*" | "BVerbatim" | "BVerbatim*" | "LVerbatim" | "LVerbatim*" | "lstlisting" | "minted"
    )
}

/// Index of the closing `]` of a `[...]` group that stays on one line (brace aware).
fn closing_bracket_on_line(u: &[u16], open: usize) -> Option<usize> {
    let n = u.len();
    if !(open < n && u[open] == 91) {
        return None;
    }
    let mut depth = 0;
    let mut k = open + 1;
    let stop = n.min(open + 300);
    while k < stop {
        let c = u[k];
        if is_line_break(c) {
            return None;
        }
        if c == 92 {
            k += 2;
            continue;
        }
        if c == 123 {
            depth += 1;
        } else if c == 125 {
            depth -= 1;
        } else if c == 93 && depth <= 0 {
            return Some(k);
        }
        k += 1;
    }
    None
}

/// Index of the closing `}` of a `{...}` group that stays on one line.
fn closing_brace_on_line(u: &[u16], open: usize) -> Option<usize> {
    let n = u.len();
    if !(open < n && u[open] == 123) {
        return None;
    }
    let mut depth = 0;
    let mut k = open;
    let stop = n.min(open + 200);
    while k < stop {
        let c = u[k];
        if is_line_break(c) {
            return None;
        }
        if c == 92 {
            k += 2;
            continue;
        }
        if c == 123 {
            depth += 1;
        } else if c == 125 {
            depth -= 1;
            if depth == 0 {
                return Some(k);
            }
        }
        k += 1;
    }
    None
}

/// Reads an environment name inside `{...}` at `open`: (name start, name end, offset after the brace).
fn environment_name(u: &[u16], open: usize) -> Option<(usize, usize, usize)> {
    let n = u.len();
    if !(open < n && u[open] == 123) {
        return None;
    }
    let mut k = open + 1;
    let stop = n.min(open + 120);
    while k < stop {
        let c = u[k];
        if c == 125 {
            return Some((open + 1, k, k + 1));
        }
        if c == 123 || c == 92 || c == 37 || is_line_break(c) {
            return None;
        }
        k += 1;
    }
    None
}

fn verbatim_opening(u: &[u16], name: &str, from: usize) -> usize {
    let mut body_start = from;
    if !verbatim_with_options(name) {
        return body_start;
    }
    let n = u.len();
    if body_start < n && u[body_start] == 91 {
        if let Some(close) = closing_bracket_on_line(u, body_start) {
            body_start = close + 1;
        }
    }
    if name == "minted" {
        let mut k = body_start;
        while k < n && is_hspace(u[k]) {
            k += 1;
        }
        if k < n && u[k] == 123 {
            if let Some(close) = closing_brace_on_line(u, k) {
                body_start = close + 1;
            }
        }
    }
    body_start
}

/// Offset of the `\` of `\end{name}` searching from `from`; `u.len()` when unterminated.
fn verbatim_end(u: &[u16], name: &str, from: usize) -> usize {
    let pattern: Vec<u16> = format!("\\end{{{name}}}").encode_utf16().collect();
    let m = pattern.len();
    let n = u.len();
    let mut k = from;
    while k + m <= n {
        if u[k] == 92 && u[k..k + m] == pattern[..] {
            return k;
        }
        k += 1;
    }
    n
}

fn opens_conditional(name: &str) -> bool {
    if !name.starts_with("if") || name.len() <= 2 {
        return false;
    }
    if name == "iff" || name == "ifthenelse" {
        return false;
    }
    const PREFIXES: [&str; 18] = [
        "ifdef", "ifundef", "ifstr", "ifbool", "ifblank", "ifnumcomp", "ifnumequal", "ifnumgreater", "ifnumless", "ifdimcomp",
        "ifdimequal", "ifdimgreater", "ifdimless", "ifcsdef", "ifcsundef", "ifcsstring", "ifcsempty", "iffontchar",
    ];
    !PREFIXES.iter().any(|p| name.starts_with(p))
}

#[derive(Clone, Copy)]
enum MaskKind {
    Begin,
    Verb,
    Lstinline,
    Mintinline,
    RawGroup,
    Iffalse,
}

/// UTF-16 snapshot plus a copy in which everything that must not be interpreted as TeX is blanked with spaces
/// (comments, verbatim bodies, `\verb`, raw `\url`/`\href` arguments, `\iffalse` blocks). Line breaks are
/// kept, so offsets and lines are identical in both copies.
struct Masked {
    original: Vec<u16>,
    masked: Vec<u16>,
    line_starts: Vec<usize>,
}

impl Masked {
    fn new(units: Vec<u16>) -> Masked {
        let masked = units.clone();
        let mut m = Masked { original: units, masked, line_starts: vec![0] };
        m.build();
        m
    }

    /// 1-based line containing `offset`.
    fn line_at(&self, offset: usize) -> usize {
        let mut lo = 0;
        let mut hi = self.line_starts.len() - 1;
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.line_starts[mid] <= offset {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo + 1
    }

    /// True when the line break at `index` is followed by a whitespace-only line (paragraph break).
    fn is_paragraph_break(&self, index: usize) -> bool {
        let o = &self.original;
        let n = o.len();
        let mut j = index + 1;
        if o[index] == 13 && j < n && o[j] == 10 {
            j += 1;
        }
        while j < n && is_hspace(o[j]) {
            j += 1;
        }
        j < n && is_line_break(o[j])
    }

    fn build(&mut self) {
        let n = self.original.len();
        let mut i = 0;
        while i < n {
            match self.original[i] {
                10 => {
                    self.line_starts.push(i + 1);
                    i += 1;
                }
                13 => {
                    if i + 1 < n && self.original[i + 1] == 10 {
                        i += 1;
                    }
                    self.line_starts.push(i + 1);
                    i += 1;
                }
                37 => {
                    let mut j = i + 1;
                    while j < n && self.original[j] != 10 && self.original[j] != 13 {
                        j += 1;
                    }
                    self.blank(i, j);
                    i = j;
                }
                92 => i = self.command(i),
                _ => i += 1,
            }
        }
    }

    fn blank(&mut self, a: usize, b: usize) {
        for k in a..b {
            self.masked[k] = 32;
        }
    }

    /// Blanks a multi-line range, keeping line terminators and registering the line starts inside.
    fn blank_body(&mut self, a: usize, b: usize) {
        let n = self.original.len();
        for k in a..b {
            let c = self.original[k];
            if c == 10 {
                self.line_starts.push(k + 1);
            } else if c == 13 {
                if !(k + 1 < n && self.original[k + 1] == 10) {
                    self.line_starts.push(k + 1);
                }
            } else {
                self.masked[k] = 32;
            }
        }
    }

    /// Handles a backslash at `i` and returns where scanning continues.
    fn command(&mut self, i: usize) -> usize {
        let n = self.original.len();
        if i + 1 >= n {
            return i + 1;
        }
        let d = self.original[i + 1];
        if !is_command_letter(d) {
            if is_line_break(d) {
                return i + 1;
            }
            if is_high_surrogate(d) && i + 2 < n && is_low_surrogate(self.original[i + 2]) {
                return i + 3;
            }
            return i + 2;
        }
        let mut j = i + 2;
        while j < n && is_command_letter(self.original[j]) {
            j += 1;
        }
        let length = j - i - 1;
        if !(3..=10).contains(&length) {
            return j;
        }
        // Cheap pre-filter on the first letter: b(egin) v/V(erb) l(stinline) m(intinline) u(rl) h(ref) i(ffalse) c(atcode)
        if !matches!(d, 98 | 118 | 86 | 108 | 109 | 117 | 104 | 105 | 99) {
            return j;
        }
        let name = &self.original[i + 1..j];
        let kind = if u16_eq(name, "begin") {
            MaskKind::Begin
        } else if u16_eq(name, "verb") || u16_eq(name, "Verb") {
            MaskKind::Verb
        } else if u16_eq(name, "lstinline") {
            MaskKind::Lstinline
        } else if u16_eq(name, "mintinline") {
            MaskKind::Mintinline
        } else if u16_eq(name, "url") || u16_eq(name, "href") {
            MaskKind::RawGroup
        } else if u16_eq(name, "iffalse") {
            MaskKind::Iffalse
        } else {
            return j;
        };
        match kind {
            MaskKind::Begin => self.mask_verbatim_environment(j),
            MaskKind::Verb => self.mask_delimited(j, true).unwrap_or(j),
            MaskKind::Lstinline => self.mask_lstinline(j).unwrap_or(j),
            MaskKind::Mintinline => self.mask_mintinline(j).unwrap_or(j),
            MaskKind::RawGroup => self.mask_raw_group(j).unwrap_or(j),
            MaskKind::Iffalse => self.mask_iffalse(i, j).unwrap_or(j),
        }
    }

    fn mask_verbatim_environment(&mut self, j: usize) -> usize {
        let n = self.original.len();
        let mut k = j;
        while k < n && is_hspace(self.original[k]) {
            k += 1;
        }
        let Some((ns, ne, end_of_name)) = environment_name(&self.original, k) else {
            return j;
        };
        let Some(name) = is_verbatim_environment(&self.original[ns..ne]) else {
            return j;
        };
        let body_start = verbatim_opening(&self.original, name, end_of_name);
        let end = verbatim_end(&self.original, name, body_start);
        if end > body_start {
            self.blank_body(body_start, end);
        }
        end.max(end_of_name)
    }

    /// `\verb|...|`: blanks the delimiters and content (and the star). Returns the next scan position.
    fn mask_delimited(&mut self, j: usize, allow_star: bool) -> Option<usize> {
        let n = self.original.len();
        let mut k = j;
        if allow_star && k < n && self.original[k] == 42 {
            k += 1;
        }
        if k >= n {
            return None;
        }
        let delimiter = self.original[k];
        if is_tex_whitespace(delimiter) {
            return None;
        }
        let mut m = k + 1;
        while m < n && self.original[m] != delimiter && !is_line_break(self.original[m]) {
            m += 1;
        }
        if !(m < n && self.original[m] == delimiter) {
            return None;
        }
        self.blank(j, m + 1);
        Some(m + 1)
    }

    fn mask_lstinline(&mut self, j: usize) -> Option<usize> {
        let n = self.original.len();
        let mut k = j;
        if k < n && self.original[k] == 91 {
            if let Some(close) = closing_bracket_on_line(&self.original, k) {
                k = close + 1;
            }
        }
        if !(k < n) || self.original[k] == 123 {
            return None;
        }
        self.mask_delimited(k, false)
    }

    fn mask_mintinline(&mut self, j: usize) -> Option<usize> {
        let n = self.original.len();
        let mut k = j;
        if k < n && self.original[k] == 91 {
            if let Some(close) = closing_bracket_on_line(&self.original, k) {
                k = close + 1;
            }
        }
        if !(k < n && self.original[k] == 123) {
            return None;
        }
        let lang_close = closing_brace_on_line(&self.original, k)?;
        k = lang_close + 1;
        if !(k < n) || self.original[k] == 123 {
            return None;
        }
        self.mask_delimited(k, false)
    }

    /// `\url{...}`, `\href{...}`: blank the content of the first braced group.
    fn mask_raw_group(&mut self, j: usize) -> Option<usize> {
        let n = self.original.len();
        let mut k = j;
        while k < n && is_hspace(self.original[k]) {
            k += 1;
        }
        if !(k < n && self.original[k] == 123) {
            return None;
        }
        let mut depth = 0;
        let mut m = k;
        let stop = n.min(k + 2000);
        while m < stop {
            let c = self.original[m];
            if c == 123 {
                depth += 1;
            } else if c == 125 {
                depth -= 1;
                if depth == 0 {
                    self.blank_body(k + 1, m);
                    return Some(m + 1);
                }
            } else if is_line_break(c) {
                let mut t = m + 1;
                if c == 13 && t < n && self.original[t] == 10 {
                    t += 1;
                }
                while t < n && is_hspace(self.original[t]) {
                    t += 1;
                }
                if t >= n || is_line_break(self.original[t]) {
                    return None;
                }
            }
            m += 1;
        }
        None
    }

    /// Blanks `\iffalse ... \fi` when `\iffalse` is the first thing on its line.
    fn mask_iffalse(&mut self, command_start: usize, name_end: usize) -> Option<usize> {
        let n = self.original.len();
        let mut k = command_start as isize - 1;
        while k >= 0 && is_hspace(self.original[k as usize]) {
            k -= 1;
        }
        if k >= 0 && !is_line_break(self.original[k as usize]) {
            return None;
        }
        let mut depth = 1;
        let mut m = name_end;
        let mut buf = [0u8; 48];
        while m < n {
            let c = self.original[m];
            if c == 37 {
                while m < n && !is_line_break(self.original[m]) {
                    m += 1;
                }
                continue;
            }
            if c == 92 {
                if m + 1 >= n {
                    break;
                }
                let d = self.original[m + 1];
                if is_command_letter(d) {
                    let mut e = m + 2;
                    while e < n && is_command_letter(self.original[e]) {
                        e += 1;
                    }
                    let name = ascii_name(&self.original[m + 1..e], &mut buf);
                    if name == Some("fi") {
                        depth -= 1;
                        if depth == 0 {
                            self.blank_body(name_end, m);
                            return Some(m);
                        }
                    } else if name.is_some_and(opens_conditional) {
                        depth += 1;
                    }
                    m = e;
                } else {
                    m += 2;
                }
                continue;
            }
            m += 1;
        }
        None
    }
}

// ===================================================================================================
// StructureWalker (Scanning/StructureWalker.swift) – the part completion uses
// ===================================================================================================

#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Def {
    Command,
    Environment,
    DocumentCommand,
    DocumentEnvironment,
    PlainDef,
    Hook(u8),
}

fn definition_shape(name: &str) -> Option<Def> {
    Some(match name {
        "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" | "newrobustcmd" | "renewrobustcmd"
        | "providerobustcmd" => Def::Command,
        "newenvironment" | "renewenvironment" => Def::Environment,
        "NewDocumentCommand" | "RenewDocumentCommand" | "ProvideDocumentCommand" | "DeclareDocumentCommand"
        | "NewExpandableDocumentCommand" => Def::DocumentCommand,
        "NewDocumentEnvironment" | "RenewDocumentEnvironment" | "ProvideDocumentEnvironment" | "DeclareDocumentEnvironment" => {
            Def::DocumentEnvironment
        }
        "def" | "gdef" | "edef" | "xdef" => Def::PlainDef,
        "AtBeginDocument" | "AtEndDocument" | "AtEndPreamble" => Def::Hook(1),
        "AtBeginEnvironment" | "AtEndEnvironment" | "BeforeBeginEnvironment" | "AfterBeginEnvironment"
        | "BeforeEndEnvironment" | "AfterEndEnvironment" | "AddToHook" | "NewEnviron" | "RenewEnviron" | "csdef" | "csgdef"
        | "csedef" | "gappto" | "eappto" | "appto" | "preto" => Def::Hook(2),
        "apptocmd" | "pretocmd" | "patchcmd" => Def::Hook(4),
        _ => return None,
    })
}

fn is_text_mode_command(name: &str) -> bool {
    matches!(
        name,
        "text" | "mbox" | "hbox" | "textrm" | "textit" | "textbf" | "textsf" | "texttt" | "textnormal" | "textup" | "textsl"
            | "textsc" | "intertext" | "shortintertext" | "emph" | "fbox" | "parbox"
    )
}

fn is_math_environment(name: &str) -> bool {
    let base = name.strip_suffix('*').unwrap_or(name);
    matches!(
        base,
        "equation" | "align" | "alignat" | "flalign" | "gather" | "multline" | "eqnarray" | "math" | "displaymath" | "dmath"
    )
}

/// Environments that only make sense inside math (used to rank `\begin{` suggestions).
fn is_inner_math_environment(name: &str) -> bool {
    matches!(
        name,
        "aligned" | "alignedat" | "gathered" | "split" | "cases" | "dcases" | "rcases" | "matrix" | "pmatrix" | "bmatrix"
            | "Bmatrix" | "vmatrix" | "Vmatrix" | "smallmatrix" | "array" | "subarray"
    )
}

struct OpenEnv {
    name: String,
    /// Offset of the `\begin`.
    offset: usize,
}

#[derive(Default)]
struct Walk {
    open_environments: Vec<OpenEnv>,
    inline_math: bool,
    display_math: bool,
    math_environment_depth: i32,
    text_group_depth: i32,
}

impl Walk {
    /// Math mode at the stop position (text groups such as `\text{..}` count as text).
    fn is_in_math(&self) -> bool {
        (self.inline_math || self.display_math || self.math_environment_depth > 0) && self.text_group_depth == 0
    }

    fn pop_environment(&mut self) {
        if let Some(top) = self.open_environments.pop() {
            if is_math_environment(&top.name) {
                self.math_environment_depth -= 1;
            }
        }
    }
}

/// Walks the masked source up to (not including) `limit`.
fn walk(src: &Masked, limit: usize) -> Walk {
    let u = &src.masked;
    let n = u.len();
    let stop = limit.min(n);
    let mut w = Walk::default();
    let mut open_braces: Vec<bool> = Vec::new();
    // 0 none, 1 `$`, 2 `$$`, 3 `\(`, 4 `\[`
    let mut math_mode = 0u8;
    let mut last_text_command_end: Option<usize> = None;
    let mut has_begin_document = false;
    let mut buf = [0u8; 48];
    let mut i = 0;

    while i < stop {
        match u[i] {
            123 => {
                let mut is_text = false;
                if let Some(e) = last_text_command_end {
                    if e <= i {
                        is_text = u[e..i].iter().all(|&c| is_hspace(c));
                    }
                }
                last_text_command_end = None;
                open_braces.push(is_text);
                if is_text {
                    w.text_group_depth += 1;
                }
                i += 1;
            }
            125 => {
                if let Some(true) = open_braces.pop() {
                    w.text_group_depth -= 1;
                }
                i += 1;
            }
            36 => {
                if w.math_environment_depth > 0 || math_mode == 3 || math_mode == 4 {
                    i += 1;
                } else if math_mode == 0 {
                    if i + 1 < n && u[i + 1] == 36 {
                        math_mode = 2;
                        i += 2;
                    } else {
                        math_mode = 1;
                        i += 1;
                    }
                } else if math_mode == 1 {
                    math_mode = 0;
                    i += 1;
                } else if i + 1 < n && u[i + 1] == 36 {
                    math_mode = 0;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            10 | 13 => {
                if src.is_paragraph_break(i) {
                    math_mode = 0;
                }
                if u[i] == 13 && i + 1 < n && u[i + 1] == 10 {
                    i += 1;
                }
                i += 1;
            }
            92 => {
                if i + 1 >= n {
                    i += 1;
                    continue;
                }
                let d = u[i + 1];
                if !is_command_letter(d) {
                    if is_line_break(d) {
                        i += 1;
                        continue;
                    }
                    if d == 40 && math_mode == 0 && w.math_environment_depth == 0 {
                        math_mode = 3;
                    } else if d == 41 && math_mode == 3 {
                        math_mode = 0;
                    } else if d == 91 && math_mode == 0 && w.math_environment_depth == 0 {
                        math_mode = 4;
                    } else if d == 93 && math_mode == 4 {
                        math_mode = 0;
                    }
                    i += if is_high_surrogate(d) && i + 2 < n && is_low_surrogate(u[i + 2]) { 3 } else { 2 };
                    continue;
                }
                let mut j = i + 2;
                while j < n && is_command_letter(u[j]) {
                    j += 1;
                }
                let name = ascii_name(&u[i + 1..j], &mut buf);

                if name == Some("begin") || name == Some("end") {
                    let is_begin = name == Some("begin");
                    let mut k = j;
                    while k < n && is_hspace(u[k]) {
                        k += 1;
                    }
                    // A name cut off by the stop position is not a complete command.
                    let parsed = match environment_name(u, k) {
                        Some(p) if p.2 <= stop => p,
                        _ => {
                            i = j;
                            continue;
                        }
                    };
                    let env_name = String::from_utf16_lossy(&u[parsed.0..parsed.1]);
                    if is_begin {
                        if env_name == "document" && w.open_environments.is_empty() {
                            has_begin_document = true;
                        }
                        if is_math_environment(&env_name) {
                            w.math_environment_depth += 1;
                        }
                        w.open_environments.push(OpenEnv { name: env_name, offset: i });
                    } else if let Some(top) = w.open_environments.last() {
                        if top.name == env_name {
                            w.pop_environment();
                        } else if top.name == "document" {
                            // Stray `\end{x}` with only the document open: ignored.
                        } else if let Some(pos) = w.open_environments.iter().rposition(|e| e.name == env_name) {
                            while w.open_environments.len() > pos {
                                w.pop_environment();
                            }
                        } else {
                            w.pop_environment();
                        }
                        if env_name == "document" && has_begin_document && w.open_environments.is_empty() {
                            break;
                        }
                    }
                    i = parsed.2;
                    continue;
                }

                if let Some(nm) = name {
                    if let Some(shape) = definition_shape(nm) {
                        if let Some(end) = definition_end(u, shape, j) {
                            if end <= stop {
                                i = end;
                                continue;
                            }
                        }
                    }
                    if is_text_mode_command(nm) {
                        last_text_command_end = Some(j);
                    }
                }
                i = j;
            }
            _ => i += 1,
        }
    }
    w.inline_math = math_mode == 1 || math_mode == 3;
    w.display_math = math_mode == 2 || math_mode == 4;
    w
}

/// End offset of a complete definition (name, parameters and all bodies), or `None` when it is malformed.
fn definition_end(u: &[u16], shape: Def, from: usize) -> Option<usize> {
    let n = u.len();
    let mut p = skip_whitespace(u, from);
    match shape {
        Def::Hook(groups) => {
            // Optional `[...]`, an optional bare control sequence (`\appto\cmd{...}`), then up to `groups` braced groups.
            let mut end = p;
            let mut found = 0;
            let mut guard_count = 0;
            while found < groups && guard_count < 8 {
                guard_count += 1;
                end = skip_whitespace(u, end);
                if end < n && u[end] == 91 {
                    if let Some(e) = bracketed(u, end, 400) {
                        end = e;
                        continue;
                    }
                }
                if end < n && u[end] == 92 && found == 0 {
                    if let Some(e) = control_sequence_end(u, end) {
                        end = e;
                        continue;
                    }
                }
                if end < n && u[end] == 123 {
                    if let Some(e) = braced(u, end, MAX_ARGUMENT_LENGTH) {
                        end = e;
                        found += 1;
                        continue;
                    }
                }
                break;
            }
            if found > 0 {
                Some(end)
            } else {
                None
            }
        }
        Def::PlainDef => {
            if !(p < n && u[p] == 92) {
                return None;
            }
            let mut k = control_sequence_end(u, p)?;
            let stop = n.min(k + 120);
            while k < stop && u[k] != 123 {
                if is_line_break(u[k]) && k + 1 < n && is_line_break(u[k + 1]) {
                    return None;
                }
                k += 1;
            }
            if k >= stop {
                return None;
            }
            braced(u, k, MAX_ARGUMENT_LENGTH)
        }
        Def::Command | Def::Environment | Def::DocumentCommand | Def::DocumentEnvironment => {
            if p < n && u[p] == 42 {
                p = skip_whitespace(u, p + 1);
            }
            // Name: {\foo}, \foo or {name}
            if p < n && u[p] == 123 {
                p = braced(u, p, 300)?;
            } else if (shape == Def::Command || shape == Def::DocumentCommand) && p < n && u[p] == 92 {
                p = control_sequence_end(u, p)?;
            } else {
                return None;
            }
            p = skip_whitespace(u, p);
            let mut bodies = if shape == Def::Environment || shape == Def::DocumentEnvironment { 2 } else { 1 };
            if shape == Def::DocumentCommand || shape == Def::DocumentEnvironment {
                if !(p < n && u[p] == 123) {
                    return None;
                }
                let spec_end = braced(u, p, 400)?;
                p = skip_whitespace(u, spec_end);
            } else {
                let mut optionals = 0;
                while optionals < 2 && p < n && u[p] == 91 {
                    match bracketed(u, p, 400) {
                        Some(e) => {
                            p = skip_whitespace(u, e);
                            optionals += 1;
                        }
                        None => break,
                    }
                }
            }
            let mut end = p;
            while bodies > 0 {
                if !(end < n && u[end] == 123) {
                    return None;
                }
                let body_end = braced(u, end, MAX_ARGUMENT_LENGTH)?;
                end = skip_whitespace(u, body_end);
                bodies -= 1;
                if bodies > 0 {
                    continue;
                }
                return Some(body_end);
            }
            None
        }
    }
}

// ===================================================================================================
// CompletionContext (Completion/CompletionContext.swift)
// ===================================================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// `\input`, `\subfile`: `.tex` files, extension kept.
    TexWithExtension,
    /// `\include`: `.tex` files, extension dropped.
    TexWithoutExtension,
    Image,
    Pdf,
    /// `\bibliography{a,b}`: `.bib` files without extension (list).
    Bibliography,
    /// `\addbibresource{refs.bib}`.
    BibResource,
}

/// Where the cursor is, as far as completion is concerned. Offsets are UTF-16 positions in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionContext {
    /// `\fr|`: the command starts at `start` (the backslash).
    Command { start: usize },
    /// `\begin{ite|` / `\end{|`: the name being typed starts at `item_start`.
    Environment { is_end: bool, item_start: usize },
    /// `\ref{|`, `\cref{a,|`...
    Label { item_start: usize },
    /// `\cite{|` and natbib / biblatex variants (comma lists).
    Citation { item_start: usize },
    /// `\usepackage{|`, `\RequirePackage{|` (comma lists).
    Package { item_start: usize },
    File { kind: FileKind, item_start: usize },
}

impl CompletionContext {
    /// Start offset of the item being typed.
    pub fn start(&self) -> usize {
        match *self {
            CompletionContext::Command { start } => start,
            CompletionContext::Environment { item_start, .. }
            | CompletionContext::Label { item_start }
            | CompletionContext::Citation { item_start }
            | CompletionContext::Package { item_start }
            | CompletionContext::File { item_start, .. } => item_start,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            CompletionContext::Command { .. } => "command",
            CompletionContext::Environment { is_end: false, .. } => "environment",
            CompletionContext::Environment { is_end: true, .. } => "endEnvironment",
            CompletionContext::Label { .. } => "label",
            CompletionContext::Citation { .. } => "citation",
            CompletionContext::Package { .. } => "package",
            CompletionContext::File { .. } => "file",
        }
    }
}

enum Argument {
    Environment { is_end: bool },
    Label { is_list: bool },
    Citation,
    Package,
    File(FileKind),
}

fn argument_for_command(name: &[u16]) -> Option<Argument> {
    let mut buf = [0u8; 48];
    if let Some(s) = ascii_name(name, &mut buf) {
        match s {
            "begin" => return Some(Argument::Environment { is_end: false }),
            "end" => return Some(Argument::Environment { is_end: true }),
            "usepackage" | "RequirePackage" => return Some(Argument::Package),
            "input" | "subfile" => return Some(Argument::File(FileKind::TexWithExtension)),
            "include" => return Some(Argument::File(FileKind::TexWithoutExtension)),
            "includegraphics" => return Some(Argument::File(FileKind::Image)),
            "includepdf" => return Some(Argument::File(FileKind::Pdf)),
            "bibliography" => return Some(Argument::File(FileKind::Bibliography)),
            "addbibresource" => return Some(Argument::File(FileKind::BibResource)),
            "ref" | "eqref" | "autoref" | "pageref" | "nameref" | "vref" | "Vref" | "hyperref" | "crefrange" | "Crefrange" => {
                return Some(Argument::Label { is_list: false })
            }
            "cref" | "Cref" | "labelcref" | "cpageref" | "Cpageref" => return Some(Argument::Label { is_list: true }),
            _ => {}
        }
    }
    if is_cite_like(name) {
        return Some(Argument::Citation);
    }
    None
}

/// Analyses the masked text around `cursor`. Comments and verbatim are blanked in `u`, so they never
/// produce a context.
fn detect(u: &[u16], cursor: usize) -> Option<CompletionContext> {
    let n = u.len();
    if cursor > n {
        return None;
    }

    // 1. `\letters` immediately before the cursor.
    let mut j = cursor;
    while j > 0 && is_command_letter(u[j - 1]) {
        j -= 1;
    }
    if j > 0 && u[j - 1] == 92 {
        let mut backslashes = 0;
        let mut k = j as isize - 1;
        while k >= 0 && u[k as usize] == 92 {
            backslashes += 1;
            k -= 1;
        }
        return if backslashes % 2 == 1 { Some(CompletionContext::Command { start: j - 1 }) } else { None };
    }

    // 2. Inside the braces of a known command's argument.
    let lowest = cursor.saturating_sub(2_500);
    let mut k = cursor;
    let mut breaks = 0;
    while k > lowest {
        let c = u[k - 1];
        if c == 123 {
            break;
        }
        if c == 125 || c == 92 {
            return None;
        }
        if is_line_break(c) {
            if !(c == 13 && k < n && u[k] == 10) {
                breaks += 1;
            }
            if breaks >= 2 {
                return None;
            }
        } else if !is_hspace(c) {
            breaks = 0;
        }
        k -= 1;
    }
    if !(k > lowest && u[k - 1] == 123) {
        return None;
    }
    let open = k - 1;

    let skip_spaces_back = |position: usize| {
        let mut q = position;
        while q > 0 && is_hspace(u[q - 1]) {
            q -= 1;
        }
        q
    };

    let mut p = skip_spaces_back(open);
    let mut groups = 0;
    while p > 0 && u[p - 1] == 93 && groups < 4 {
        let mut depth = 0;
        let mut q = p as isize - 1;
        let floor = p.saturating_sub(600) as isize;
        let mut found: isize = -1;
        while q >= floor {
            let c = u[q as usize];
            if c == 93 {
                depth += 1;
            } else if c == 91 {
                depth -= 1;
                if depth == 0 {
                    found = q;
                    break;
                }
            }
            q -= 1;
        }
        if found < 0 {
            return None;
        }
        p = skip_spaces_back(found as usize);
        groups += 1;
    }
    if p > 0 && u[p - 1] == 42 {
        p -= 1;
    }
    let name_end = p;
    let mut name_start = p;
    while name_start > 0 && is_command_letter(u[name_start - 1]) {
        name_start -= 1;
    }
    if !(name_start < name_end && name_start > 0 && u[name_start - 1] == 92) {
        return None;
    }
    let mut backslashes = 0;
    let mut b = name_start as isize - 1;
    while b >= 0 && u[b as usize] == 92 {
        backslashes += 1;
        b -= 1;
    }
    if backslashes % 2 != 1 {
        return None;
    }
    let argument = argument_for_command(&u[name_start..name_end])?;

    // Start of the item being typed.
    let item_start = |is_list: bool| {
        let mut start = open + 1;
        if is_list {
            let mut q = cursor;
            while q > open + 1 {
                if u[q - 1] == 44 {
                    start = q;
                    break;
                }
                q -= 1;
            }
        }
        while start < cursor && is_tex_whitespace(u[start]) {
            start += 1;
        }
        start
    };

    Some(match argument {
        Argument::Environment { is_end } => CompletionContext::Environment { is_end, item_start: item_start(false) },
        Argument::Label { is_list } => CompletionContext::Label { item_start: item_start(is_list) },
        Argument::Citation => CompletionContext::Citation { item_start: item_start(true) },
        Argument::Package => CompletionContext::Package { item_start: item_start(true) },
        Argument::File(kind) => CompletionContext::File { kind, item_start: item_start(kind == FileKind::Bibliography) },
    })
}

/// Detects the completion context at `cursor` (UTF-16 offset) of `text`; comments, verbatim bodies, `\verb`,
/// `\url` arguments and `\iffalse` blocks never produce a context.
pub fn detect_context(text: &str, cursor: usize) -> Option<CompletionContext> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let src = Masked::new(units);
    detect(&src.masked, cursor)
}

// ===================================================================================================
// Ranking (Completion/CompletionRanking.swift)
// ===================================================================================================

#[inline]
fn is_separator(b: u8) -> bool {
    matches!(b, 58 | 45 | 95 | 46 | 47 | 32)
}

/// Tiers (lower is better): 0 exact, 1 case-sensitive prefix, 2 case-insensitive prefix, 3 prefix of a
/// "word" inside the candidate (after `: - _ . / space`), 4 subsequence (fuzzy), 5 substring of secondary text.
/// `scratch` holds the lowercased candidate (reused between calls).
fn tier(typed: &str, typed_lower: &str, candidate: &str, allow_fuzzy: bool, scratch: &mut Vec<u8>) -> Option<u8> {
    if typed.is_empty() {
        return Some(1);
    }
    if candidate == typed {
        return Some(0);
    }
    if candidate.starts_with(typed) {
        return Some(1);
    }
    scratch.clear();
    if candidate.is_ascii() {
        scratch.extend(candidate.bytes().map(|b| b.to_ascii_lowercase()));
    } else {
        scratch.extend_from_slice(candidate.to_lowercase().as_bytes());
    }
    let hay: &[u8] = scratch;
    let needle = typed_lower.as_bytes();
    if hay.starts_with(needle) {
        return Some(2);
    }
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    // Word-start prefix.
    let mut i = 1;
    while i + needle.len() <= hay.len() {
        if is_separator(hay[i - 1]) && !is_separator(hay[i]) && hay[i] == needle[0] && hay[i..i + needle.len()] == *needle {
            return Some(3);
        }
        i += 1;
    }
    if !allow_fuzzy {
        return None;
    }
    // Subsequence.
    let mut h = 0;
    for &byte in needle {
        while h < hay.len() && hay[h] != byte {
            h += 1;
        }
        if h == hay.len() {
            return None;
        }
        h += 1;
    }
    Some(4)
}

/// Filters `items` by `typed` and orders them best first (ties: `priority` ascending, then candidate order).
fn rank<'a>(
    items: &[&'a CompletionItem],
    typed: &str,
    allow_fuzzy: bool,
    limit: usize,
    key: &dyn Fn(&'a CompletionItem) -> &'a str,
    secondary: Option<&dyn Fn(usize, &'a CompletionItem) -> Option<&'a str>>,
    priority: &dyn Fn(&'a CompletionItem) -> i64,
) -> Vec<&'a CompletionItem> {
    let typed_lower = typed.to_lowercase();
    let mut scratch: Vec<u8> = Vec::with_capacity(64);
    let mut scored: Vec<(u8, i64, usize)> = Vec::with_capacity(items.len().min(512));
    for (index, item) in items.iter().enumerate() {
        let mut score = tier(typed, &typed_lower, key(item), allow_fuzzy, &mut scratch);
        if score.is_none() {
            if let Some(sec) = secondary {
                if let Some(text) = sec(index, item) {
                    if text.to_lowercase().contains(&typed_lower) {
                        score = Some(5);
                    }
                }
            }
        }
        if let Some(s) = score {
            scored.push((s, priority(item), index));
        }
    }
    scored.sort_unstable();
    scored.iter().take(limit).map(|&(_, _, index)| items[index]).collect()
}

// ===================================================================================================
// Engine (Completion/CompletionEngine.swift)
// ===================================================================================================

/// The most frequently typed text-mode commands, best first (they win ties and fill a bare `\`).
pub const COMMON_TEXT_COMMANDS: [&str; 56] = [
    "\\begin", "\\end", "\\section", "\\subsection", "\\subsubsection", "\\textbf", "\\textit", "\\emph", "\\label", "\\ref",
    "\\cite", "\\item", "\\usepackage", "\\documentclass", "\\includegraphics", "\\caption", "\\footnote", "\\input", "\\include",
    "\\newcommand", "\\chapter", "\\paragraph", "\\title", "\\author", "\\date", "\\maketitle", "\\tableofcontents",
    "\\bibliography", "\\bibliographystyle", "\\texttt", "\\underline", "\\centering", "\\hline", "\\hspace", "\\vspace", "\\url",
    "\\href", "\\eqref", "\\cref", "\\autoref", "\\newpage", "\\noindent", "\\textsc", "\\textsf", "\\textrm",
    "\\printbibliography", "\\addbibresource", "\\newtheorem", "\\newenvironment", "\\renewcommand", "\\providecommand", "\\def",
    "\\tiny", "\\small", "\\large", "\\Large",
];
// `\today` is the 57th entry in Swift; kept separately so the array above stays a plain const.
const COMMON_TEXT_EXTRA: [&str; 1] = ["\\today"];

/// The most frequently typed commands inside math, best first.
pub const COMMON_MATH_COMMANDS: [&str; 59] = [
    "\\frac", "\\sum", "\\int", "\\alpha", "\\beta", "\\gamma", "\\delta", "\\epsilon", "\\varepsilon", "\\theta", "\\lambda", "\\mu",
    "\\pi", "\\sigma", "\\phi", "\\omega", "\\infty", "\\partial", "\\nabla", "\\mathbb", "\\mathcal", "\\mathbf", "\\mathrm",
    "\\left", "\\right", "\\cdot", "\\cdots", "\\ldots", "\\leq", "\\geq", "\\neq", "\\approx", "\\in", "\\subset", "\\subseteq",
    "\\cup", "\\cap", "\\to", "\\mapsto", "\\rightarrow", "\\Rightarrow", "\\sqrt", "\\lim", "\\prod", "\\forall", "\\exists",
    "\\text", "\\times", "\\otimes", "\\oplus", "\\langle", "\\rangle", "\\hat", "\\bar", "\\vec", "\\tilde", "\\dot",
    "\\operatorname", "\\quad",
];
const COMMON_MATH_EXTRA: [&str; 4] = ["\\qquad", "\\begin", "\\end", "\\label"];

/// Environments people open most often, best first.
pub const COMMON_ENVIRONMENTS: [&str; 19] = [
    "document", "itemize", "enumerate", "equation", "align", "figure", "table", "tabular", "center", "theorem", "lemma", "proof",
    "abstract", "description", "verbatim", "minipage", "cases", "matrix", "frame",
];

fn rank_table(lists: &[&[&'static str]]) -> HashMap<&'static str, i64> {
    let mut map = HashMap::new();
    let mut index = 0i64;
    for list in lists {
        for &name in *list {
            map.entry(name).or_insert(index);
            index += 1;
        }
    }
    map
}

fn common_text_rank() -> &'static HashMap<&'static str, i64> {
    static T: OnceLock<HashMap<&'static str, i64>> = OnceLock::new();
    T.get_or_init(|| rank_table(&[&COMMON_TEXT_COMMANDS, &COMMON_TEXT_EXTRA]))
}
fn common_math_rank() -> &'static HashMap<&'static str, i64> {
    static T: OnceLock<HashMap<&'static str, i64>> = OnceLock::new();
    T.get_or_init(|| rank_table(&[&COMMON_MATH_COMMANDS, &COMMON_MATH_EXTRA]))
}
fn common_environment_rank() -> &'static HashMap<&'static str, i64> {
    static T: OnceLock<HashMap<&'static str, i64>> = OnceLock::new();
    T.get_or_init(|| rank_table(&[&COMMON_ENVIRONMENTS]))
}

const DEFINED_IN_PROJECT: &str = "Defined in project";

/// `\begin` and `\end` are environment openers handled by the environment completion; the catalog tables
/// do not list them, so the engine supplies them.
fn core_commands(innermost: Option<&str>) -> [CompletionItem; 2] {
    let mut end = CompletionItem::new(
        Kind::Command,
        "\\end",
        Some("\\end{${1:environment}}"),
        Some("End an environment"),
        None,
    );
    if let Some(name) = innermost {
        if !name.is_empty() {
            end.insert_text = format!("\\end{{{name}}}");
            end.detail = Some(format!("Close the environment \u{201C}{name}\u{201D}"));
        }
    }
    [
        CompletionItem::new(Kind::Command, "\\begin", Some("\\begin{${1:environment}}"), Some("Begin an environment"), None),
        end,
    ]
}

fn after_first_char(label: &str) -> &str {
    let mut it = label.chars();
    it.next();
    it.as_str()
}

fn emit(item: &CompletionItem, convert: bool) -> CompletionItem {
    let mut out = item.clone();
    if convert {
        if let Cow::Owned(c) = to_codemirror(&out.insert_text) {
            out.insert_text = c;
        }
    }
    out
}

fn command_items(typed: &str, in_math: bool, sym: &SymbolsRef, innermost: Option<&str>) -> Vec<CompletionItem> {
    let cat = catalog();
    let mut custom: Vec<&CompletionItem> = Vec::new();
    let mut custom_labels: HashSet<&str> = HashSet::new();
    for item in sym.custom_commands {
        if custom_labels.insert(item.label.as_str()) {
            custom.push(item);
        }
    }
    let core_all = core_commands(innermost);
    let core: Vec<&CompletionItem> = core_all.iter().filter(|c| !custom_labels.contains(c.label.as_str())).collect();
    let core_labels: HashSet<&str> = core.iter().map(|c| c.label.as_str()).collect();
    let usable = |i: &&CompletionItem| !custom_labels.contains(i.label.as_str()) && !core_labels.contains(i.label.as_str());

    let mut ordered: Vec<&CompletionItem> = Vec::with_capacity(custom.len() + core.len() + cat.commands.len() + cat.math_commands.len());
    ordered.extend(&custom);
    ordered.extend(&core);
    if in_math {
        ordered.extend(cat.math_commands.iter().filter(usable));
        ordered.extend(cat.commands.iter().filter(usable).filter(|i| !cat.math_labels.contains(&i.label)));
    } else {
        ordered.extend(cat.commands.iter().filter(usable));
        ordered.extend(cat.math_commands.iter().filter(usable).filter(|i| !cat.text_labels.contains(&i.label)));
    }

    let common_rank = if in_math { common_math_rank() } else { common_text_rank() };
    let body = after_first_char(typed);
    let limit = if body.is_empty() { BARE_BACKSLASH_LIMIT } else { MAX_ITEMS };
    let ranked = rank(
        &ordered,
        body,
        body.chars().count() >= 2,
        limit,
        &|i| after_first_char(&i.label),
        None,
        &|i| {
            if custom_labels.contains(i.label.as_str()) {
                -1
            } else {
                common_rank.get(i.label.as_str()).copied().unwrap_or(100_000)
            }
        },
    );
    ranked.into_iter().map(|i| emit(i, custom_labels.contains(i.label.as_str()))).collect()
}

fn environment_candidates(sym: &SymbolsRef) -> (Vec<CompletionItem>, HashSet<String>) {
    let mut custom = Vec::new();
    let mut names: HashSet<String> = HashSet::new();
    for name in sym.custom_environments {
        if names.insert(name.clone()) {
            custom.push(CompletionItem {
                kind: Kind::Environment,
                label: name.clone(),
                insert_text: format!("{name}}}\n\t${{}}\n\\end{{{name}}}"),
                detail: Some(DEFINED_IN_PROJECT.to_string()),
                package: None,
            });
        }
    }
    (custom, names)
}

fn begin_environment_items(typed: &str, in_math: bool, sym: &SymbolsRef) -> Vec<CompletionItem> {
    let (custom, names) = environment_candidates(sym);
    let cat = catalog();
    let mut candidates: Vec<&CompletionItem> = custom.iter().collect();
    candidates.extend(cat.environments.iter().filter(|i| !names.contains(&i.label)));
    let common = common_environment_rank();
    let custom_set: HashSet<&str> = sym.custom_environments.iter().map(String::as_str).collect();
    let ranked = rank(&candidates, typed, true, MAX_ITEMS, &|i| i.label.as_str(), None, &|i| {
        let c = common.get(i.label.as_str()).copied().unwrap_or(500);
        if custom_set.contains(i.label.as_str()) {
            return -1;
        }
        // Inside math, environments that only make sense there (aligned, cases, matrices...) come first.
        if in_math && !is_inner_math_environment(&i.label) {
            1_000 + c
        } else {
            c
        }
    });
    ranked.into_iter().cloned().collect()
}

fn end_environment_items(typed: &str, w: &Walk, src: &Masked, sym: &SymbolsRef) -> Vec<CompletionItem> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut unclosed: Vec<CompletionItem> = Vec::new();
    for open in w.open_environments.iter().rev() {
        if seen.insert(open.name.as_str()) {
            let line = src.line_at(open.offset);
            unclosed.push(CompletionItem {
                kind: Kind::Environment,
                label: open.name.clone(),
                insert_text: format!("{}}}", open.name),
                detail: Some(format!("Opened on line {line}")),
                package: None,
            });
        }
    }
    let refs: Vec<&CompletionItem> = unclosed.iter().collect();
    let ranked = rank(&refs, typed, true, MAX_ITEMS, &|i| i.label.as_str(), None, &|_| 0);
    if !ranked.is_empty() {
        return ranked.into_iter().cloned().collect();
    }
    // Nothing open matches what was typed: fall back to every known environment name.
    let (custom, names) = environment_candidates(sym);
    let cat = catalog();
    let all: Vec<CompletionItem> = custom
        .iter()
        .chain(cat.environments.iter().filter(|i| !names.contains(&i.label)))
        .map(|i| CompletionItem {
            kind: Kind::Environment,
            label: i.label.clone(),
            insert_text: format!("{}}}", i.label),
            detail: i.detail.clone(),
            package: i.package.clone(),
        })
        .collect();
    let refs: Vec<&CompletionItem> = all.iter().collect();
    rank(&refs, typed, true, MAX_ITEMS, &|i| i.label.as_str(), None, &|_| 0)
        .into_iter()
        .cloned()
        .collect()
}

/// Last path component for `/` and `\` separated paths.
fn last_path_component(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn label_items(typed: &str, sym: &SymbolsRef) -> Vec<CompletionItem> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut items = Vec::new();
    for label in sym.labels {
        if seen.insert(label.key.as_str()) {
            let detail = match &label.context {
                Some(c) => c.clone(),
                None => format!("{}:{}", last_path_component(&label.path), label.line),
            };
            items.push(CompletionItem {
                kind: Kind::Label,
                label: label.key.clone(),
                insert_text: label.key.clone(),
                detail: Some(detail),
                package: None,
            });
        }
    }
    let refs: Vec<&CompletionItem> = items.iter().collect();
    rank(&refs, typed, true, MAX_ITEMS, &|i| i.label.as_str(), None, &|_| 0)
        .into_iter()
        .cloned()
        .collect()
}

fn citation_items(typed: &str, sym: &SymbolsRef) -> Vec<CompletionItem> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut items = Vec::new();
    let mut metadata: Vec<String> = Vec::new();
    for entry in sym.bib_entries {
        if !seen.insert(entry.key.as_str()) {
            continue;
        }
        let mut summary = String::new();
        if let Some(author) = &entry.author {
            if !author.is_empty() {
                summary = author.clone();
            }
        }
        if let Some(year) = &entry.year {
            if !year.is_empty() {
                if summary.is_empty() {
                    summary = format!("({year})");
                } else {
                    summary.push_str(&format!(" ({year})"));
                }
            }
        }
        if let Some(title) = &entry.title {
            if !title.is_empty() {
                if summary.is_empty() {
                    summary = title.clone();
                } else {
                    summary.push_str(&format!(" \u{2014} {title}"));
                }
            }
        }
        let detail = if summary.is_empty() { entry.entry_type.clone() } else { Some(summary.clone()) };
        items.push(CompletionItem {
            kind: Kind::Citation,
            label: entry.key.clone(),
            insert_text: entry.key.clone(),
            detail,
            package: None,
        });
        metadata.push(summary);
    }
    let refs: Vec<&CompletionItem> = items.iter().collect();
    let secondary = |index: usize, _: &CompletionItem| Some(metadata[index].as_str());
    rank(&refs, typed, true, MAX_ITEMS, &|i| i.label.as_str(), Some(&secondary), &|_| 0)
        .into_iter()
        .cloned()
        .collect()
}

fn package_items(typed: &str, sym: &SymbolsRef) -> Vec<CompletionItem> {
    let cat = catalog();
    let known: HashSet<&str> = cat.packages.iter().map(|p| p.label.as_str()).collect();
    // Swift inserts every local package at index 0 in file order, i.e. they end up reversed.
    let mut locals: Vec<CompletionItem> = Vec::new();
    for path in sym.project_files {
        if let Some(name) = path.strip_suffix(".sty") {
            if !known.contains(name) {
                locals.push(CompletionItem {
                    kind: Kind::Package,
                    label: name.to_string(),
                    insert_text: name.to_string(),
                    detail: Some("Local package".to_string()),
                    package: None,
                });
            }
        }
    }
    let mut candidates: Vec<&CompletionItem> = locals.iter().rev().collect();
    candidates.extend(cat.packages.iter());
    rank(&candidates, typed, true, MAX_ITEMS, &|i| i.label.as_str(), None, &|_| 0)
        .into_iter()
        .cloned()
        .collect()
}

fn has_extension(path: &str, extensions: &[&str]) -> bool {
    let b = path.as_bytes();
    extensions.iter().any(|ext| {
        let e = ext.as_bytes();
        b.len() > e.len() && b[b.len() - e.len() - 1] == b'.' && b[b.len() - e.len()..].eq_ignore_ascii_case(e)
    })
}

fn file_items(typed: &str, kind: FileKind, sym: &SymbolsRef) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for raw in sym.project_files {
        let path: Cow<str> = if raw.contains('\\') { Cow::Owned(raw.replace('\\', "/")) } else { Cow::Borrowed(raw) };
        let path: &str = &path;
        let insert: &str = match kind {
            FileKind::TexWithExtension => {
                if !has_extension(path, &["tex"]) {
                    continue;
                }
                path
            }
            FileKind::TexWithoutExtension => {
                if !has_extension(path, &["tex"]) {
                    continue;
                }
                &path[..path.len() - 4]
            }
            FileKind::Image => {
                if !has_extension(path, &["pdf", "png", "jpg", "jpeg", "eps"]) {
                    continue;
                }
                path
            }
            FileKind::Pdf => {
                if !has_extension(path, &["pdf"]) {
                    continue;
                }
                path
            }
            FileKind::Bibliography => {
                if !has_extension(path, &["bib"]) {
                    continue;
                }
                &path[..path.len() - 4]
            }
            FileKind::BibResource => {
                if !has_extension(path, &["bib"]) {
                    continue;
                }
                path
            }
        };
        items.push(CompletionItem {
            kind: Kind::File,
            label: insert.to_string(),
            insert_text: insert.to_string(),
            detail: None,
            package: None,
        });
    }
    let refs: Vec<&CompletionItem> = items.iter().collect();
    rank(&refs, typed, true, MAX_ITEMS, &|i| i.label.as_str(), None, &|_| 0)
        .into_iter()
        .cloned()
        .collect()
}

fn units_string(u: &[u16], from: usize, to: usize) -> String {
    String::from_utf16_lossy(&u[from..to])
}

fn complete_masked(src: &Masked, cursor: usize, sym: &SymbolsRef) -> Option<CompletionResult> {
    let u = &src.masked;
    if cursor > u.len() {
        return None;
    }
    let context = detect(u, cursor)?;
    let name = context.name();
    let mut in_math = false;
    let (from, to, typed, items) = match context {
        CompletionContext::Command { start } => {
            let typed = units_string(u, start, cursor);
            let w = walk(src, start);
            in_math = w.is_in_math();
            let items = command_items(&typed, in_math, sym, w.open_environments.last().map(|e| e.name.as_str()));
            (start, cursor, typed, items)
        }
        CompletionContext::Environment { is_end, item_start } => {
            let typed = units_string(u, item_start, cursor);
            let w = walk(src, item_start);
            in_math = w.is_in_math();
            let items = if is_end {
                end_environment_items(&typed, &w, src, sym)
            } else {
                begin_environment_items(&typed, in_math, sym)
            };
            // The inserted text ends with the closing brace: swallow an existing one (and the rest of the name).
            let mut end = cursor;
            let mut k = cursor;
            while k < u.len() && (is_command_letter(u[k]) || is_digit(u[k]) || u[k] == 42) {
                k += 1;
            }
            if k < u.len() && u[k] == 125 {
                end = k + 1;
            }
            (item_start, end, typed, items)
        }
        CompletionContext::Label { item_start } => {
            let typed = units_string(u, item_start, cursor);
            let items = label_items(&typed, sym);
            (item_start, cursor, typed, items)
        }
        CompletionContext::Citation { item_start } => {
            let typed = units_string(u, item_start, cursor);
            let items = citation_items(&typed, sym);
            (item_start, cursor, typed, items)
        }
        CompletionContext::Package { item_start } => {
            let typed = units_string(u, item_start, cursor);
            let items = package_items(&typed, sym);
            (item_start, cursor, typed, items)
        }
        CompletionContext::File { kind, item_start } => {
            let typed = units_string(u, item_start, cursor);
            let items = file_items(&typed, kind, sym);
            (item_start, cursor, typed, items)
        }
    };
    if items.is_empty() {
        return None;
    }
    let mut items = items;
    items.truncate(MAX_ITEMS);
    Some(CompletionResult { from, to, prefix: typed, context: name.to_string(), in_math, items })
}

/// Completion at `cursor` (UTF-16 offset into `text`). `None` when there is nothing to offer.
pub fn complete(text: &str, cursor: usize, symbols: &SymbolsRef) -> Option<CompletionResult> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if cursor > units.len() {
        return None;
    }
    let src = Masked::new(units);
    complete_masked(&src, cursor, symbols)
}

/// Completion at a 1-based `line` / UTF-16 `column` (CodeMirror position + 1). A column past the end of the
/// line is clamped to the line end; a line past the end of the text yields `None`.
pub fn complete_at(text: &str, line: u32, column: u32, symbols: &SymbolsRef) -> Option<CompletionResult> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let src = Masked::new(units);
    let cursor = offset_of(&src, line, column)?;
    complete_masked(&src, cursor, symbols)
}

fn offset_of(src: &Masked, line: u32, column: u32) -> Option<usize> {
    let line = line.max(1) as usize;
    let start = *src.line_starts.get(line - 1)?;
    let o = &src.original;
    let mut end = start;
    while end < o.len() && !is_line_break(o[end]) {
        end += 1;
    }
    Some((start + (column.max(1) as usize - 1)).min(end))
}

/// The IPC entry point: items only, ranked like the Swift engine (empty when nothing applies).
pub fn completions(
    text: &str,
    line: u32,
    column: u32,
    labels: &[LabelInfo],
    bib_keys: &[BibInfo],
    project_files: &[String],
) -> Vec<CompletionItem> {
    let sym = SymbolsRef { labels, bib_entries: bib_keys, project_files, ..SymbolsRef::default() };
    complete_at(text, line, column, &sym).map(|r| r.items).unwrap_or_default()
}

// ===================================================================================================
// Tests (TeXCoreChecks: CompletionChecks + SnippetChecks)
// ===================================================================================================

#[allow(clippy::field_reassign_with_default)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const MARK: char = '\u{2038}';

    fn label(key: &str, path: &str, line: u32, context: Option<&str>) -> LabelInfo {
        LabelInfo { key: key.into(), path: path.into(), line, context: context.map(str::to_string) }
    }

    fn bib(key: &str, ty: &str, title: Option<&str>, author: Option<&str>, year: Option<&str>) -> BibInfo {
        BibInfo {
            key: key.into(),
            entry_type: Some(ty.into()),
            title: title.map(str::to_string),
            author: author.map(str::to_string),
            year: year.map(str::to_string),
        }
    }

    fn custom(label: &str, insert: &str) -> CompletionItem {
        CompletionItem::new(Kind::Command, label, Some(insert), Some("Defined in project"), None)
    }

    fn test_symbols() -> ProjectSymbols {
        ProjectSymbols {
            labels: vec![
                label("sec:intro", "/p/main.tex", 3, Some("Introduction")),
                label("eq:main", "/p/main.tex", 10, Some("equation")),
                label("eq:other", "/p/sections/a.tex", 4, Some("align")),
                label("fig:one", "/p/main.tex", 20, None),
            ],
            bib_entries: vec![
                bib("knuth1984", "book", Some("The Art of Computer Programming"), Some("Knuth, Donald"), Some("1984")),
                bib("lamport94", "book", Some("LaTeX: A Document Preparation System"), Some("Lamport, Leslie"), Some("1994")),
                bib("hatcher2002", "book", Some("Algebraic Topology"), Some("Hatcher, Allen"), Some("2002")),
                bib("bare", "misc", None, None, None),
            ],
            custom_commands: vec![custom("\\R", "\\R"), custom("\\norm", "\\norm{${1:arg}}"), custom("\\vec", "\\vec{${1:arg}}")],
            custom_environments: vec!["myenv".into()],
            project_files: [
                "main.tex", "sections/intro.tex", "sections/methods.tex", "figs/plot.pdf", "figs/photo.png", "refs.bib",
                "extra/more.bib", "mystyle.sty",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }

    /// Splits at the cursor marker; the cursor is a UTF-16 offset.
    fn split_cursor(marked: &str) -> (String, usize) {
        let pos = marked.find(MARK).expect("missing cursor marker");
        let text = format!("{}{}", &marked[..pos], &marked[pos + MARK.len_utf8()..]);
        (text, marked[..pos].encode_utf16().count())
    }

    #[allow(dead_code)]
    struct Run {
        result: Option<CompletionResult>,
        text: String,
        cursor: usize,
    }

    fn run(marked: &str, sym: &ProjectSymbols) -> Run {
        let (text, cursor) = split_cursor(marked);
        let result = complete(&text, cursor, &sym.as_ref());
        Run { result, text, cursor }
    }

    fn c(marked: &str) -> Option<CompletionResult> {
        run(marked, &test_symbols()).result
    }
    fn ce(marked: &str) -> Option<CompletionResult> {
        run(marked, &ProjectSymbols::default()).result
    }
    fn cs(marked: &str, sym: &ProjectSymbols) -> Option<CompletionResult> {
        run(marked, sym).result
    }

    fn labels_of(r: &Option<CompletionResult>) -> Vec<String> {
        r.as_ref().map(|r| r.items.iter().map(|i| i.label.clone()).collect()).unwrap_or_default()
    }
    fn first(r: &Option<CompletionResult>) -> Option<&str> {
        r.as_ref().and_then(|r| r.items.first()).map(|i| i.label.as_str())
    }
    fn range(r: &Option<CompletionResult>) -> Option<(usize, usize)> {
        r.as_ref().map(|r| (r.from, r.to - r.from))
    }
    fn position(label: &str, r: &Option<CompletionResult>) -> Option<usize> {
        r.as_ref().and_then(|r| r.items.iter().position(|i| i.label == label))
    }
    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // ---------------- Completion: commands ----------------

    #[test]
    fn commands() {
        let a = c("Some text \\fr\u{2038}");
        assert_eq!(a.as_ref().unwrap().prefix, "\\fr");
        assert_eq!(range(&a), Some((10, 3)));
        let items = &a.as_ref().unwrap().items;
        assert!(!items.is_empty());
        let first_other = items.iter().position(|i| !i.label.to_lowercase().starts_with("\\fr")).unwrap_or(items.len());
        assert!(items[..first_other].iter().all(|i| i.label.starts_with("\\fr")), "prefix matches first");
        assert!(items[first_other..].iter().all(|i| !i.label.to_lowercase().starts_with("\\fr")), "no prefix match after a fuzzy one");
        assert!(items.iter().any(|i| i.label == "\\frametitle") && items.iter().any(|i| i.label == "\\frac"));
        assert!(items.iter().all(|i| i.kind == Kind::Command));
        assert!(items.len() <= 200);

        // Exact match first, then case-sensitive prefix, then case-insensitive prefix.
        let exact = c("\\section\u{2038}");
        assert_eq!(first(&exact), Some("\\section"));
        if let (Some(star), Some(sub)) = (position("\\section*", &exact), position("\\subsection", &exact)) {
            assert!(star < sub, "prefix match \\section* before the fuzzy match \\subsection");
        }
        let ci = c("\\TEXTB\u{2038}");
        assert_eq!(first(&ci), Some("\\textbf"), "{:?}", &labels_of(&ci)[..3]);

        // Fuzzy (subsequence) needs two letters.
        let fuzzy = c("\\tbf\u{2038}");
        assert!(labels_of(&fuzzy).contains(&"\\textbf".to_string()), "subsequence match for \\tbf");
        assert!(c("\\zzzzqq\u{2038}").is_none(), "no match -> None");
        let one = c("\\q\u{2038}");
        assert!(one.is_none_or(|r| r.items.iter().all(|i| i.label.to_lowercase().starts_with("\\q"))), "one typed letter: no fuzzy noise");

        // Project commands first and with their snippet.
        let cu = c("\\no\u{2038}");
        assert_eq!(first(&cu), Some("\\norm"));
        assert_eq!(cu.as_ref().unwrap().items[0].insert_text, "\\norm{${1:arg}}");
        assert_eq!(first(&c("\\R\u{2038}")), Some("\\R"), "exact match of a project command");
        let vec = c("a \\vec\u{2038}").unwrap();
        assert_eq!(vec.items.iter().filter(|i| i.label == "\\vec").count(), 1, "a project command replaces the catalog entry");
        assert_eq!(vec.items[0].detail.as_deref(), Some("Defined in project"));

        // Cursor in the middle of text: replacement covers only what precedes the cursor.
        assert_eq!(range(&c("\\fr\u{2038}ac and more")), Some((0, 3)));

        // UTF-16 offsets.
        assert_eq!(range(&c("a\u{e7}\u{e3}o \u{e9} \\fr\u{2038}")), Some((7, 3)));
        assert_eq!(range(&c("\u{1F600} \\fr\u{2038}")), Some((3, 3)));
        assert_eq!(range(&c("a\r\nb \\fr\u{2038}")), Some((5, 3)));

        // Star and digits are not part of the name being typed.
        assert!(c("\\section*\u{2038}").is_none(), "after a star there is nothing to complete");
        assert!(c("x\\\\fr\u{2038}").is_none(), "\\\\ is a line break, not a command start");
        assert_eq!(range(&c("x\\\\\\fr\u{2038}")), Some((3, 3)), "\\\\\\fr is \\\\ + \\fr");
        assert!(c("Plain wor\u{2038}").is_none());
        assert!(c("\u{2038}").is_none());
        assert!(complete("abc", 10, &SymbolsRef::default()).is_none());
        assert_eq!(complete("\\fr", 3, &SymbolsRef::default()).unwrap().prefix, "\\fr");
        // Lone `\` at the very start with no cursor offset beyond the text.
        assert!(complete("", 0, &SymbolsRef::default()).is_none());
    }

    #[test]
    fn bare_backslash_and_common_commands() {
        let bare = c("\\\u{2038}").unwrap();
        assert_eq!(bare.prefix, "\\");
        assert_eq!((bare.from, bare.to), (0, 1));
        assert_eq!(bare.items.len(), 80);
        assert_eq!(bare.items[0].label, "\\R", "project commands come first");
        let labels: Vec<&str> = bare.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels.iter().collect::<HashSet<_>>().len(), labels.len(), "no duplicates");
        for common in ["\\begin", "\\section", "\\textbf", "\\label", "\\ref", "\\cite", "\\item"] {
            assert!(labels.contains(&common), "bare \\ offers {common}");
        }
        assert_eq!(ce("\\\u{2038}").unwrap().items.len(), 80);
        let math_bare = labels_of(&ce("$x = \\\u{2038}$"));
        for common in ["\\frac", "\\alpha", "\\sum", "\\int", "\\mathbb", "\\left"] {
            assert!(math_bare.contains(&common.to_string()), "math bare \\ offers {common}");
        }
        assert!(!math_bare.contains(&"\\section".to_string()), "no \\section in math suggestions");
    }

    #[test]
    fn math_aware_ranking() {
        for marked in [
            "$x = \\fr\u{2038}$", "\\[ \\fr\u{2038} \\]", "\\begin{equation}\\fr\u{2038}\\end{equation}",
            "\\begin{align*} a &= \\fr\u{2038} \\end{align*}", "$$ \\fr\u{2038} $$", "\\(\\fr\u{2038}\\)", "\\begin{gather}\\fr\u{2038}",
        ] {
            let r = ce(marked);
            assert_eq!(first(&r), Some("\\frac"), "math: {marked}");
            assert!(r.unwrap().in_math);
        }
        for marked in [
            "\\fr\u{2038}", "$x$ \\fr\u{2038}", "\\begin{itemize}\\fr\u{2038}", "$x \\text{ \\fr\u{2038}}$", "\\[ x \\] \\fr\u{2038}",
            "% $\n\\fr\u{2038}", "\\begin{equation}x\\end{equation} \\fr\u{2038}", "$x \\mbox{\\fr\u{2038}}$",
        ] {
            let r = ce(marked);
            let (f, t) = (position("\\frac", &r), position("\\frametitle", &r));
            assert!(f.is_some() && t.is_some() && t.unwrap() < f.unwrap(), "text: \\frametitle before \\frac in {marked:?}");
        }
        // Common commands win ties.
        assert_eq!(first(&ce("\\su\u{2038}")), Some("\\subsection"));
        assert_eq!(first(&ce("$\\su\u{2038}$")), Some("\\sum"));
        assert_eq!(first(&ce("\\te\u{2038}")), Some("\\textbf"));
        assert_eq!(first(&ce("$\\la\u{2038}$")), Some("\\lambda"));
        assert_eq!(first(&ce("$\\al\u{2038}$")), Some("\\alpha"));
        // Math-only commands are still available in text mode (after the text commands).
        assert!(position("\\alpha", &ce("\\alp\u{2038}")).is_some());
    }

    // ---------------- Completion: \begin{ and \end{ ----------------

    #[test]
    fn begin_and_end() {
        let a = c("\\begin{ite\u{2038}}");
        assert_eq!(a.as_ref().unwrap().prefix, "ite");
        assert_eq!(range(&a), Some((7, 4)), "covers `ite` and the brace");
        assert_eq!(first(&a), Some("itemize"));
        // CodeMirror syntax: `$0` becomes `${}`.
        assert_eq!(a.as_ref().unwrap().items[0].insert_text, "itemize}\n\t\\item ${}\n\\end{itemize}");
        assert!(a.as_ref().unwrap().items.iter().all(|i| i.kind == Kind::Environment));

        assert_eq!(range(&c("\\begin{ite\u{2038}")), Some((7, 3)), "no closing brace to swallow");
        assert_eq!(range(&c("\\begin{ite\u{2038}ms}")), Some((7, 6)), "rest of the name and the brace");
        assert_eq!(range(&c("\\begin{ite\u{2038}} text } more")), Some((7, 4)), "only the first brace");
        assert_eq!(range(&c("\\begin{ite\u{2038} \\foo}")), Some((7, 3)), "a brace after other text is not adjacent");
        let spaced = c("\\begin {ite\u{2038}}");
        assert_eq!(first(&spaced), Some("itemize"));
        assert_eq!(range(&spaced), Some((8, 4)));

        let all = c("\\begin{\u{2038}}").unwrap();
        assert_eq!(all.prefix, "");
        assert_eq!(all.items[0].label, "myenv", "project environments first");
        assert_eq!(all.items[0].insert_text, "myenv}\n\t${}\n\\end{myenv}");
        assert!(all.items.len() > 100);

        assert_eq!(first(&ce("\\begin{align\u{2038}}")), Some("align"), "exact match first");
        assert!(labels_of(&ce("\\begin{align\u{2038}}")).contains(&"align*".to_string()));
        assert!(labels_of(&ce("\\begin{tbl\u{2038}")).contains(&"table".to_string()), "fuzzy: tbl -> table");

        // Math context: inner math environments first.
        let in_math = ce("\\[ \\begin{\u{2038}} \\]");
        assert!(is_inner_math_environment(first(&in_math).unwrap()), "inside \\[ \\]: {:?}", &labels_of(&in_math)[..3]);
        let outside = ce("\\begin{\u{2038}}");
        assert!(!is_inner_math_environment(first(&outside).unwrap()), "outside math the catalog order is kept");
        assert_eq!(first(&ce("\\begin{ite\u{2038}}")), Some("itemize"));

        // Masked contexts.
        assert!(c("% \\begin{ite\u{2038}}").is_none(), "comment");
        assert!(c("\\begin{verbatim}\n\\begin{ite\u{2038}}\n\\end{verbatim}").is_none(), "verbatim body");

        // \end{ - innermost unclosed first.
        let doc = "\\begin{document}\n\\begin{itemize}\n\\item x\n\\begin{center}\n\\end{center}\n\\end{\u{2038}}";
        let end = run(doc, &ProjectSymbols::default());
        assert_eq!(labels_of(&end.result), strs(&["itemize", "document"]), "innermost first; closed environments are not offered");
        let r = end.result.as_ref().unwrap();
        assert_eq!(r.items[0].insert_text, "itemize}");
        assert_eq!(r.items[0].kind, Kind::Environment);
        assert!(r.items[0].detail.as_deref().unwrap().contains('2'), "detail names the opening line: {:?}", r.items[0].detail);
        assert_eq!((r.from, r.to - r.from), (end.cursor, 1), "swallows the auto-paired brace");
        assert_eq!(r.context, "endEnvironment");
        let open = run(&doc.replace("\\end{\u{2038}}", "\\end{\u{2038}"), &ProjectSymbols::default());
        assert_eq!(range(&open.result), Some((open.cursor, 0)), "no brace to swallow");

        assert_eq!(labels_of(&ce("\\begin{a}\\begin{b}\\begin{c}\\end{\u{2038}")), strs(&["c", "b", "a"]));
        assert_eq!(labels_of(&ce("\\begin{x}\\end{x}\\begin{y}\\end{\u{2038}")), strs(&["y"]));
        assert_eq!(labels_of(&ce("\\begin{document}\\begin{theorem}\\end{doc\u{2038}}")), strs(&["document"]), "typed prefix filters the open ones");
        assert_eq!(labels_of(&ce("% \\begin{ghost}\n\\begin{real}\n\\end{\u{2038}}")), strs(&["real"]), "commented \\begin ignored");
        assert_eq!(labels_of(&ce("\\begin{verbatim}\\begin{x}\\end{verbatim}\\begin{y}\\end{\u{2038}")), strs(&["y"]));
        assert_eq!(labels_of(&c("\\begin{myenv}\n\\end{\u{2038}")), strs(&["myenv"]));
        let none = ce("\\end{\u{2038}}").unwrap();
        assert!(none.items.len() > 50, "nothing open: any environment name");
        assert!(none.items.iter().all(|i| i.insert_text.ends_with('}') && !i.insert_text.contains("\\end")));
        assert!(labels_of(&ce("\\begin{a}\\end{a}\\end{b\u{2038}")).contains(&"bmatrix".to_string()), "no open match -> catalog fallback");
        assert_eq!(labels_of(&ce("\\begin{a*}\\end{a\u{2038}")), strs(&["a*"]), "starred environment names");
        // Environments defined by \newenvironment are not "open" inside the definition.
        assert_eq!(
            labels_of(&ce("\\newenvironment{foo}{\\begin{center}}{\\end{center}}\\begin{real}\\end{\u{2038}")),
            strs(&["real"])
        );
    }

    // ---------------- Completion: labels ----------------

    #[test]
    fn labels() {
        let all = c("\\ref{\u{2038}}");
        assert_eq!(labels_of(&all), strs(&["sec:intro", "eq:main", "eq:other", "fig:one"]));
        assert_eq!(range(&all), Some((5, 0)));
        let details: Vec<Option<&str>> = all.as_ref().unwrap().items.iter().map(|i| i.detail.as_deref()).collect();
        assert_eq!(details, vec![Some("Introduction"), Some("equation"), Some("align"), Some("main.tex:20")], "context or file:line");
        assert!(all.as_ref().unwrap().items.iter().all(|i| i.kind == Kind::Label && i.insert_text == i.label));

        let eq = c("see \\ref{eq:\u{2038}}");
        assert_eq!(labels_of(&eq), strs(&["eq:main", "eq:other"]));
        assert_eq!(eq.as_ref().unwrap().prefix, "eq:");
        assert_eq!(range(&eq), Some((9, 3)), "no brace extension for labels");
        assert_eq!(labels_of(&c("\\ref{eq\u{2038}}")), strs(&["eq:main", "eq:other"]));
        assert_eq!(labels_of(&c("\\ref{sin\u{2038}}")), strs(&["sec:intro"]), "fuzzy");
        assert_eq!(labels_of(&c("\\ref{intro\u{2038}}")), strs(&["sec:intro"]), "word-start match after ':'");
        assert_eq!(labels_of(&c("\\ref{EQ:M\u{2038}}")), strs(&["eq:main"]), "case-insensitive");
        assert!(c("\\ref{nothing\u{2038}}").is_none());
        for command in ["ref", "eqref", "cref", "Cref", "autoref", "pageref", "nameref", "vref", "Vref"] {
            assert_eq!(first(&c(&format!("\\{command}{{fig\u{2038}}}"))), Some("fig:one"), "\\{command}");
        }
        assert_eq!(first(&c("\\ref*{fig\u{2038}}")), Some("fig:one"), "starred \\ref*");
        assert_eq!(first(&c("\\ref [x] {fig\u{2038}}")), Some("fig:one"), "optional argument between");
        // Comma lists only for cleveref-style commands.
        let cref = c("\\cref{eq:main,fi\u{2038}}");
        assert_eq!(labels_of(&cref), strs(&["fig:one"]));
        assert_eq!(cref.as_ref().unwrap().prefix, "fi");
        assert_eq!(range(&cref), Some((14, 2)));
        assert_eq!(labels_of(&c("\\cref{eq:main, \u{2038}}")).len(), 4, "after a comma: all labels");
        assert_eq!(range(&c("\\cref{eq:main, \u{2038}}")), Some((15, 0)));
        // Many labels are capped at 200.
        let mut many = test_symbols();
        many.labels = (0..300).map(|i| label(&format!("lab{i}"), "/p/main.tex", i + 1, None)).collect();
        assert_eq!(cs("\\ref{\u{2038}}", &many).unwrap().items.len(), 200);
        let lab29 = labels_of(&cs("\\ref{lab29\u{2038}}", &many));
        assert!(lab29.iter().take(11).all(|l| l.starts_with("lab29")) && lab29.len() > 11, "prefix matches first, fuzzy after");
        // Duplicates collapse.
        let mut dup = test_symbols();
        dup.labels.push(label("eq:main", "/p/b.tex", 1, None));
        assert_eq!(labels_of(&cs("\\ref{eq:m\u{2038}}", &dup)), strs(&["eq:main"]));
        // Contexts that must not complete.
        assert!(c("\\label{\u{2038}}").is_none(), "defining a label");
        assert!(c("\\ref{eq:main}\u{2038}").is_none(), "after the closing brace");
        assert!(c("% \\ref{eq\u{2038}}").is_none());
        assert!(c("\\ref{\n\n\u{2038}").is_none(), "blank line ends the argument");
        assert!(c("\\foo{eq\u{2038}}").is_none(), "unknown command");
        assert!(c("\\textbf{eq\u{2038}}").is_none());
        assert_eq!(c("\\ref{a\\foo\u{2038}}").unwrap().prefix, "\\foo", "a command inside the braces is a command context");
        // Windows-style label path for the file:line detail.
        let mut win = ProjectSymbols::default();
        win.labels = vec![label("k", "C:\\proj\\main.tex", 7, None)];
        assert_eq!(cs("\\ref{\u{2038}}", &win).unwrap().items[0].detail.as_deref(), Some("main.tex:7"));
    }

    // ---------------- Completion: citations ----------------

    #[test]
    fn citations() {
        let all = c("\\cite{\u{2038}}");
        assert_eq!(labels_of(&all), strs(&["knuth1984", "lamport94", "hatcher2002", "bare"]));
        let items = &all.as_ref().unwrap().items;
        assert_eq!(items[0].detail.as_deref(), Some("Knuth, Donald (1984) \u{2014} The Art of Computer Programming"));
        assert_eq!(items.last().unwrap().detail.as_deref(), Some("misc"), "no metadata -> entry type");
        assert!(items.iter().all(|i| i.kind == Kind::Citation));
        assert_eq!(labels_of(&c("\\cite{kn\u{2038}}")), strs(&["knuth1984"]));
        let list = c("\\cite{knuth1984,la\u{2038}}");
        assert_eq!(labels_of(&list), strs(&["lamport94"]));
        assert_eq!(list.as_ref().unwrap().prefix, "la");
        assert_eq!(range(&list), Some((16, 2)), "only the current item");
        assert_eq!(range(&c("\\cite{knuth1984, la\u{2038}}")), Some((17, 2)), "space after the comma is skipped");
        assert_eq!(c("\\cite{knuth1984,\n  la\u{2038}}").unwrap().prefix, "la", "newline after the comma");
        assert_eq!(c("\\cite{a,b,\u{2038}}").unwrap().items.len(), 4);
        assert_eq!(labels_of(&c("\\citep[see][p.~3]{ha\u{2038}}")), strs(&["hatcher2002"]), "two optional arguments");
        assert_eq!(labels_of(&c("\\cite[p.~3]{ha\u{2038}}")), strs(&["hatcher2002"]));
        assert_eq!(range(&c("\\cite{kn\u{2038}uth1984}")), Some((6, 2)), "the rest of the key is not replaced");
        for command in [
            "cite", "citep", "citet", "citeauthor", "citeyear", "Cite", "parencite", "textcite", "autocite", "footcite", "nocite",
            "Citep", "supercite", "cites",
        ] {
            assert_eq!(first(&c(&format!("\\{command}{{lam\u{2038}}}"))), Some("lamport94"), "\\{command}");
        }
        assert_eq!(first(&c("\\cite*{lam\u{2038}}")), Some("lamport94"), "starred");
        // Matching on author and title when the key does not match.
        assert_eq!(labels_of(&c("\\cite{Allen\u{2038}}")), strs(&["hatcher2002"]), "author");
        assert_eq!(labels_of(&c("\\cite{Algebraic\u{2038}}")), strs(&["hatcher2002"]), "title");
        assert!(c("\\cite{zzzzz\u{2038}}").is_none());
        assert!(ce("\\cite{kn\u{2038}}").is_none(), "no bibliography");
        assert!(c("\\citestyle{kn\u{2038}}").is_none(), "\\citestyle is not a citation command");
    }

    // ---------------- Completion: packages ----------------

    #[test]
    fn packages() {
        let ams = ce("\\usepackage{amsm\u{2038}}");
        assert_eq!(first(&ams), Some("amsmath"));
        assert_eq!(ams.as_ref().unwrap().prefix, "amsm");
        assert_eq!(range(&ams), Some((12, 4)));
        assert!(ams.as_ref().unwrap().items.iter().all(|i| i.kind == Kind::Package));
        assert_eq!(first(&ce("\\usepackage[utf8]{inpu\u{2038}}")), Some("inputenc"));
        let list = ce("\\usepackage{amsmath,tik\u{2038}}");
        assert!(labels_of(&list).contains(&"tikz".to_string()));
        assert_eq!(list.as_ref().unwrap().prefix, "tik");
        assert_eq!(range(&list), Some((20, 3)));
        assert_eq!(first(&ce("\\RequirePackage{xcol\u{2038}}")), Some("xcolor"));
        assert_eq!(first(&ce("\\usepackage[a=b,c=d]{geo\u{2038}}")), Some("geometry"));
        let local = c("\\usepackage{mys\u{2038}}");
        assert_eq!(first(&local), Some("mystyle"), "a local .sty comes first");
        assert_eq!(local.unwrap().items[0].detail.as_deref(), Some("Local package"));
        assert_eq!(ce("\\usepackage{\u{2038}}").map(|r| r.items.len()), Some(200), "capped at 200");
        assert!(ce("\\documentclass{art\u{2038}}").is_none(), "\\documentclass is not completed");
        // Several local packages: Swift inserts each at index 0, so the last one listed comes first.
        let mut two = ProjectSymbols::default();
        two.project_files = strs(&["a.sty", "b.sty"]);
        assert_eq!(labels_of(&cs("\\usepackage{\u{2038}}", &two))[..2], strs(&["b", "a"])[..]);
    }

    // ---------------- Completion: files ----------------

    #[test]
    fn files() {
        let files = |m: &str| labels_of(&c(m));
        assert_eq!(files("\\input{sec\u{2038}}"), strs(&["sections/intro.tex", "sections/methods.tex"]));
        assert_eq!(files("\\input{intro\u{2038}}"), strs(&["sections/intro.tex"]), "word-start after '/'");
        assert_eq!(files("\\input{smth\u{2038}}"), strs(&["sections/methods.tex"]), "fuzzy");
        assert_eq!(files("\\include{sec\u{2038}}"), strs(&["sections/intro", "sections/methods"]), "\\include drops the extension");
        assert_eq!(files("\\subfile{\u{2038}}"), strs(&["main.tex", "sections/intro.tex", "sections/methods.tex"]));
        assert_eq!(files("\\includegraphics[width=2cm]{fi\u{2038}}"), strs(&["figs/plot.pdf", "figs/photo.png"]));
        assert_eq!(files("\\includegraphics*{\u{2038}}"), strs(&["figs/plot.pdf", "figs/photo.png"]));
        assert_eq!(files("\\includepdf[pages=-]{\u{2038}}"), strs(&["figs/plot.pdf"]));
        assert_eq!(files("\\bibliography{re\u{2038}}"), strs(&["refs", "extra/more"]), "prefix first, then the fuzzy match");
        assert_eq!(files("\\bibliography{refs,\u{2038}}"), strs(&["refs", "extra/more"]), "list: second item");
        assert_eq!(files("\\addbibresource{\u{2038}}"), strs(&["refs.bib", "extra/more.bib"]));
        let r = c("\\input{sec\u{2038}}");
        assert_eq!(range(&r), Some((7, 3)));
        assert!(r.unwrap().items.iter().all(|i| i.kind == Kind::File));
        assert_eq!(range(&c("\\bibliography{refs,ex\u{2038}}")), Some((19, 2)), "only the current item");
        assert!(c("\\input{zzz\u{2038}}").is_none());
        assert!(ce("\\input{sec\u{2038}}").is_none());
        assert_eq!(range(&c("\\input{ sec\u{2038}}")), Some((8, 3)), "leading space skipped");
        // Windows: upper-case extensions and backslash separators.
        let mut win = ProjectSymbols::default();
        win.project_files = strs(&["Chapters\\One.TEX", "img\\Fig.PNG"]);
        assert_eq!(labels_of(&cs("\\input{\u{2038}}", &win)), strs(&["Chapters/One.TEX"]));
        assert_eq!(labels_of(&cs("\\include{\u{2038}}", &win)), strs(&["Chapters/One"]));
        assert_eq!(labels_of(&cs("\\includegraphics{\u{2038}}", &win)), strs(&["img/Fig.PNG"]));
    }

    // ---------------- Completion: masked regions and robustness ----------------

    #[test]
    fn masked_regions_and_robustness() {
        assert!(c("% \\fr\u{2038}").is_none(), "comment");
        assert!(c("text % comment \\fr\u{2038}").is_none());
        assert!(c("\\begin{verbatim}\n\\fr\u{2038}\n\\end{verbatim}").is_none(), "verbatim");
        assert!(c("\\begin{lstlisting}[language=C]\n\\fr\u{2038}\n\\end{lstlisting}").is_none());
        assert!(c("\\verb|\\fr\u{2038}|").is_none(), "\\verb");
        assert!(c("\\url{http://x/\\fr\u{2038}}").is_none(), "\\url");
        assert!(c("\\iffalse\n\\fr\u{2038}\n\\fi").is_none(), "\\iffalse block");
        // Comment ends at the line end.
        assert_eq!(range(&c("% c\n\\fr\u{2038}")), Some((4, 3)));
        assert_eq!(range(&c("\\fr\u{2038} % c")), Some((0, 3)));
        // Long unterminated input does not hang.
        let long = format!("{}\\ref{{eq\u{2038}", "word \\alpha {x ".repeat(6000));
        let started = Instant::now();
        let _ = c(&long);
        assert!(started.elapsed().as_secs_f64() < 5.0, "completion in a 90 KB document stays fast: {:?}", started.elapsed());
        // Fuzz: every cursor position of a document keeps the replacement range valid.
        let sample = "\\documentclass{article}\n\\usepackage{amsmath}\n\\begin{document}\n\\section{A \\ref{eq:main}}\n$x \\frac{a}{b}$ \\cite{knuth1984,}\n\\begin{itemize}\n\\item \u{e9}\u{1F600}\n\\end{itemize}\n\\input{sections/intro}\n\\end{document}\n";
        let sym = test_symbols();
        let len = sample.encode_utf16().count();
        let mut bad = 0;
        for cursor in 0..=len {
            if let Some(r) = complete(sample, cursor, &sym.as_ref()) {
                if r.from > cursor || r.to < cursor || r.to > len || r.items.is_empty() {
                    bad += 1;
                }
            }
        }
        assert_eq!(bad, 0, "replacement ranges valid at every cursor position");
    }

    #[test]
    fn line_column_entry_points() {
        let sym = test_symbols();
        let text = "ab\r\n\\ref{fi}\n";
        // line 2, column 8 is just after "fi" (UTF-16, 1-based).
        let r = complete_at(text, 2, 8, &sym.as_ref()).unwrap();
        assert_eq!(r.prefix, "fi");
        assert_eq!((r.from, r.to), (text.find("fi").unwrap(), text.find("fi").unwrap() + 2));
        assert!(complete_at(text, 9, 1, &sym.as_ref()).is_none(), "line past the end");
        assert_eq!(complete_at("\\fr", 1, 99, &sym.as_ref()).unwrap().prefix, "\\fr", "column clamped to the line end");
        let items = completions("\\cite{kn}", 1, 9, &sym.labels, &sym.bib_entries, &sym.project_files);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "knuth1984");
        // Emoji before the cursor: columns count UTF-16 units.
        let e = complete_at("\u{1F600}\\fr", 1, 6, &sym.as_ref()).unwrap();
        assert_eq!((e.from, e.to, e.prefix.as_str()), (2, 5, "\\fr"));
    }

    #[test]
    fn context_detection() {
        assert_eq!(detect_context("\\fr", 3), Some(CompletionContext::Command { start: 0 }));
        assert_eq!(detect_context("\\begin{ab", 9), Some(CompletionContext::Environment { is_end: false, item_start: 7 }));
        assert_eq!(detect_context("\\end{", 5), Some(CompletionContext::Environment { is_end: true, item_start: 5 }));
        assert_eq!(detect_context("\\cref{a, b", 10), Some(CompletionContext::Label { item_start: 9 }));
        assert_eq!(detect_context("\\citep{a,b", 10), Some(CompletionContext::Citation { item_start: 9 }));
        assert_eq!(detect_context("\\usepackage{a", 13), Some(CompletionContext::Package { item_start: 12 }));
        assert_eq!(
            detect_context("\\include{a", 10),
            Some(CompletionContext::File { kind: FileKind::TexWithoutExtension, item_start: 9 })
        );
        assert_eq!(detect_context("plain", 5), None);
        // Walker: math vs text.
        let r = ce("$a \\text{\\fr\u{2038}}$").unwrap();
        assert!(!r.in_math, "\\text group is text");
        assert!(ce("$a \\fr\u{2038}$").unwrap().in_math);
    }

    // ---------------- Catalog ----------------

    #[test]
    fn catalog_dedup_semantics() {
        let cat = catalog();
        assert!(cat.commands.len() > 400 && cat.math_commands.len() > 500 && cat.environments.len() > 100 && cat.packages.len() > 200);
        let mut seen = HashSet::new();
        let expected: Vec<&str> = cat
            .commands
            .iter()
            .chain(cat.math_commands.iter())
            .map(|i| i.label.as_str())
            .filter(|l| seen.insert(*l))
            .collect();
        let got: Vec<&str> = all_commands().iter().map(|i| i.label.as_str()).collect();
        assert_eq!(got, expected, "text first, then math, first occurrence wins");
        let text: HashSet<&str> = cat.commands.iter().map(|i| i.label.as_str()).collect();
        let shared: Vec<&CompletionItem> = cat.math_commands.iter().filter(|i| text.contains(i.label.as_str())).collect();
        assert!(shared.len() >= 10, "labels in both tables: {}", shared.len());
        for item in &shared {
            let kept = all_commands().iter().find(|i| i.label == item.label).unwrap();
            let text_item = cat.commands.iter().find(|i| i.label == item.label).unwrap();
            assert_eq!(kept, text_item, "{} keeps the text-mode entry", item.label);
        }
        assert!(all_commands().len() < cat.commands.len() + cat.math_commands.len());
    }

    // ---------------- Snippet ----------------

    fn stops(s: &Snippet) -> Vec<String> {
        s.tab_stops.iter().map(|r| format!("{},{}", r.location, r.length)).collect()
    }
    fn sp(location: usize, length: usize) -> Span {
        Span { location, length }
    }

    #[test]
    fn snippet_placeholders_and_final_cursor() {
        let frac = Snippet::parse("\\frac{${1:num}}{${2:den}}$0");
        assert_eq!(frac.text, "\\frac{num}{den}");
        assert_eq!(stops(&frac), ["6,3", "11,3"]);
        assert_eq!(frac.final_cursor, Some(15));
        assert!(frac.mirrors.is_empty());

        let nc = Snippet::parse("\\newcommand{${1:\\name}}[${2:0}]{${3:definition}}$0");
        assert_eq!(nc.text, "\\newcommand{\\name}[0]{definition}");
        assert_eq!(stops(&nc), ["12,5", "19,1", "22,10"]);
        assert_eq!(nc.final_cursor, Some(33));

        let plain = Snippet::parse("\\alpha");
        assert_eq!(plain.text, "\\alpha");
        assert!(plain.tab_stops.is_empty());
        assert_eq!(plain.final_cursor, None);

        let empty = Snippet::parse("");
        assert_eq!(empty.text, "");
        assert_eq!(empty.final_cursor, None);

        let middle = Snippet::parse("\\begin{x}\n\t$0\n\\end{x}");
        assert_eq!(middle.text, "\\begin{x}\n\t\n\\end{x}");
        assert_eq!(middle.final_cursor, Some(11));

        assert_eq!(stops(&Snippet::parse("$2 $1")), ["1,0", "0,0"], "tab stops are sorted by index, bare $N has empty text");
        assert_eq!(stops(&Snippet::parse("${1}")), ["0,0"]);
        assert_eq!(stops(&Snippet::parse("a${12:xy}b$10")), ["4,0", "1,2"], "multi-digit indices sort numerically (10 before 12)");
        assert_eq!(stops(&Snippet::parse("\u{e9}${1:\u{e7}\u{1F600}}")), ["1,3"], "UTF-16 offsets");
        assert_eq!(Snippet::parse("\u{e9}${1:\u{e7}\u{1F600}}").text, "\u{e9}\u{e7}\u{1F600}");

        let nested = Snippet::parse("${1:a ${2:b}}");
        assert_eq!(nested.text, "a b");
        assert_eq!(stops(&nested), ["0,3", "2,1"]);
    }

    #[test]
    fn snippet_contract() {
        // (1) real newline / tab characters are kept; backslash sequences are ordinary LaTeX.
        assert_eq!(Snippet::parse("a\nb\tc").text, "a\nb\tc");
        let latex = "\\newcommand \\textbf{${1:x}} \\tau \\n \\t";
        assert_eq!(Snippet::parse(latex).text, "\\newcommand \\textbf{x} \\tau \\n \\t", "\\n and \\t are not escapes");
        assert_eq!(
            Snippet::parse("\\begin{itemize}\n\t\\item $0\n\\end{itemize}").text,
            "\\begin{itemize}\n\t\\item \n\\end{itemize}"
        );

        // (2) a `$` preceded by a backslash is never a tab stop; both characters stay.
        assert_eq!(Snippet::parse("\\$").text, "\\$");
        let dollar = Snippet::parse("\\$${1:x}");
        assert_eq!(dollar.text, "\\$x");
        assert_eq!(stops(&dollar), ["2,1"], "the second $ starts a placeholder");
        let literal = Snippet::parse("cost \\$1 and \\$0 and \\${1:x}");
        assert_eq!(literal.text, "cost \\$1 and \\$0 and \\${1:x}");
        assert!(literal.tab_stops.is_empty());
        assert_eq!(literal.final_cursor, None);
        assert_eq!(Snippet::parse("\\$5 \\$").text, "\\$5 \\$");

        // (3) defaults may contain backslashes and balanced braces.
        let frac = Snippet::parse("${1:\\frac{a}{b}}");
        assert_eq!(frac.text, "\\frac{a}{b}");
        assert_eq!(stops(&frac), ["0,11"]);
        assert_eq!(Snippet::parse("${1:{}}x").text, "{}x");
        assert_eq!(Snippet::parse("${1:\\}}x").text, "\\}x", "escaped brace inside a default");
        assert_eq!(Snippet::parse("[${1:\\{}]").text, "[\\{]", "escaped opening brace inside a default");
        assert_eq!(Snippet::parse("\\left\\{ ${1:x} \\right\\}").text, "\\left\\{ x \\right\\}");
        assert_eq!(stops(&Snippet::parse("\\left\\{ ${1:x} \\right\\}")), ["8,1"]);

        // (4) a `$` not followed by a digit or `{` is literal.
        for literal_text in ["$x$", "a $ b", "$", "$$", "US$", "$ {1}", "$a$b", "50$-$x", "${", "${a}", "${1", "${1:abc", "${1:{x}"] {
            let s = Snippet::parse(literal_text);
            assert_eq!(s.text, literal_text, "literal: {literal_text}");
            assert!(s.tab_stops.is_empty(), "no tab stops: {literal_text}");
        }
        let two = Snippet::parse("$$0");
        assert_eq!(two.text, "$");
        assert_eq!(two.final_cursor, Some(1));
    }

    #[test]
    fn snippet_mirrors() {
        let mirror = Snippet::parse("${1:x} and $1");
        assert_eq!(mirror.text, "x and x");
        assert_eq!(stops(&mirror), ["0,1"], "only the first occurrence is a tab stop");
        assert_eq!(mirror.mirrors, vec![vec![sp(6, 1)]]);

        let reversed = Snippet::parse("$1 ${1:x}");
        assert_eq!(reversed.text, "x x");
        assert_eq!(stops(&reversed), ["0,1"]);

        let triple = Snippet::parse("${1:ab} $1 $1");
        assert_eq!(triple.text, "ab ab ab");
        assert_eq!(stops(&triple), ["0,2"]);
        assert_eq!(triple.mirrors[0].len(), 2);

        let two = Snippet::parse("\\begin{${1:env}} $0 \\end{$1}");
        assert_eq!(two.text, "\\begin{env}  \\end{env}");
        assert_eq!(stops(&two), ["7,3"]);
        assert_eq!(two.final_cursor, Some(12));
    }

    #[test]
    fn snippet_to_codemirror_mapping() {
        let cm = |s: &str| Snippet::parse(s).to_codemirror();
        assert_eq!(cm("\\frac{${1:num}}{${2:den}}$0"), "\\frac{${1:num}}{${2:den}}${}");
        assert_eq!(cm("\\alpha"), "\\alpha");
        assert_eq!(cm("\\begin{x}\n\t$0\n\\end{x}"), "\\begin{x}\n\t${}\n\\end{x}");
        assert_eq!(cm("$2 $1"), "${2} ${1}");
        assert_eq!(cm("${1:x} and $1"), "${1:x} and ${1:x}", "mirrors repeat the default");
        assert_eq!(cm("$1 ${1:x}"), "${1:x} ${1:x}");
        assert_eq!(cm("\\begin{${1:env}} $0 \\end{$1}"), "\\begin{${1:env}} ${} \\end{${1:env}}");
        assert_eq!(cm("${1:a ${2:b}}"), "${1:a b}", "nested placeholders are flattened");
        assert_eq!(cm("${1:\\frac{a}{b}}"), "${1}", "braces in a default cannot be expressed");
        assert_eq!(cm("[${1:\\{}]"), "[${1}]", "backslash-brace would be unescaped by CodeMirror");
        assert_eq!(cm("${1:{}}x"), "${1}x");
        assert_eq!(cm("$x$ \\$ ${1:y}"), "$x$ \\$ ${1:y}");
        // $0 inside a placeholder default is dropped (no field inside a field).
        assert_eq!(cm("${1:a$0b}"), "${1:ab}");
        // Idempotent, and plain text is borrowed.
        for s in ["\\frac{${1:a}}{${2:b}}$0", "${1:x} $1", "a\n\t$0"] {
            let once = to_codemirror(s).into_owned();
            assert_eq!(to_codemirror(&once), once);
        }
        assert!(matches!(to_codemirror("\\alpha"), Cow::Borrowed(_)));
        assert!(matches!(to_codemirror("\\x{${1:y}}"), Cow::Borrowed(_)), "already CodeMirror syntax");
    }

    /// Minimal emulation of CodeMirror's field parser (`[#$]\{(?:(\d+)(?::([^}]*))?|((?:\\[{}]|[^}])*))\}`)
    /// returning the expanded text and the (seq, start, end) of every field.
    fn emulate_codemirror(template: &str) -> (String, Vec<(Option<usize>, usize, usize)>) {
        let mut fields = Vec::new();
        let mut out = String::new();
        let mut rest = template;
        while let Some(pos) = rest.find(['$', '#']) {
            let tail = &rest[pos..];
            if !tail[1..].starts_with('{') {
                out.push_str(&rest[..pos + 1]);
                rest = &rest[pos + 1..];
                continue;
            }
            let body = &tail[2..];
            let digits = body.bytes().take_while(|b| b.is_ascii_digit()).count();
            let (seq, name, consumed);
            if digits > 0 && (body[digits..].starts_with('}') || body[digits..].starts_with(':')) {
                seq = Some(body[..digits].parse::<usize>().unwrap());
                if body[digits..].starts_with(':') {
                    let close = body[digits + 1..].find('}');
                    match close {
                        Some(cl) => {
                            name = body[digits + 1..digits + 1 + cl].to_string();
                            consumed = digits + 1 + cl + 1;
                        }
                        None => {
                            out.push_str(&rest[..pos + 1]);
                            rest = &rest[pos + 1..];
                            continue;
                        }
                    }
                } else {
                    name = String::new();
                    consumed = digits + 1;
                }
            } else {
                match body.find('}') {
                    Some(cl) => {
                        seq = None;
                        name = body[..cl].replace("\\{", "{").replace("\\}", "}");
                        consumed = cl + 1;
                    }
                    None => {
                        out.push_str(&rest[..pos + 1]);
                        rest = &rest[pos + 1..];
                        continue;
                    }
                }
            }
            out.push_str(&rest[..pos]);
            let start = out.encode_utf16().count();
            out.push_str(&name);
            fields.push((seq, start, start + name.encode_utf16().count()));
            rest = &rest[pos + 2 + consumed..];
        }
        out.push_str(rest);
        (out, fields)
    }

    #[test]
    fn every_catalog_item_parses_and_converts_cleanly() {
        let raw = parse_raw_catalog();
        let all: Vec<&CompletionItem> =
            raw.commands.iter().chain(&raw.math_commands).chain(&raw.environments).chain(&raw.packages).collect();
        assert!(all.len() > 1000, "catalog size {}", all.len());
        let mut problems: Vec<String> = Vec::new();
        for item in &all {
            let snippet = Snippet::parse(&item.insert_text);
            let length = snippet.text.encode_utf16().count();
            if snippet.text.contains("${") {
                problems.push(format!("leftover ${{ in {}: {}", item.label, item.insert_text));
            }
            // No unescaped `$digit` may survive.
            let b = snippet.text.as_bytes();
            for k in 0..b.len().saturating_sub(1) {
                if b[k] == b'$' && b[k + 1].is_ascii_digit() && (k == 0 || b[k - 1] != b'\\') {
                    problems.push(format!("leftover $N in {}: {}", item.label, item.insert_text));
                    break;
                }
            }
            for r in &snippet.tab_stops {
                if r.location + r.length > length {
                    problems.push(format!("range out of bounds in {}", item.label));
                }
            }
            if let Some(cur) = snippet.final_cursor {
                if cur > length {
                    problems.push(format!("cursor out of bounds in {}", item.label));
                }
            }
            if item.insert_text.is_empty() {
                problems.push(format!("empty insertText for {}", item.label));
            }
            // Contract: line breaks and indentation are real U+000A / U+0009 characters, never `\n` / `\t` escapes.
            let raw_units: Vec<u16> = item.insert_text.encode_utf16().collect();
            let mut r = 0;
            while r + 2 < raw_units.len() {
                let third = raw_units[r + 2];
                let is_letter = (65..=90).contains(&third) || (97..=122).contains(&third);
                if raw_units[r] == 92
                    && (raw_units[r + 1] == 110 || raw_units[r + 1] == 116)
                    && !is_letter
                    && third != 123
                    && third != 42
                    && third != 91
                    && (r == 0 || raw_units[r - 1] != 92)
                {
                    problems.push(format!("escape-looking \\n/\\t in {}: {:?}", item.label, item.insert_text));
                    break;
                }
                r += 1;
            }
            if item.label.is_empty() || (item.label.contains(' ') && item.kind != Kind::Snippet && item.kind != Kind::File) {
                problems.push(format!("suspicious label '{}'", item.label));
            }

            // CodeMirror conversion: same expanded text, every field where Swift has it, no stray fields.
            let cm = snippet.to_codemirror();
            let (expanded, fields) = emulate_codemirror(&cm);
            // The five entries with braces in a default intentionally lose that default.
            let lossy = ["\\tikzset", "\\logo", "\\sisetup", "\\ifthenelse", "\\AtBeginSection"].contains(&item.label.as_str());
            if expanded != snippet.text && !lossy {
                problems.push(format!("CodeMirror text differs for {}: {cm:?} vs {:?}", item.label, snippet.text));
            }
            let numbered: HashSet<usize> = fields.iter().filter_map(|f| f.0).collect();
            let swift_numbered: HashSet<usize> = snippet.tab_stop_indices.iter().copied().collect();
            // Only nested stops (flattened) may be missing.
            if !numbered.is_subset(&swift_numbered) {
                problems.push(format!("unexpected CodeMirror field in {}: {cm:?}", item.label));
            }
            let unnumbered = fields.iter().filter(|f| f.0.is_none()).count();
            if unnumbered != usize::from(snippet.final_cursor.is_some() && !cm.is_empty() && cm.contains("${}")) {
                problems.push(format!("final cursor mismatch in {}: {cm:?}", item.label));
            }
            if to_codemirror(&cm) != cm {
                problems.push(format!("conversion not idempotent for {}", item.label));
            }
        }
        assert!(problems.is_empty(), "catalog problems ({} total): {:#?}", problems.len(), &problems[..problems.len().min(10)]);

        // Environments: `name}` + args + indented body + `\end{name}`.
        for env in &raw.environments {
            let text = Snippet::parse(&env.insert_text).text;
            assert!(text.starts_with(&format!("{}}}", env.label)), "{} starts with name}}", env.label);
            assert!(text.ends_with(&format!("\n\\end{{{}}}", env.label)), "{} ends with \\end{{name}}", env.label);
            assert!(text.contains("\n\t"), "{} has an indented body line", env.label);
        }
        // Commands insert their own label.
        let not_prefixed: Vec<String> = raw
            .commands
            .iter()
            .chain(&raw.math_commands)
            .filter(|i| !i.insert_text.contains(&i.label))
            .map(|i| format!("{} -> {}", i.label, i.insert_text))
            .collect();
        assert!(not_prefixed.is_empty(), "command insertText contains its label: {:?}", &not_prefixed[..not_prefixed.len().min(10)]);
        assert!(raw.commands.iter().all(|i| i.label.starts_with('\\')), "all command labels start with a backslash");
        assert!(raw.math_commands.iter().all(|i| i.label.starts_with('\\')));
        for table in [&raw.commands, &raw.math_commands, &raw.environments, &raw.packages] {
            let mut seen = HashSet::new();
            let duplicates: Vec<&str> = table.iter().map(|i| i.label.as_str()).filter(|l| !seen.insert(*l)).collect();
            assert!(duplicates.len() < 5, "duplicate labels in one table: {:?}", &duplicates[..duplicates.len().min(5)]);
        }
        // The five entries with braces in a default lose the default (documented limitation).
        assert_eq!(
            Snippet::parse("\\ifthenelse{${1:\\equal{\\x}{\\y}}}{${2:true}}{${3:false}}$0").to_codemirror(),
            "\\ifthenelse{${1}}{${2:true}}{${3:false}}${}"
        );
    }

    #[test]
    fn engine_output_is_codemirror_syntax() {
        let r = ce("\\fra\u{2038}").unwrap();
        let frac = r.items.iter().find(|i| i.label == "\\frac").unwrap();
        assert_eq!(frac.insert_text, "\\frac{${1:a}}{${2:b}}${}");
        let r = c("\\no\u{2038}").unwrap();
        assert_eq!(r.items[0].insert_text, "\\norm{${1:arg}}");
        // Project commands given in the Swift syntax are converted on the way out.
        let mut sym = ProjectSymbols::default();
        sym.custom_commands = vec![custom("\\foo", "\\foo{${1:a}}$0")];
        assert_eq!(cs("\\fo\u{2038}", &sym).unwrap().items[0].insert_text, "\\foo{${1:a}}${}");
        // IPC shape.
        let json = serde_json::to_value(&r.items[0]).unwrap();
        assert_eq!(json["kind"], "command");
        assert!(json.get("insertText").is_some());
    }
}

//! Reference / citation scanning (port of `ReferenceScanner.swift`) and the live checks built on it
//! (port of `ReferenceLinter.swift`).

use super::arguments as args;
use super::masked::MaskedSource;
use super::models::*;
use super::syntax::*;
use crate::core::log_parser::{LogDiagnostic, Severity, Source};
use crate::core::project_index::ProjectModel;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;

/// A key written in the source with the exact place of the key text: the argument of a reference
/// (`\ref{key}`), a citation (`\cite{key}`), a `\label{key}` or a `\bibitem{key}`.
///
/// `line` and `column` are 1-based and `column` / `length` / `offset` count UTF-16 units.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyOccurrence {
    /// The key as written, without surrounding whitespace.
    pub key: String,
    pub file: FilePath,
    pub line: usize,
    pub column: usize,
    /// Length of `key` in UTF-16 units.
    pub length: usize,
    /// UTF-16 offset of the first character of the key in its file.
    pub offset: usize,
    /// The control word that carries the key, without the backslash (`"cref"`, `"parencite"`, `"label"`,
    /// `"bibitem"`; `"begin"` / `"lstinputlisting"` / `"newlabel"` for labels defined through an option).
    pub command: String,
    /// The key sits between a TeX `\if…` and its `\fi` (so it may be one of several alternatives).
    pub is_conditional: bool,
}

impl KeyOccurrence {
    /// The key's range in the file's UTF-16 text.
    pub fn range(&self) -> Range<usize> {
        self.offset..self.offset + self.length
    }
}

/// Everything the reference scan of one file yields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReferenceScan {
    /// Every `\label{key}` (not deduplicated).
    pub labels: Vec<KeyOccurrence>,
    /// Labels defined without `\label`: a `label=key` option of `\begin{…}[…]` / `\lstinputlisting[…]` and a
    /// hand-written `\newlabel{key}{…}`. They count as defined but are never reported as duplicates.
    pub option_labels: Vec<KeyOccurrence>,
    /// Every key of a reference command (`\ref \eqref \cref{a,b} \crefrange{a}{b} \hyperref[key]{…}` …).
    pub references: Vec<KeyOccurrence>,
    /// Every key of a citation command (`\cite \citep \parencite{a,b}` …); `\nocite{*}` is skipped.
    pub citations: Vec<KeyOccurrence>,
    /// Every `\bibitem[label]{key}`.
    pub bib_items: Vec<KeyOccurrence>,
    /// Prefixes of label keys a macro builds at run time (`\label{sec:#1}` -> `"sec:"`; `\label{#1}` -> `""`).
    pub dynamic_label_prefixes: Vec<String>,
    /// `\externaldocument` (xr, xr-hyper, zref-xr) or one of those packages.
    pub uses_external_documents: bool,
    /// `\externalcitedocument` (xcite).
    pub uses_external_citations: bool,
}

/// Commands whose mandatory argument is a comma list of label keys.
pub fn is_reference_command(name: &str) -> bool {
    matches!(
        name,
        "ref"
            | "eqref"
            | "pageref"
            | "autoref"
            | "nameref"
            | "vref"
            | "Vref"
            | "cref"
            | "Cref"
            | "cpageref"
            | "Cpageref"
            | "labelcref"
            | "labelcpageref"
            | "subref"
            | "autopageref"
            | "fref"
            | "Fref"
            | "namecref"
            | "nameCref"
            | "lcnamecref"
            | "namecrefs"
            | "nameCrefs"
            | "lcnamecrefs"
    )
}

/// Commands with two mandatory label arguments (`\crefrange{first}{last}`).
pub fn is_reference_range_command(name: &str) -> bool {
    matches!(name, "crefrange" | "Crefrange" | "cpagerefrange" | "Cpagerefrange" | "vrefrange" | "Vrefrange")
}

/// Commands whose first mandatory argument is a comma list of citation keys.
pub fn is_citation_command(name: &str) -> bool {
    matches!(
        name,
        // LaTeX / natbib
        "cite" | "citep" | "citet" | "citealp" | "citealt" | "citeauthor" | "citeyear" | "citeyearpar" | "citenum"
            | "Citep" | "Citet" | "Citealp" | "Citealt" | "Citeauthor" | "citetalias" | "citepalias" | "Citetalias"
            | "Citepalias"
            // biblatex
            | "Cite" | "parencite" | "Parencite" | "textcite" | "Textcite" | "autocite" | "Autocite" | "footcite"
            | "footcitetext" | "smartcite" | "Smartcite" | "supercite" | "citetitle" | "Citetitle" | "citedate"
            | "citeurl" | "fullcite" | "footfullcite" | "citefield" | "nocite"
            // biblatex multicites: only the first key group is read
            | "cites" | "Cites" | "parencites" | "Parencites" | "textcites" | "Textcites" | "autocites" | "Autocites"
            | "footcites" | "smartcites" | "Smartcites" | "supercites"
    )
}

#[derive(Clone, Copy)]
enum Kind {
    Reference,
    ReferenceRange,
    Hyperref,
    Citation,
    Label,
    NewLabel,
    BibItem,
    Begin,
    Listing,
    ExternalDocument,
    ExternalCitations,
    Package,
    NewIf,
}

fn kind_of(name: &str) -> Option<Kind> {
    if is_reference_command(name) {
        return Some(Kind::Reference);
    }
    if is_reference_range_command(name) {
        return Some(Kind::ReferenceRange);
    }
    if is_citation_command(name) {
        return Some(Kind::Citation);
    }
    Some(match name {
        "hyperref" => Kind::Hyperref,
        "label" => Kind::Label,
        "newlabel" => Kind::NewLabel,
        "bibitem" => Kind::BibItem,
        "begin" => Kind::Begin,
        "lstinputlisting" => Kind::Listing,
        "externaldocument" | "zexternaldocument" => Kind::ExternalDocument,
        "externalcitedocument" => Kind::ExternalCitations,
        "newif" => Kind::NewIf,
        "usepackage" | "RequirePackage" => Kind::Package,
        _ => return None,
    })
}

/// Prefixes of names that start with `if` but take their branches as braced arguments (etoolbox, ifthen,
/// ifdraft …): they are not closed by `\fi`.
const BRACED_CONDITIONAL_PREFIXES: [&str; 27] = [
    "ifdef", "ifundef", "ifstr", "ifbool", "ifblank", "ifnumcomp", "ifnumequal", "ifnumgreater", "ifnumless", "ifnumodd",
    "ifdimcomp", "ifdimequal", "ifdimgreater", "ifdimless", "ifdimodd", "ifcsdef", "ifcsundef", "ifcsstring", "ifcsempty",
    "ifcsvoid", "iftoggle", "ifpackage", "ifclass", "ifdraft", "ifinlist", "ifrmnum", "ifpatchable",
];

/// `\ifx`, `\iftrue`, `\ifdefined`, `\ifmmode`, the `\iffoo` a `\newif` made …: closed by a `\fi`.
fn opens_conditional(name: &str) -> bool {
    if !name.starts_with("if") {
        return false;
    }
    if name == "if" || name == "ifdefined" || name == "ifcsname" || name == "iffontchar" {
        return true;
    }
    if name == "iff" || name == "ifthenelse" {
        return false;
    }
    for p in BRACED_CONDITIONAL_PREFIXES {
        if name.starts_with(p) {
            return false;
        }
    }
    true
}

const LABEL_WORD: [u16; 5] = [108, 97, 98, 101, 108];

/// Scans `text`. Positions refer to `text` as given (`\n`, `\r\n` and `\r` are all handled).
pub fn scan_text(text: &str, file: &str) -> ReferenceScan {
    scan(&MaskedSource::new(text), file)
}

struct Scanner<'a> {
    src: &'a MaskedSource,
    u: &'a [u16],
    file: FilePath,
    conditional_depth: usize,
}

impl Scanner<'_> {
    /// The key at `range` (already trimmed), or `None` when it cannot be compared statically.
    fn occurrence(&self, range: Range<usize>, command: &str) -> Option<KeyOccurrence> {
        if range.is_empty() {
            return None;
        }
        let u = self.u;
        for k in range.clone() {
            let c = u[k];
            if c == 35 || c == 92 || c == 123 || c == 125 || is_line_break(c) {
                return None;
            }
        }
        let (line, column) = self.src.position_at(range.start);
        Some(KeyOccurrence {
            key: utf16_string(u, range.clone()),
            file: self.file.clone(),
            line,
            column,
            length: range.len(),
            offset: range.start,
            command: command.to_string(),
            is_conditional: self.conditional_depth > 0,
        })
    }

    fn add_list(&self, group: Range<usize>, command: &str, keep: &mut Vec<KeyOccurrence>, skip_star: bool) {
        for item in args::split_list(self.u, group) {
            if skip_star && item.len() == 1 && self.u[item.start] == 42 {
                continue;
            }
            if let Some(found) = self.occurrence(item, command) {
                keep.push(found);
            }
        }
    }

    /// `label=key` entries of an option list.
    fn add_option_labels(&self, options: Range<usize>, command: &str, keep: &mut Vec<KeyOccurrence>) {
        let u = self.u;
        for item in args::split_list(u, options) {
            if item.len() <= 6 {
                continue;
            }
            let mut p = item.start;
            if u[p..p + LABEL_WORD.len()] != LABEL_WORD {
                continue;
            }
            p += LABEL_WORD.len();
            while p < item.end && is_tex_whitespace(u[p]) {
                p += 1;
            }
            if p >= item.end || u[p] != 61 {
                continue;
            }
            let mut value = args::trimmed(u, (p + 1)..item.end);
            if value.len() >= 2 && u[value.start] == 123 && u[value.end - 1] == 125 {
                value = args::trimmed(u, (value.start + 1)..(value.end - 1));
            }
            if let Some(found) = self.occurrence(value, command) {
                keep.push(found);
            }
        }
    }

    /// `[options]` right after `from`, if any.
    fn options_after(&self, from: usize) -> Option<Range<usize>> {
        let u = self.u;
        let p = args::skip_whitespace(u, from);
        if p < u.len() && u[p] == 91 {
            return args::bracketed(u, p).map(|(content, _)| content);
        }
        None
    }
}

pub fn scan(src: &MaskedSource, file: &str) -> ReferenceScan {
    let mut result = ReferenceScan::default();
    let u: &[u16] = &src.masked;
    let n = u.len();
    let mut prefixes: BTreeSet<String> = BTreeSet::new();
    let mut sc = Scanner { src, u, file: FilePath::from(file), conditional_depth: 0 };

    let mut i = 0;
    while i < n {
        if u[i] != 92 {
            i += 1;
            continue;
        }
        if i + 1 >= n {
            break;
        }
        let d = u[i + 1];
        if !is_command_letter(d) {
            if is_line_break(d) {
                i += 1;
            } else if is_high_surrogate(d) && i + 2 < n && is_low_surrogate(u[i + 2]) {
                i += 3;
            } else {
                i += 2;
            }
            continue;
        }
        let mut j = i + 2;
        while j < n && is_command_letter(u[j]) {
            j += 1;
        }
        let length = j - i - 1;
        if length == 2 && d == 102 && u[i + 2] == 105 {
            // \fi
            sc.conditional_depth = sc.conditional_depth.saturating_sub(1);
            i = j;
            continue;
        }
        if length == 2 && d == 105 && u[i + 2] == 102 {
            // \if
            sc.conditional_depth += 1;
            i = j;
            continue;
        }
        if !(3..=20).contains(&length) {
            i = j;
            continue;
        }
        let mut buf = [0u8; 48];
        let name = ascii_name(u, (i + 1)..j, &mut buf);
        if d == 105 && u[i + 2] == 102 && opens_conditional(&name) {
            // \ifx, \iftrue …
            sc.conditional_depth += 1;
            i = j;
            continue;
        }
        let Some(kind) = kind_of(&name) else {
            i = j;
            continue;
        };
        i = j;

        match kind {
            Kind::Reference => 'arm: {
                let Some(call) = args::parse(u, j, 1, true) else { break 'arm };
                sc.add_list(call.mandatory[0].clone(), &name, &mut result.references, false);
            }
            Kind::ReferenceRange => 'arm: {
                let Some(call) = args::parse(u, j, 2, true) else { break 'arm };
                if call.mandatory.len() != 2 {
                    break 'arm;
                }
                for group in &call.mandatory {
                    sc.add_list(group.clone(), &name, &mut result.references, false);
                }
            }
            Kind::Hyperref => 'arm: {
                // `\hyperref[label]{text}`; the older `\hyperref{url}{category}{name}{text}` has no label.
                let Some(group) = sc.options_after(j) else { break 'arm };
                if let Some(found) = sc.occurrence(args::trimmed(u, group), &name) {
                    result.references.push(found);
                }
            }
            Kind::Citation => 'arm: {
                let Some(call) = args::parse(u, j, 1, true) else { break 'arm };
                sc.add_list(call.mandatory[0].clone(), &name, &mut result.citations, true);
            }
            Kind::Label => 'arm: {
                let Some(call) = args::parse(u, j, 1, false) else { break 'arm };
                let r = args::trimmed(u, call.mandatory[0].clone());
                if r.is_empty() {
                    break 'arm;
                }
                if let Some(found) = sc.occurrence(r.clone(), &name) {
                    result.labels.push(found);
                } else if !r.clone().any(|k| is_line_break(u[k])) {
                    // `\label{sec:#1}`, `\label{fig:\thechapter}` …: the macro builds the key at run time.
                    let mut k = r.start;
                    while k < r.end && ![35u16, 92, 123, 125].contains(&u[k]) {
                        k += 1;
                    }
                    prefixes.insert(utf16_string(u, r.start..k));
                }
            }
            Kind::NewLabel => 'arm: {
                let Some(call) = args::parse(u, j, 1, false) else { break 'arm };
                if let Some(found) = sc.occurrence(args::trimmed(u, call.mandatory[0].clone()), &name) {
                    result.option_labels.push(found);
                }
            }
            Kind::BibItem => 'arm: {
                let Some(call) = args::parse(u, j, 1, false) else { break 'arm };
                if let Some(found) = sc.occurrence(args::trimmed(u, call.mandatory[0].clone()), &name) {
                    result.bib_items.push(found);
                }
            }
            Kind::Begin => 'arm: {
                let mut k = j;
                while k < n && is_hspace(u[k]) {
                    k += 1;
                }
                let Some((_, end)) = environment_name(u, k) else { break 'arm };
                i = end;
                if let Some(group) = sc.options_after(end) {
                    sc.add_option_labels(group, &name, &mut result.option_labels);
                }
            }
            Kind::Listing => {
                if let Some(group) = sc.options_after(j) {
                    sc.add_option_labels(group, &name, &mut result.option_labels);
                }
            }
            Kind::ExternalDocument => result.uses_external_documents = true,
            Kind::ExternalCitations => result.uses_external_citations = true,
            Kind::NewIf => {
                // `\newif\iffoo` declares a conditional: the `\iffoo` that follows does not open one.
                let p = args::skip_whitespace(u, j);
                if p < n && u[p] == 92 {
                    if let Some((_, end)) = args::control_sequence(u, p) {
                        i = end;
                    }
                }
            }
            Kind::Package => 'arm: {
                let Some(call) = args::parse(u, j, 1, false) else { break 'arm };
                for item in args::split_list(u, call.mandatory[0].clone()) {
                    let pkg = utf16_string(u, item);
                    if matches!(pkg.as_str(), "xr" | "xr-hyper" | "zref-xr") {
                        result.uses_external_documents = true;
                    }
                }
            }
        }
    }
    result.dynamic_label_prefixes = prefixes.into_iter().collect();
    result
}

// ---------------------------------------------------------------------------------------------------
// ReferenceLinter
// ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FindingKind {
    UndefinedReference,
    DuplicateLabel,
    MissingCitation,
}

/// One warning of the reference linter with what it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceFinding {
    pub kind: FindingKind,
    /// The label or citation key the warning is about.
    pub key: String,
    pub diagnostic: LogDiagnostic,
}

/// Labels that packages define by themselves: `\pageref{LastPage}` (lastpage), `\pageref{TotPages}` (totpages).
pub const AUTOMATIC_LABELS: [&str; 2] = ["LastPage", "TotPages"];

/// At most this many other definitions are named in a duplicate label's `detail`.
pub const MAX_LISTED_PLACES: usize = 5;

/// The warnings for `index`: undefined references, duplicate labels, missing citations (all
/// `Severity::Warning`, `Source::Structure`, with line/column/length of the key). Ordered by file (document
/// order of the index) and then by position.
pub fn diagnostics(index: &ProjectModel) -> Vec<LogDiagnostic> {
    findings(index).into_iter().map(|f| f.diagnostic).collect()
}

/// `path` relative to the root file's directory (`sections/intro.tex`), or just its file name.
fn display_path(path: &str, root: &str) -> String {
    if let Some(rest) = path.strip_prefix(root) {
        if let Some(rest) = rest.strip_prefix('/') {
            return rest.to_string();
        }
    }
    path.rsplit('/').next().unwrap_or(path).to_string()
}

pub fn findings(index: &ProjectModel) -> Vec<ReferenceFinding> {
    struct Found {
        file: usize,
        offset: usize,
        finding: ReferenceFinding,
    }
    let mut found: Vec<Found> = Vec::new();
    let mut order: HashMap<&str, usize> = HashMap::new();
    for (position, file) in index.files.iter().enumerate() {
        order.entry(file.as_ref()).or_insert(position);
    }

    let add = |found: &mut Vec<Found>, kind: FindingKind, usage: &KeyOccurrence, message: String, detail: Option<String>| {
        let mut diagnostic = LogDiagnostic::new(Severity::Warning, Source::Structure, message);
        diagnostic.detail = detail;
        diagnostic.path = Some(usage.file.to_string());
        diagnostic.line = Some(usage.line as u32);
        diagnostic.column = Some(usage.column as u32);
        diagnostic.length = Some(usage.length.max(1) as u32);
        found.push(Found {
            file: order.get(usage.file.as_ref()).copied().unwrap_or(usize::MAX),
            offset: usage.offset,
            finding: ReferenceFinding { kind, key: usage.key.clone(), diagnostic },
        });
    };

    // ----- Duplicate labels -----
    if index.label_sites.len() > 1 {
        let mut groups: HashMap<&str, Vec<&KeyOccurrence>> = HashMap::new();
        for site in &index.label_sites {
            groups.entry(site.key.as_str()).or_default().push(site);
        }
        let root_dir = crate::core::project_index::parent_dir(&index.root_file);
        for sites in groups.values() {
            if sites.len() <= 1 {
                continue;
            }
            // Alternatives of one `\if…\else…\fi` define the label on purpose: report only when at least two
            // definitions always run.
            if sites.iter().filter(|s| !s.is_conditional).count() < 2 {
                continue;
            }
            let mut ordered: Vec<&KeyOccurrence> = sites.clone();
            ordered.sort_by(|a, b| {
                let fa = order.get(a.file.as_ref()).copied().unwrap_or(usize::MAX);
                let fb = order.get(b.file.as_ref()).copied().unwrap_or(usize::MAX);
                fa.cmp(&fb).then(a.offset.cmp(&b.offset))
            });
            for site in &ordered {
                let mut places: Vec<String> = Vec::new();
                for other in &ordered {
                    if places.len() >= MAX_LISTED_PLACES {
                        break;
                    }
                    if other.file == site.file && other.offset == site.offset {
                        continue;
                    }
                    places.push(format!("{}:{}", display_path(&other.file, &root_dir), other.line));
                }
                let shown = places.join(", ");
                let hidden = ordered.len() - 1 - places.len();
                let detail = if hidden > 0 {
                    format!("Also defined at {shown} and {hidden} more")
                } else {
                    format!("Also defined at {shown}")
                };
                add(&mut found, FindingKind::DuplicateLabel, site, format!("Label \u{2018}{}\u{2019} is defined more than once", site.key), Some(detail));
            }
        }
    }

    // ----- Undefined references -----
    if !index.uses_external_documents && !index.reference_uses.is_empty() {
        let mut defined: HashSet<&str> = AUTOMATIC_LABELS.iter().copied().collect();
        for site in &index.label_sites {
            defined.insert(site.key.as_str());
        }
        for site in &index.option_labels {
            defined.insert(site.key.as_str());
        }
        let prefixes = &index.dynamic_label_prefixes;
        for usage in &index.reference_uses {
            if defined.contains(usage.key.as_str()) {
                continue;
            }
            if prefixes.iter().any(|p| usage.key.starts_with(p.as_str())) {
                continue;
            }
            add(&mut found, FindingKind::UndefinedReference, usage, format!("Reference \u{2018}{}\u{2019} is not defined", usage.key), None);
        }
    }

    // ----- Missing citations -----
    let has_bibliography = !index.bib_files.is_empty() || !index.bib_items.is_empty();
    if has_bibliography && !index.has_unknown_bibliography_source && !index.uses_external_citations && !index.citation_uses.is_empty() {
        let mut entry_keys: HashSet<String> = HashSet::new();
        for entry in &index.bib_entries {
            entry_keys.insert(entry.key.to_lowercase());
        }
        let mut item_keys: HashSet<&str> = HashSet::new();
        for item in &index.bib_items {
            item_keys.insert(item.key.as_str());
        }
        for usage in &index.citation_uses {
            if !item_keys.contains(usage.key.as_str()) && !entry_keys.contains(&usage.key.to_lowercase()) {
                add(
                    &mut found,
                    FindingKind::MissingCitation,
                    usage,
                    format!("Citation \u{2018}{}\u{2019} is not in the bibliography", usage.key),
                    None,
                );
            }
        }
    }

    found.sort_by(|a, b| a.file.cmp(&b.file).then(a.offset.cmp(&b.offset)));
    found.into_iter().map(|f| f.finding).collect()
}

/// Recognizes the warnings LaTeX itself prints about the same problems — ``Reference `k' on page 3
/// undefined``, ``Citation `k' on page 3 undefined`` and ``Label `k' multiply defined`` (trailing period
/// optional, any quote style) — so a host can hide them while it shows the live warning for the same key.
pub fn tex_warning(message: &str) -> Option<(FindingKind, String)> {
    let mut text: &str = message.trim_matches(|c: char| c == ' ' || c == '\t');
    if let Some(t) = text.strip_suffix('.') {
        text = t;
    }
    let kind;
    let head;
    if let Some(t) = text.strip_suffix(" multiply defined") {
        kind = FindingKind::DuplicateLabel;
        head = "Label ";
        text = t;
    } else if text.ends_with(" undefined") && text.rfind(" on page ").is_some() {
        let pos = text.rfind(" on page ")?;
        text = &text[..pos];
        if text.starts_with("Reference ") {
            kind = FindingKind::UndefinedReference;
            head = "Reference ";
        } else if text.starts_with("Citation ") {
            kind = FindingKind::MissingCitation;
            head = "Citation ";
        } else {
            return None;
        }
    } else {
        return None;
    }
    let quoted = text.strip_prefix(head)?;
    let chars: Vec<char> = quoted.chars().collect();
    if chars.len() < 3 {
        return None;
    }
    let is_quote = |c: char| matches!(c, '`' | '\'' | '\u{2018}' | '\u{2019}' | '"');
    if !is_quote(chars[0]) || !is_quote(chars[chars.len() - 1]) {
        return None;
    }
    Some((kind, chars[1..chars.len() - 1].iter().collect()))
}

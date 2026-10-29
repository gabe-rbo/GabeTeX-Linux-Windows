//! Lightweight structural scanning of a single LaTeX source (port of `LaTeXScanner.swift`; no file I/O).
//! All functions ignore commented-out text and the bodies of verbatim-like environments.

use super::arguments as args;
use super::definitions as defs;
use super::masked::MaskedSource;
use super::models::*;
use super::syntax::*;
use crate::commands::CompletionItem;
use std::collections::HashSet;
use std::ops::Range;

/// Parts of a scan the caller wants (bit set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanParts(pub u8);

impl ScanParts {
    pub const ROOT_INFO: ScanParts = ScanParts(1);
    pub const INCLUDES: ScanParts = ScanParts(2);
    pub const OUTLINE: ScanParts = ScanParts(4);
    pub const LABELS: ScanParts = ScanParts(8);
    pub const DEFINITIONS: ScanParts = ScanParts(16);
    pub const ALL: ScanParts = ScanParts(31);

    pub const fn contains(self, other: ScanParts) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn union(self, other: ScanParts) -> ScanParts {
        ScanParts(self.0 | other.0)
    }
}

/// Everything one pass over a file yields.
#[derive(Debug, Clone, Default)]
pub struct FileScan {
    pub is_root: bool,
    /// Name and `[options]` of `\documentclass`.
    pub document_class: Option<String>,
    pub document_class_options: Option<String>,
    pub includes: Vec<IncludeReference>,
    /// Headings, frames and labels in source order.
    pub outline: Vec<OutlineEntry>,
    pub labels: Vec<LabelDefinition>,
    pub custom_commands: Vec<CompletionItem>,
    pub custom_environments: Vec<String>,
}

// ---------------------------------------------------------------------------------------------------
// Public single-purpose entry points
// ---------------------------------------------------------------------------------------------------

fn is_newline_char(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// The first (at most) 20 lines of `text`. `\r\n` counts as a single terminator.
fn first_lines(text: &str, max: usize) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut iter = text.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        if is_newline_char(c) {
            lines.push(&text[start..i]);
            let mut next = i + c.len_utf8();
            if c == '\r' && next < bytes.len() && bytes[next] == b'\n' {
                iter.next();
                next += 1;
            }
            start = next;
            if lines.len() >= max {
                return lines;
            }
        }
    }
    if lines.len() < max {
        lines.push(&text[start..]);
    }
    lines
}

fn trim_blank(s: &str) -> &str {
    s.trim_start_matches([' ', '\t'])
}

/// Magic comments in the leading comment block / first 20 lines.
///
/// Recognized (keys are case-insensitive, `!TEX`, `!TeX` and `!BIB` accepted): `% !TEX root = path`,
/// `% !TEX program = x` (also `TS-program`), `% !TEX spellcheck = x`, `% !BIB program = x` and
/// `% !TEX bib = x`. The first occurrence of a key wins.
pub fn magic_comments(text: &str) -> MagicComments {
    let mut result = MagicComments::default();
    for raw in first_lines(text, 20) {
        let line = trim_blank(raw);
        let Some(line) = line.strip_prefix('%') else { continue };
        let line = trim_blank(line);
        let Some(line) = line.strip_prefix('!') else { continue };
        let domain_len = line.char_indices().find(|(_, c)| !c.is_alphabetic()).map(|(i, _)| i).unwrap_or(line.len());
        let domain = line[..domain_len].to_lowercase();
        if domain != "tex" && domain != "bib" {
            continue;
        }
        let rest = &line[domain_len..];
        let Some(eq) = rest.find('=') else { continue };
        let key = rest[..eq].trim().to_lowercase();
        let value = rest[eq + 1..].trim();
        if value.is_empty() {
            continue;
        }
        if domain == "bib" {
            if (key == "program" || key == "ts-program") && result.bib_program.is_none() {
                result.bib_program = Some(value.to_string());
            }
            continue;
        }
        match key.as_str() {
            "root" | "ts-root" => {
                if result.root.is_none() {
                    result.root = Some(value.to_string());
                }
            }
            "program" | "ts-program" => {
                if result.program.is_none() {
                    result.program = Some(value.to_string());
                }
            }
            "spellcheck" | "ts-spellcheck" => {
                if result.spellcheck.is_none() {
                    result.spellcheck = Some(value.to_string());
                }
            }
            "bib" | "bibtex" | "ts-bib"
                if result.bib_program.is_none() => {
                    result.bib_program = Some(value.to_string());
                }
            _ => {}
        }
    }
    result
}

/// True when the text is a root document: an uncommented `\documentclass` (or `\begin{document}`).
pub fn is_root_document(text: &str) -> bool {
    scan(&MaskedSource::new(text), "/", ScanParts::ROOT_INFO).is_root
}

/// Every file reference in source order.
pub fn includes(text: &str) -> Vec<IncludeReference> {
    scan(&MaskedSource::new(text), "/", ScanParts::INCLUDES).includes
}

/// Sectioning commands (incl. starred), beamer frames and labels, in source order.
pub fn outline_entries(text: &str, file: &str) -> Vec<OutlineEntry> {
    scan(&MaskedSource::new(text), file, ScanParts::OUTLINE).outline
}

/// `\label{...}` definitions with a short context: the innermost enclosing environment (layout wrappers such
/// as `document`, `center` or `minipage` are skipped) or, outside environments, the title of the nearest
/// preceding heading.
pub fn labels(text: &str, file: &str) -> Vec<LabelDefinition> {
    scan(&MaskedSource::new(text), file, ScanParts::LABELS).labels
}

/// User-defined commands as completion items (insertText has one placeholder per argument).
pub fn custom_commands(text: &str) -> Vec<CompletionItem> {
    scan(&MaskedSource::new(text), "/", ScanParts::DEFINITIONS).custom_commands
}

/// User-defined environment names (`\newenvironment`, `\newtheorem`, `\NewDocumentEnvironment` ...).
pub fn custom_environments(text: &str) -> Vec<String> {
    scan(&MaskedSource::new(text), "/", ScanParts::DEFINITIONS).custom_environments
}

// ---------------------------------------------------------------------------------------------------
// Internal single-pass scan
// ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Action {
    Section(OutlineKind),
    Label,
    FrameTitle,
    Begin,
    End,
    DocumentClass,
    InputLike(IncludeKind),
    ImportLike(IncludeKind),
    Bibliography,
    AddBibResource,
    IncludeGraphics,
    IncludePdf,
    Listing,
    InputMinted,
    UsePackage,
    NewCommand,
    PlainDef,
    MathOperator,
    DocumentCommand,
    PairedDelimiter,
    PairedDelimiterX,
    Environment,
    TheoremList,
}

fn action(name: &str) -> Option<Action> {
    if let Some(kind) = sectioning_kind(name) {
        return Some(Action::Section(kind));
    }
    Some(match name {
        "label" => Action::Label,
        "frametitle" => Action::FrameTitle,
        "begin" => Action::Begin,
        "end" => Action::End,
        "documentclass" => Action::DocumentClass,
        "input" => Action::InputLike(IncludeKind::Input),
        "include" => Action::InputLike(IncludeKind::Include),
        "subfile" => Action::InputLike(IncludeKind::Subfile),
        "import" => Action::ImportLike(IncludeKind::ImportFile),
        "subimport" => Action::ImportLike(IncludeKind::Subimport),
        "bibliography" => Action::Bibliography,
        "addbibresource" => Action::AddBibResource,
        "includegraphics" => Action::IncludeGraphics,
        "includepdf" => Action::IncludePdf,
        "lstinputlisting" => Action::Listing,
        "inputminted" => Action::InputMinted,
        "usepackage" | "RequirePackage" => Action::UsePackage,
        "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" | "newrobustcmd" | "renewrobustcmd"
        | "providerobustcmd" => Action::NewCommand,
        "def" | "gdef" | "edef" | "xdef" => Action::PlainDef,
        "DeclareMathOperator" => Action::MathOperator,
        "NewDocumentCommand" | "RenewDocumentCommand" | "ProvideDocumentCommand" | "DeclareDocumentCommand"
        | "NewExpandableDocumentCommand" | "RenewExpandableDocumentCommand" | "ProvideExpandableDocumentCommand"
        | "DeclareExpandableDocumentCommand" => Action::DocumentCommand,
        "DeclarePairedDelimiter" => Action::PairedDelimiter,
        "DeclarePairedDelimiterX" | "DeclarePairedDelimiterXPP" => Action::PairedDelimiterX,
        "newenvironment" | "renewenvironment" | "NewDocumentEnvironment" | "RenewDocumentEnvironment"
        | "ProvideDocumentEnvironment" | "DeclareDocumentEnvironment" | "newtheorem" | "newtcolorbox" | "renewtcolorbox"
        | "newtcbtheorem" | "newmdtheoremenv" | "newmdenv" | "NewEnviron" | "NewTColorBox" | "DeclareTColorBox" => {
            Action::Environment
        }
        "declaretheorem" => Action::TheoremList,
        _ => return None,
    })
}

/// Environments that wrap content without being a useful label context.
fn is_layout_environment(name: &str) -> bool {
    matches!(
        name,
        "document"
            | "center"
            | "flushleft"
            | "flushright"
            | "minipage"
            | "adjustbox"
            | "spacing"
            | "singlespace"
            | "onehalfspace"
            | "doublespace"
            | "frame"
            | "multicols"
            | "multicols*"
            | "group"
            | "small"
            | "footnotesize"
            | "scriptsize"
            | "tiny"
            | "large"
            | "Large"
            | "LARGE"
            | "huge"
            | "Huge"
            | "tcolorbox"
            | "framed"
            | "mdframed"
    )
}

struct Ctx<'a> {
    src: &'a MaskedSource,
    u: &'a [u16],
    file: FilePath,
    result: FileScan,
}

impl Ctx<'_> {
    fn add_reference(&mut self, kind: IncludeKind, range: Range<usize>, directory: Option<String>) {
        let u = self.u;
        let r = args::trimmed(u, range);
        if r.is_empty() {
            return;
        }
        let argument = utf16_string(u, r.clone());
        if argument.contains('#') || argument.contains('\\') {
            return;
        }
        self.result.includes.push(IncludeReference { kind, argument, directory, line: self.src.line_at(r.start), range: r });
    }

    fn add_list(&mut self, kind: IncludeKind, range: Range<usize>) {
        for item in args::split_list(self.u, range) {
            self.add_reference(kind, item, None);
        }
    }

    fn skip_overlay(&self, from: usize) -> usize {
        let u = self.u;
        let n = u.len();
        let mut p = from;
        while p < n && is_hspace(u[p]) {
            p += 1;
        }
        if p < n && u[p] == 60 {
            let mut q = p + 1;
            while q < n && u[q] != 62 && !is_line_break(u[q]) {
                q += 1;
            }
            if q < n && u[q] == 62 {
                p = q + 1;
            }
        }
        p
    }
}

/// One pass over an already masked source.
pub fn scan(src: &MaskedSource, file: &str, parts: ScanParts) -> FileScan {
    let u: &[u16] = &src.masked;
    let n = u.len();
    let mut cx = Ctx { src, u, file: FilePath::from(file), result: FileScan::default() };
    let mut env_stack: Vec<String> = Vec::new();
    let mut open_frames: Vec<isize> = Vec::new(); // indices into outline of open beamer frames (-1: untracked)
    let mut dropped_frames: HashSet<usize> = HashSet::new();
    let mut last_heading: Option<String> = None;
    let mut seen_commands: HashSet<String> = HashSet::new();
    let mut seen_environments: HashSet<String> = HashSet::new();
    let wants_outline = parts.contains(ScanParts::OUTLINE);
    let wants_labels = parts.contains(ScanParts::LABELS);
    let wants_headings = wants_outline || wants_labels;
    let wants_includes = parts.contains(ScanParts::INCLUDES);
    let wants_defs = parts.contains(ScanParts::DEFINITIONS);

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
        let mut buf = [0u8; 48];
        let act = {
            let name = ascii_name(u, (i + 1)..j, &mut buf);
            action(&name)
        };
        let Some(act) = act else {
            i = j;
            continue;
        };
        let command_start = i;
        i = j;
        match act {
            // ----- Sectioning and labels -----
            Action::Section(kind) => 'arm: {
                if !wants_headings {
                    break 'arm;
                }
                let Some(call) = args::parse(u, j, 1, true) else { break 'arm };
                let title_range = call.mandatory[0].clone();
                let title = clean_title(src, title_range);
                last_heading = Some(title.clone());
                if wants_outline {
                    cx.result.outline.push(OutlineEntry {
                        kind,
                        title,
                        is_starred: call.is_starred,
                        file: cx.file.clone(),
                        line: src.line_at(command_start),
                        offset: command_start,
                    });
                }
            }

            Action::FrameTitle => 'arm: {
                if !wants_headings {
                    break 'arm;
                }
                let mut p = cx.skip_overlay(j);
                if p < n && u[p] == 91 {
                    if let Some((_, end)) = args::bracketed(u, p) {
                        p = args::skip_whitespace(u, end);
                    }
                }
                if p >= n || u[p] != 123 {
                    break 'arm;
                }
                let Some((content, _)) = args::braced(u, p) else { break 'arm };
                let title = clean_title(src, content);
                last_heading = Some(title.clone());
                if let Some(&frame_index) = open_frames.last() {
                    if frame_index >= 0 && cx.result.outline[frame_index as usize].title.is_empty() {
                        cx.result.outline[frame_index as usize].title = title;
                    }
                } else if wants_outline {
                    cx.result.outline.push(OutlineEntry {
                        kind: OutlineKind::Frame,
                        title,
                        is_starred: false,
                        file: cx.file.clone(),
                        line: src.line_at(command_start),
                        offset: command_start,
                    });
                }
            }

            Action::Label => 'arm: {
                if !wants_headings {
                    break 'arm;
                }
                let Some(call) = args::parse(u, j, 1, false) else { break 'arm };
                let r = args::trimmed(u, call.mandatory[0].clone());
                if r.is_empty() {
                    break 'arm;
                }
                let key = utf16_string(u, r);
                if key.contains('#') || key.contains('\\') || key.contains('{') {
                    break 'arm;
                }
                let line = src.line_at(command_start);
                let context: Option<String> = env_stack
                    .iter()
                    .rev()
                    .find(|e| !is_layout_environment(e))
                    .cloned()
                    .or_else(|| last_heading.clone());
                if wants_labels {
                    cx.result.labels.push(LabelDefinition {
                        key: key.clone(),
                        file: cx.file.clone(),
                        line,
                        context: context.filter(|c| !c.is_empty()),
                    });
                }
                if wants_outline {
                    cx.result.outline.push(OutlineEntry {
                        kind: OutlineKind::Label,
                        title: key,
                        is_starred: false,
                        file: cx.file.clone(),
                        line,
                        offset: command_start,
                    });
                }
            }

            // ----- Environments -----
            Action::Begin => 'arm: {
                let mut k = j;
                while k < n && is_hspace(u[k]) {
                    k += 1;
                }
                let Some((name_range, end)) = environment_name(u, k) else { break 'arm };
                let env_name = utf16_string(u, name_range);
                if env_name == "document" {
                    cx.result.is_root = true;
                }
                let is_frame = env_name == "frame";
                env_stack.push(env_name);
                i = end;
                if is_frame {
                    if wants_outline {
                        let mut p = cx.skip_overlay(end);
                        if p < n && u[p] == 91 {
                            if let Some((_, e)) = args::bracketed(u, p) {
                                p = e;
                            }
                        }
                        while p < n && is_hspace(u[p]) {
                            p += 1;
                        }
                        let mut title = String::new();
                        if p < n && u[p] == 123 {
                            if let Some((content, _)) = args::braced(u, p) {
                                title = clean_title(src, content);
                            }
                        }
                        if !title.is_empty() {
                            last_heading = Some(title.clone());
                        }
                        cx.result.outline.push(OutlineEntry {
                            kind: OutlineKind::Frame,
                            title,
                            is_starred: false,
                            file: cx.file.clone(),
                            line: src.line_at(command_start),
                            offset: command_start,
                        });
                        open_frames.push(cx.result.outline.len() as isize - 1);
                    } else {
                        open_frames.push(-1);
                    }
                }
            }

            Action::End => 'arm: {
                let mut k = j;
                while k < n && is_hspace(u[k]) {
                    k += 1;
                }
                let Some((name_range, end)) = environment_name(u, k) else { break 'arm };
                let env_name = utf16_string(u, name_range);
                if let Some(position) = env_stack.iter().rposition(|e| *e == env_name) {
                    env_stack.truncate(position);
                }
                i = end;
                if env_name == "frame" {
                    if let Some(frame_index) = open_frames.pop() {
                        if frame_index >= 0 && cx.result.outline[frame_index as usize].title.is_empty() {
                            dropped_frames.insert(frame_index as usize);
                        }
                    }
                }
            }

            Action::DocumentClass => 'arm: {
                cx.result.is_root = true;
                let Some(call) = args::parse(u, j, 1, false) else { break 'arm };
                let cls = call.mandatory[0].clone();
                cx.result.document_class = Some(utf16_string(u, args::trimmed(u, cls.clone())));
                if let Some(options) = call.optionals.first() {
                    cx.result.document_class_options = Some(utf16_string(u, args::trimmed(u, options.clone())));
                }
                if wants_includes {
                    cx.add_reference(IncludeKind::Documentclass, cls, None);
                }
            }

            // ----- Includes -----
            Action::InputLike(kind) => 'arm: {
                if !wants_includes {
                    break 'arm;
                }
                let mut p = args::skip_whitespace(u, j);
                if p < n && u[p] == 123 {
                    if let Some((content, _)) = args::braced(u, p) {
                        cx.add_reference(kind, content, None);
                    }
                } else if kind == IncludeKind::Input && p > j && p < n && u[p] != 92 && u[p] != 37 && u[p] != 125 {
                    // TeX primitive form: \input file.tex
                    let start = p;
                    while p < n && !is_tex_whitespace(u[p]) && u[p] != 92 && u[p] != 123 && u[p] != 125 && u[p] != 37 {
                        p += 1;
                    }
                    cx.add_reference(IncludeKind::Input, start..p, None);
                }
            }

            Action::ImportLike(kind) => 'arm: {
                if !wants_includes {
                    break 'arm;
                }
                let Some(call) = args::parse(u, j, 2, true) else { break 'arm };
                if call.mandatory.len() != 2 {
                    break 'arm;
                }
                let directory = utf16_string(u, args::trimmed(u, call.mandatory[0].clone()));
                cx.add_reference(kind, call.mandatory[1].clone(), Some(directory));
            }

            Action::Bibliography => 'arm: {
                if !wants_includes {
                    break 'arm;
                }
                let Some(call) = args::parse(u, j, 1, false) else { break 'arm };
                cx.add_list(IncludeKind::Bibliography, call.mandatory[0].clone());
            }

            Action::UsePackage => 'arm: {
                if !wants_includes {
                    break 'arm;
                }
                let Some(call) = args::parse(u, j, 1, false) else { break 'arm };
                cx.add_list(IncludeKind::Usepackage, call.mandatory[0].clone());
            }

            Action::AddBibResource | Action::IncludeGraphics | Action::IncludePdf | Action::Listing => 'arm: {
                if !wants_includes {
                    break 'arm;
                }
                let Some(call) = args::parse(u, j, 1, true) else { break 'arm };
                let kind = match act {
                    Action::AddBibResource => IncludeKind::Addbibresource,
                    Action::IncludeGraphics => IncludeKind::Includegraphics,
                    Action::IncludePdf => IncludeKind::Includepdf,
                    _ => IncludeKind::Lstinputlisting,
                };
                cx.add_reference(kind, call.mandatory[0].clone(), None);
            }

            Action::InputMinted => 'arm: {
                if !wants_includes {
                    break 'arm;
                }
                let Some(call) = args::parse(u, j, 2, true) else { break 'arm };
                if call.mandatory.len() != 2 {
                    break 'arm;
                }
                cx.add_reference(IncludeKind::Inputminted, call.mandatory[1].clone(), None);
            }

            // ----- Definitions -----
            Action::NewCommand
            | Action::PlainDef
            | Action::MathOperator
            | Action::DocumentCommand
            | Action::PairedDelimiter
            | Action::PairedDelimiterX => 'arm: {
                if !wants_defs {
                    break 'arm;
                }
                let item = match act {
                    Action::NewCommand => defs::new_command(u, j),
                    Action::PlainDef => defs::plain_def(u, j),
                    Action::MathOperator => defs::math_operator(u, j),
                    Action::DocumentCommand => defs::document_command(u, j),
                    Action::PairedDelimiter => defs::paired_delimiter(u, j, false),
                    _ => defs::paired_delimiter(u, j, true),
                };
                if let Some(item) = item {
                    if seen_commands.insert(item.label.clone()) {
                        cx.result.custom_commands.push(item);
                    }
                }
            }

            Action::Environment | Action::TheoremList => 'arm: {
                if !wants_defs {
                    break 'arm;
                }
                let is_list = matches!(act, Action::TheoremList);
                for env_name in defs::environment_names(u, j, is_list) {
                    if seen_environments.insert(env_name.clone()) {
                        cx.result.custom_environments.push(env_name);
                    }
                }
            }
        }
    }

    if !dropped_frames.is_empty() {
        let kept: Vec<OutlineEntry> = std::mem::take(&mut cx.result.outline)
            .into_iter()
            .enumerate()
            .filter(|(idx, _)| !dropped_frames.contains(idx))
            .map(|(_, e)| e)
            .collect();
        cx.result.outline = kept;
    }
    cx.result
}

/// Heading title: comments removed, `\label{...}` removed, whitespace collapsed.
pub fn clean_title(src: &MaskedSource, range: Range<usize>) -> String {
    let raw = src.text_without_comments(range);
    let mut out: Vec<u16> = Vec::with_capacity(raw.len());
    const LABEL: [u16; 6] = [92, 108, 97, 98, 101, 108];
    let mut i = 0;
    let mut pending_space = false;
    while i < raw.len() {
        let c = raw[i];
        if c == 92
            && i + LABEL.len() <= raw.len()
            && raw[i..i + LABEL.len()] == LABEL
            && (i + LABEL.len() == raw.len() || !is_command_letter(raw[i + LABEL.len()]))
        {
            let mut p = i + LABEL.len();
            while p < raw.len() && is_tex_whitespace(raw[p]) {
                p += 1;
            }
            if p < raw.len() && raw[p] == 123 {
                if let Some((_, end)) = args::braced(&raw, p) {
                    i = end;
                    continue;
                }
            }
        }
        if is_tex_whitespace(c) {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(32);
                pending_space = false;
            }
            out.push(c);
        }
        i += 1;
    }
    String::from_utf16_lossy(&out)
}

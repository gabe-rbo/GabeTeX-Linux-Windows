//! UTF-16 helpers and static LaTeX knowledge shared by the tokenizer, scanner, linters and statistics
//! (port of `Syntax/LaTeXSyntax.swift`). All text handling is done on `&[u16]` so offsets, columns and
//! lengths are UTF-16 units, the unit CodeMirror (and the Swift app) uses.

use std::borrow::Cow;
use std::ops::Range;

#[inline(always)]
pub fn is_ascii_letter(c: u16) -> bool {
    c.wrapping_sub(65) < 26 || c.wrapping_sub(97) < 26
}
#[inline(always)]
pub fn is_ascii_digit(c: u16) -> bool {
    c.wrapping_sub(48) < 10
}
/// Letters allowed in a control word (`@` is a letter inside packages and is accepted everywhere).
#[inline(always)]
pub fn is_command_letter(c: u16) -> bool {
    is_ascii_letter(c) || c == 64
}
#[inline(always)]
pub fn is_hspace(c: u16) -> bool {
    c == 32 || c == 9
}
#[inline(always)]
pub fn is_line_break(c: u16) -> bool {
    c == 10 || c == 13
}
#[inline(always)]
pub fn is_tex_whitespace(c: u16) -> bool {
    c == 32 || c == 9 || c == 10 || c == 13
}
#[inline(always)]
pub fn is_high_surrogate(c: u16) -> bool {
    (c & 0xFC00) == 0xD800
}
#[inline(always)]
pub fn is_low_surrogate(c: u16) -> bool {
    (c & 0xFC00) == 0xDC00
}
/// ASCII-lowercased copy of a code unit (non-ASCII untouched).
#[inline(always)]
pub fn ascii_lower(c: u16) -> u16 {
    if c.wrapping_sub(65) < 26 {
        c + 32
    } else {
        c
    }
}

#[inline]
pub fn utf16_string(u: &[u16], r: Range<usize>) -> String {
    String::from_utf16_lossy(&u[r])
}

/// The slice `u[r]` as a `&str` when it is pure ASCII and short enough for `buf`, otherwise an owned
/// string. Control words are ASCII (`[A-Za-z@]`), so the hot scanning loops never allocate for them.
#[inline]
pub fn ascii_name<'a>(u: &[u16], r: Range<usize>, buf: &'a mut [u8; 48]) -> Cow<'a, str> {
    let len = r.end - r.start;
    if len <= buf.len() {
        let mut ascii = true;
        for (k, &c) in u[r.clone()].iter().enumerate() {
            if c >= 128 {
                ascii = false;
                break;
            }
            buf[k] = c as u8;
        }
        if ascii {
            // SAFETY-free: all bytes are < 128, hence valid UTF-8.
            return Cow::Borrowed(std::str::from_utf8(&buf[..len]).unwrap_or(""));
        }
    }
    Cow::Owned(utf16_string(u, r))
}

pub fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

// ---------------------------------------------------------------------------------------------------
// Static knowledge about LaTeX
// ---------------------------------------------------------------------------------------------------

/// What the tokenizer / scanner need to know about a control word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Special {
    #[default]
    None,
    Begin,
    End,
    Verb,
    Url,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CommandInfo {
    pub special: Special,
    pub is_structure: bool,
    pub allows_star: bool,
    pub takes_reference: bool,
}

/// Environments whose body is copied verbatim (no commands, no comments).
pub fn is_verbatim_environment(name: &str) -> bool {
    matches!(
        name,
        "verbatim"
            | "verbatim*"
            | "Verbatim"
            | "Verbatim*"
            | "BVerbatim"
            | "BVerbatim*"
            | "LVerbatim"
            | "LVerbatim*"
            | "lstlisting"
            | "minted"
            | "comment"
            | "filecontents"
            | "filecontents*"
            | "spverbatim"
            | "verbatimtab"
    )
}

/// Verbatim-like environments that accept an `[options]` argument right after `\begin{name}`.
pub fn is_verbatim_with_options(name: &str) -> bool {
    matches!(
        name,
        "Verbatim" | "Verbatim*" | "BVerbatim" | "BVerbatim*" | "LVerbatim" | "LVerbatim*" | "lstlisting" | "minted"
    )
}

/// Environments that switch the document into math mode (starred variants too).
pub fn is_math_environment(name: &str) -> bool {
    let base = name.strip_suffix('*').unwrap_or(name);
    matches!(
        base,
        "equation" | "align" | "alignat" | "flalign" | "gather" | "multline" | "eqnarray" | "math" | "displaymath" | "dmath"
    )
}

/// Environments that only make sense inside math (used to rank `\begin{` suggestions).
pub fn is_inner_math_environment(name: &str) -> bool {
    matches!(
        name,
        "aligned"
            | "alignedat"
            | "gathered"
            | "split"
            | "cases"
            | "dcases"
            | "rcases"
            | "matrix"
            | "pmatrix"
            | "bmatrix"
            | "Bmatrix"
            | "vmatrix"
            | "Vmatrix"
            | "smallmatrix"
            | "array"
            | "subarray"
    )
}

/// Commands whose braced argument is typeset in text mode even inside math.
pub fn is_text_mode_command(name: &str) -> bool {
    matches!(
        name,
        "text"
            | "mbox"
            | "hbox"
            | "textrm"
            | "textit"
            | "textbf"
            | "textsf"
            | "texttt"
            | "textnormal"
            | "textup"
            | "textsl"
            | "textsc"
            | "intertext"
            | "shortintertext"
            | "emph"
            | "fbox"
            | "parbox"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OutlineKind {
    Part,
    Chapter,
    Section,
    Subsection,
    Subsubsection,
    Paragraph,
    Subparagraph,
    /// beamer `\begin{frame}{Title}` / `\frametitle`
    Frame,
    /// `\label{key}`
    Label,
}

impl OutlineKind {
    /// Nesting depth used to build the tree. Labels are leaves (attached to the nearest heading).
    pub fn level(self) -> i32 {
        match self {
            OutlineKind::Part => 0,
            OutlineKind::Chapter => 1,
            OutlineKind::Section => 2,
            OutlineKind::Subsection => 3,
            OutlineKind::Subsubsection => 4,
            OutlineKind::Paragraph => 5,
            OutlineKind::Subparagraph => 6,
            OutlineKind::Frame => 3,
            OutlineKind::Label => 100,
        }
    }

    /// Swift raw value.
    pub fn raw(self) -> &'static str {
        match self {
            OutlineKind::Part => "part",
            OutlineKind::Chapter => "chapter",
            OutlineKind::Section => "section",
            OutlineKind::Subsection => "subsection",
            OutlineKind::Subsubsection => "subsubsection",
            OutlineKind::Paragraph => "paragraph",
            OutlineKind::Subparagraph => "subparagraph",
            OutlineKind::Frame => "frame",
            OutlineKind::Label => "label",
        }
    }
}

/// `\part` … `\subparagraph`.
pub fn sectioning_kind(name: &str) -> Option<OutlineKind> {
    Some(match name {
        "part" => OutlineKind::Part,
        "chapter" => OutlineKind::Chapter,
        "section" => OutlineKind::Section,
        "subsection" => OutlineKind::Subsection,
        "subsubsection" => OutlineKind::Subsubsection,
        "paragraph" => OutlineKind::Paragraph,
        "subparagraph" => OutlineKind::Subparagraph,
        _ => return None,
    })
}

fn takes_reference_name(name: &str) -> bool {
    matches!(
        name,
        "label"
            | "ref"
            | "eqref"
            | "cref"
            | "Cref"
            | "autoref"
            | "pageref"
            | "nameref"
            | "vref"
            | "Vref"
            | "labelcref"
            | "cpageref"
            | "Cpageref"
            | "input"
            | "include"
            | "subfile"
            | "includegraphics"
            | "includepdf"
            | "usepackage"
            | "RequirePackage"
            | "documentclass"
            | "LoadClass"
            | "bibliography"
            | "addbibresource"
            | "bibliographystyle"
            | "lstinputlisting"
            | "bibitem"
            | "includesvg"
    )
}

fn is_structure_name(name: &str) -> bool {
    matches!(
        name,
        "begin"
            | "end"
            | "documentclass"
            | "usepackage"
            | "RequirePackage"
            | "input"
            | "include"
            | "subfile"
            | "maketitle"
            | "tableofcontents"
            | "appendix"
            | "frontmatter"
            | "mainmatter"
            | "backmatter"
            | "bibliography"
            | "printbibliography"
    ) || sectioning_kind(name).is_some()
}

/// Classification of a control word, or `None` for an ordinary command.
pub fn command_info(name: &str) -> Option<CommandInfo> {
    let mut info = CommandInfo::default();
    let mut known = false;
    if takes_reference_name(name) {
        info.takes_reference = true;
        known = true;
    }
    if is_structure_name(name) {
        info.is_structure = true;
        known = true;
    }
    if sectioning_kind(name).is_some() {
        info.allows_star = true;
    }
    match name {
        "begin" => {
            info.special = Special::Begin;
            known = true;
        }
        "end" => {
            info.special = Special::End;
            known = true;
        }
        "verb" | "Verb" | "lstinline" => {
            info.special = Special::Verb;
            known = true;
        }
        "url" | "href" => {
            info.special = Special::Url;
            known = true;
        }
        _ => {}
    }
    if known {
        return Some(info);
    }
    if is_cite_like(name) {
        return Some(CommandInfo { special: Special::None, is_structure: false, allows_star: false, takes_reference: true });
    }
    None
}

/// `\cite`, `\citep`, `\Citet`, `\parencite`, `\textcite`, `\autocite`, `\footcite`, `\nocite` ...
pub fn is_cite_like(name: &str) -> bool {
    let b = name.as_bytes();
    if b.len() < 4 {
        return false;
    }
    let first = b[0];
    let last = b[b.len() - 1];
    if first == b'c' || first == b'C' {
        return b[..4].eq_ignore_ascii_case(b"cite") && !name.eq_ignore_ascii_case("citestyle");
    }
    if last == b'e' {
        return b[b.len() - 4..].eq_ignore_ascii_case(b"cite");
    }
    false
}

// ---------------------------------------------------------------------------------------------------
// Verbatim helpers (shared by tokenizer and source mask)
// ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct VerbatimOpening {
    /// Offset where the verbatim body starts.
    pub body_start: usize,
    /// `[options]` including brackets.
    pub options: Option<Range<usize>>,
    /// `{language}` of minted including braces.
    pub language: Option<Range<usize>>,
}

/// Finds the end of a `[...]` group starting at `open` on the same line (brace aware). Returns the index
/// of the closing `]`.
pub fn closing_bracket_on_line(u: &[u16], open: usize, limit: usize) -> Option<usize> {
    let n = u.len();
    if open >= n || u[open] != 91 {
        return None;
    }
    let mut depth: i32 = 0;
    let mut k = open + 1;
    let stop = n.min(open + limit);
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

/// Index of the closing `}` of a `{...}` group starting at `open` that stays on one line.
pub fn closing_brace_on_line(u: &[u16], open: usize, limit: usize) -> Option<usize> {
    let n = u.len();
    if open >= n || u[open] != 123 {
        return None;
    }
    let mut depth: i32 = 0;
    let mut k = open;
    let stop = n.min(open + limit);
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

pub fn verbatim_opening(u: &[u16], name: &str, from: usize) -> VerbatimOpening {
    let mut opening = VerbatimOpening { body_start: from, options: None, language: None };
    if !is_verbatim_with_options(name) {
        return opening;
    }
    let n = u.len();
    if opening.body_start < n && u[opening.body_start] == 91 {
        if let Some(close) = closing_bracket_on_line(u, opening.body_start, 300) {
            opening.options = Some(opening.body_start..close + 1);
            opening.body_start = close + 1;
        }
    }
    if name == "minted" {
        let mut k = opening.body_start;
        while k < n && is_hspace(u[k]) {
            k += 1;
        }
        if k < n && u[k] == 123 {
            if let Some(close) = closing_brace_on_line(u, k, 200) {
                opening.language = Some(k..close + 1);
                opening.body_start = close + 1;
            }
        }
    }
    opening
}

/// Offset of the `\` of `\end{name}` searching from `from`; `u.len()` when unterminated.
pub fn verbatim_end(u: &[u16], name: &str, from: usize) -> usize {
    let mut pattern: Vec<u16> = Vec::with_capacity(name.len() + 6);
    pattern.extend("\\end{".encode_utf16());
    pattern.extend(name.encode_utf16());
    pattern.push(125);
    let m = pattern.len();
    let n = u.len();
    let mut k = from;
    while k + m <= n {
        if u[k] == 92 {
            let mut t = 1;
            while t < m && u[k + t] == pattern[t] {
                t += 1;
            }
            if t == m {
                return k;
            }
        }
        k += 1;
    }
    n
}

/// Reads an environment name inside `{...}` starting at `open` (a `{`). Returns the name range (without
/// braces) and the offset just after the closing brace.
pub fn environment_name(u: &[u16], open: usize) -> Option<(Range<usize>, usize)> {
    let n = u.len();
    if open >= n || u[open] != 123 {
        return None;
    }
    let mut k = open + 1;
    let stop = n.min(open + 120);
    while k < stop {
        let c = u[k];
        if c == 125 {
            return Some(((open + 1)..k, k + 1));
        }
        if c == 123 || c == 92 || c == 37 || is_line_break(c) {
            return None;
        }
        k += 1;
    }
    None
}

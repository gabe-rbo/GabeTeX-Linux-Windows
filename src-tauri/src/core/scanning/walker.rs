//! Forward walk over a masked source that tracks braces, environments and math mode (port of
//! `StructureWalker.swift`), and the instant structural checks built on it (port of `StructureLinter.swift`).
//! It backs both the linter (walk to the end, report problems) and the completion engine (walk to the cursor,
//! report the innermost open environment and whether the cursor is in math mode).

use super::arguments as args;
use super::masked::MaskedSource;
use super::syntax::*;
use crate::core::log_parser::{LogDiagnostic, Severity, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenBrace {
    pub offset: usize,
    /// The group is the argument of a text-mode command (`\text{...}`) and thus not math.
    pub is_text_group: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenEnvironment {
    pub name: String,
    /// Offset of the `\begin`.
    pub offset: usize,
    /// Offset just past the closing brace of `\begin{name}`.
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub open: OpenEnvironment,
    pub close_name: String,
    pub offset: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrayEnd {
    pub name: String,
    pub offset: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Default)]
pub struct StructureWalk {
    pub stray_closing_braces: Vec<usize>,
    pub open_braces: Vec<OpenBrace>,
    pub open_environments: Vec<OpenEnvironment>,
    pub mismatches: Vec<Mismatch>,
    pub stray_ends: Vec<StrayEnd>,
    pub unclosed_dollars: Vec<usize>,
    pub has_begin_document: bool,
    /// `\end{document}` closed the document: nothing after it is analysed.
    pub document_ended: bool,
    /// The walk covered the whole source (or stopped at `\end{document}`).
    pub reached_end: bool,

    // State at the stop position.
    /// `$...$` or `\(...\)`
    pub inline_math: bool,
    /// `$$...$$` or `\[...\]`
    pub display_math: bool,
    pub math_environment_depth: i32,
    pub text_group_depth: i32,
}

impl StructureWalk {
    /// Math mode at the stop position (text groups such as `\text{..}` count as text).
    pub fn is_in_math(&self) -> bool {
        (self.inline_math || self.display_math || self.math_environment_depth > 0) && self.text_group_depth == 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Command,
    Environment,
    DocumentCommand,
    DocumentEnvironment,
    PlainDef,
    Hook(usize),
}

fn definition_shape(name: &str) -> Option<Shape> {
    Some(match name {
        "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" | "newrobustcmd" | "renewrobustcmd"
        | "providerobustcmd" => Shape::Command,
        "newenvironment" | "renewenvironment" => Shape::Environment,
        "NewDocumentCommand" | "RenewDocumentCommand" | "ProvideDocumentCommand" | "DeclareDocumentCommand"
        | "NewExpandableDocumentCommand" => Shape::DocumentCommand,
        "NewDocumentEnvironment" | "RenewDocumentEnvironment" | "ProvideDocumentEnvironment"
        | "DeclareDocumentEnvironment" => Shape::DocumentEnvironment,
        "def" | "gdef" | "edef" | "xdef" => Shape::PlainDef,
        "AtBeginDocument" | "AtEndDocument" | "AtEndPreamble" => Shape::Hook(1),
        "AtBeginEnvironment" | "AtEndEnvironment" | "BeforeBeginEnvironment" | "AfterBeginEnvironment"
        | "BeforeEndEnvironment" | "AfterEndEnvironment" | "AddToHook" | "NewEnviron" | "RenewEnviron" | "csdef"
        | "csgdef" | "csedef" | "gappto" | "eappto" | "appto" | "preto" => Shape::Hook(2),
        "apptocmd" | "pretocmd" | "patchcmd" => Shape::Hook(4),
        _ => return None,
    })
}

/// Walks `src` up to (not including) `limit`; `None` walks the whole source.
pub fn walk(src: &MaskedSource, limit: Option<usize>) -> StructureWalk {
    let u: &[u16] = &src.masked;
    let n = u.len();
    let stop = limit.unwrap_or(n).min(n);
    let mut walk = StructureWalk::default();

    // 0 none, 1 `$`, 2 `$$`, 3 `\(`, 4 `\[`
    let mut math_mode = 0u8;
    let mut math_open = 0usize;
    let mut last_text_command_end: isize = -1;
    let mut i = 0usize;

    fn pop_environment(walk: &mut StructureWalk) {
        if let Some(top) = walk.open_environments.pop() {
            if is_math_environment(&top.name) {
                walk.math_environment_depth -= 1;
            }
        }
    }

    'walk: while i < stop {
        match u[i] {
            123 => {
                // {
                let mut is_text = false;
                if last_text_command_end >= 0 && (last_text_command_end as usize) <= i {
                    is_text = true;
                    let mut k = last_text_command_end as usize;
                    while k < i {
                        if !is_hspace(u[k]) {
                            is_text = false;
                            break;
                        }
                        k += 1;
                    }
                }
                last_text_command_end = -1;
                walk.open_braces.push(OpenBrace { offset: i, is_text_group: is_text });
                if is_text {
                    walk.text_group_depth += 1;
                }
                i += 1;
            }
            125 => {
                // }
                if let Some(brace) = walk.open_braces.pop() {
                    if brace.is_text_group {
                        walk.text_group_depth -= 1;
                    }
                } else {
                    walk.stray_closing_braces.push(i);
                }
                i += 1;
            }
            36 => {
                // $
                if walk.math_environment_depth > 0 || math_mode == 3 || math_mode == 4 {
                    i += 1;
                } else if math_mode == 0 {
                    if i + 1 < n && u[i + 1] == 36 {
                        math_mode = 2;
                        i += 2;
                    } else {
                        math_mode = 1;
                        i += 1;
                    }
                    math_open = i - if math_mode == 2 { 2 } else { 1 };
                } else if math_mode == 1 {
                    math_mode = 0;
                    i += 1;
                } else {
                    // `$$`
                    if i + 1 < n && u[i + 1] == 36 {
                        math_mode = 0;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            10 | 13 => {
                if src.is_paragraph_break(i) {
                    if math_mode == 1 {
                        walk.unclosed_dollars.push(math_open);
                    }
                    math_mode = 0;
                }
                if u[i] == 13 && i + 1 < n && u[i + 1] == 10 {
                    i += 1;
                }
                i += 1;
            }
            92 => {
                // backslash
                if i + 1 >= n {
                    i += 1;
                    continue 'walk;
                }
                let d = u[i + 1];
                if !is_command_letter(d) {
                    if is_line_break(d) {
                        i += 1;
                        continue 'walk;
                    }
                    if d == 40 && math_mode == 0 && walk.math_environment_depth == 0 {
                        math_mode = 3;
                    } else if d == 41 && math_mode == 3 {
                        math_mode = 0;
                    } else if d == 91 && math_mode == 0 && walk.math_environment_depth == 0 {
                        math_mode = 4;
                    } else if d == 93 && math_mode == 4 {
                        math_mode = 0;
                    }
                    i += if is_high_surrogate(d) && i + 2 < n && is_low_surrogate(u[i + 2]) { 3 } else { 2 };
                    continue 'walk;
                }
                let mut j = i + 2;
                while j < n && is_command_letter(u[j]) {
                    j += 1;
                }
                let mut buf = [0u8; 48];
                let name = ascii_name(u, (i + 1)..j, &mut buf);

                if name == "begin" || name == "end" {
                    let mut k = j;
                    while k < n && is_hspace(u[k]) {
                        k += 1;
                    }
                    // A name cut off by the stop position is not a complete command.
                    let parsed = match environment_name(u, k) {
                        Some((r, end)) if end <= stop => (r, end),
                        _ => {
                            i = j;
                            continue 'walk;
                        }
                    };
                    let env_name = utf16_string(u, parsed.0.clone());
                    let parsed_end = parsed.1;
                    if name == "begin" {
                        if env_name == "document" && walk.open_environments.is_empty() {
                            walk.has_begin_document = true;
                        }
                        if is_math_environment(&env_name) {
                            walk.math_environment_depth += 1;
                        }
                        walk.open_environments.push(OpenEnvironment { name: env_name, offset: i, end: parsed_end });
                    } else if let Some(top) = walk.open_environments.last().cloned() {
                        if top.name == env_name {
                            pop_environment(&mut walk);
                        } else if top.name == "document" {
                            // `\end{x}` with nothing but the document open: a stray end. Closing the document
                            // here would turn the real `\end{document}` into a second, misleading error.
                            walk.stray_ends.push(StrayEnd { name: env_name.clone(), offset: i, end: parsed_end });
                        } else {
                            walk.mismatches.push(Mismatch { open: top, close_name: env_name.clone(), offset: i, end: parsed_end });
                            if let Some(position) = walk.open_environments.iter().rposition(|e| e.name == env_name) {
                                while walk.open_environments.len() > position {
                                    pop_environment(&mut walk);
                                }
                            } else {
                                pop_environment(&mut walk);
                            }
                        }
                        if env_name == "document" && walk.has_begin_document && walk.open_environments.is_empty() {
                            walk.document_ended = true;

                            break 'walk;
                        }
                    } else {
                        walk.stray_ends.push(StrayEnd { name: env_name, offset: i, end: parsed_end });
                    }
                    i = parsed_end;
                    continue 'walk;
                }

                if let Some(shape) = definition_shape(&name) {
                    if let Some(end) = definition_end(u, shape, j) {
                        if end <= stop {
                            i = end;
                            continue 'walk;
                        }
                    }
                }
                if is_text_mode_command(&name) {
                    last_text_command_end = j as isize;
                }
                i = j;
            }
            _ => i += 1,
        }
    }

    walk.inline_math = math_mode == 1 || math_mode == 3;
    walk.display_math = math_mode == 2 || math_mode == 4;
    walk.reached_end = walk.document_ended || stop >= n;
    if walk.reached_end && math_mode == 1 {
        walk.unclosed_dollars.push(math_open);
    }
    walk
}

/// End offset of a complete definition (name, parameters and all bodies), or `None` when it is malformed.
fn definition_end(u: &[u16], shape: Shape, j: usize) -> Option<usize> {
    let n = u.len();
    let mut p = args::skip_whitespace(u, j);
    match shape {
        Shape::Hook(groups) => {
            // Optional `[…]`, an optional bare control sequence (`\appto\cmd{…}`), then up to `groups` braced groups.
            let mut end = p;
            let mut found = 0;
            let mut guard_count = 0;
            while found < groups && guard_count < 8 {
                guard_count += 1;
                end = args::skip_whitespace(u, end);
                if end < n && u[end] == 91 && args::bracketed_limit(u, end, 400).is_some() {
                    end = args::bracketed_limit(u, end, 400)?.1;
                } else if end < n && u[end] == 92 && found == 0 && args::control_sequence(u, end).is_some() {
                    end = args::control_sequence(u, end)?.1;
                } else if end < n && u[end] == 123 && args::braced(u, end).is_some() {
                    end = args::braced(u, end)?.1;
                    found += 1;
                } else {
                    break;
                }
            }
            if found > 0 {
                Some(end)
            } else {
                None
            }
        }
        Shape::PlainDef => {
            if p >= n || u[p] != 92 {
                return None;
            }
            let (_, cs_end) = args::control_sequence(u, p)?;
            let mut k = cs_end;
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
            args::braced(u, k).map(|(_, e)| e)
        }
        Shape::Command | Shape::Environment | Shape::DocumentCommand | Shape::DocumentEnvironment => {
            if p < n && u[p] == 42 {
                p = args::skip_whitespace(u, p + 1);
            }
            // Name: {\foo}, \foo or {name}
            if p < n && u[p] == 123 {
                let (_, end) = args::braced_limit(u, p, 300)?;
                p = end;
            } else if (shape == Shape::Command || shape == Shape::DocumentCommand) && p < n && u[p] == 92 && args::control_sequence(u, p).is_some() {
                p = args::control_sequence(u, p)?.1;
            } else {
                return None;
            }
            p = args::skip_whitespace(u, p);
            let mut bodies = if shape == Shape::Environment || shape == Shape::DocumentEnvironment { 2 } else { 1 };
            if shape == Shape::DocumentCommand || shape == Shape::DocumentEnvironment {
                if !(p < n && u[p] == 123) {
                    return None;
                }
                let (_, spec_end) = args::braced_limit(u, p, 400)?;
                p = args::skip_whitespace(u, spec_end);
            } else {
                let mut optionals = 0;
                while optionals < 2 && p < n && u[p] == 91 {
                    match args::bracketed_limit(u, p, 400) {
                        Some((_, e)) => {
                            p = args::skip_whitespace(u, e);
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
                let (_, body_end) = args::braced(u, end)?;
                end = args::skip_whitespace(u, body_end);
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

// ---------------------------------------------------------------------------------------------------
// StructureLinter
// ---------------------------------------------------------------------------------------------------

/// Instant structural checks run on every edit (no TeX involved): unbalanced `{}` (ignoring `\{` `\}` and
/// comments/verbatim), `\begin{x}` … `\end{y}` mismatches, unclosed inline math `$` at end of paragraph,
/// `\end` without `\begin`. Diagnostics use `Source::Structure`.
///
/// The linter is deliberately conservative — a valid document must never produce a diagnostic: comments,
/// verbatim, `\verb`, `\url` and `\iffalse` blocks are ignored; definition bodies are not analysed; nothing
/// after `\end{document}` is analysed; unclosed `\begin` and stray `\end` are only reported in files
/// containing `\begin{document}`; brace checks are skipped when the file changes category codes.
pub fn lint_text(text: &str, file: &str) -> Vec<LogDiagnostic> {
    lint(&MaskedSource::new(text), file)
}

pub fn lint(src: &MaskedSource, file: &str) -> Vec<LogDiagnostic> {
    let walk = walk(src, None);
    let mut result: Vec<(usize, LogDiagnostic)> = Vec::new();

    let mut add = |severity: Severity, message: String, offset: usize, length: usize| {
        let (line, column) = src.position_at(offset);
        let mut d = LogDiagnostic::new(severity, Source::Structure, message);
        d.path = Some(file.to_string());
        d.line = Some(line as u32);
        d.column = Some(column as u32);
        d.length = Some(length.max(1) as u32);
        result.push((offset, d));
    };

    if !src.has_catcode {
        for &offset in &walk.stray_closing_braces {
            add(Severity::Error, "Unmatched closing brace \u{201C}}\u{201D}".to_string(), offset, 1);
        }
        if walk.reached_end {
            for brace in &walk.open_braces {
                add(Severity::Error, "Missing closing brace \u{201C}}\u{201D} for this \u{201C}{\u{201D}".to_string(), brace.offset, 1);
            }
        }
    }

    for mismatch in &walk.mismatches {
        let open_line = src.line_at(mismatch.open.offset);
        let message = format!(
            "\u{201C}\\begin{{{}}}\u{201D} (line {}) is closed by \u{201C}\\end{{{}}}\u{201D}",
            mismatch.open.name, open_line, mismatch.close_name
        );
        add(Severity::Error, message, mismatch.offset, mismatch.end - mismatch.offset);
    }

    if walk.has_begin_document {
        for stray in &walk.stray_ends {
            add(
                Severity::Error,
                format!("\u{201C}\\end{{{}}}\u{201D} has no matching \u{201C}\\begin\u{201D}", stray.name),
                stray.offset,
                stray.end - stray.offset,
            );
        }
        if walk.reached_end {
            for open in &walk.open_environments {
                add(Severity::Error, format!("Environment \u{201C}{}\u{201D} is never closed", open.name), open.offset, open.end - open.offset);
            }
        }
    }

    for &offset in &walk.unclosed_dollars {
        add(
            Severity::Warning,
            "Inline math is not closed: missing \u{201C}$\u{201D} before the end of the paragraph".to_string(),
            offset,
            1,
        );
    }

    result.sort_by_key(|(offset, _)| *offset);
    result.into_iter().map(|(_, d)| d).collect()
}

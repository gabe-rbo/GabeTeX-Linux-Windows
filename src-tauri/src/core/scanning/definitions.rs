//! Parsers for user definitions (`\newcommand`, `\newenvironment`, ...) used by the scanner
//! (port of `ScannerDefinitions.swift`).

use super::arguments as args;
use super::syntax::*;
use crate::commands::CompletionItem;
use std::ops::Range;

pub const DETAIL: &str = "Defined in project";

/// Command name written either as `{\foo}` or `\foo` at `p`; returns the name (with backslash) and the
/// position after it.
pub fn command_name(u: &[u16], p: usize) -> Option<(String, usize)> {
    if p >= u.len() {
        return None;
    }
    if u[p] == 123 {
        let (content, end) = args::braced_limit(u, p, 200)?;
        let inner = args::trimmed(u, content);
        if inner.len() < 2 || u[inner.start] != 92 {
            return None;
        }
        let (name, cs_end) = args::control_sequence(u, inner.start)?;
        if cs_end != inner.end {
            return None;
        }
        return Some((name, end));
    }
    if u[p] == 92 {
        return args::control_sequence(u, p);
    }
    None
}

type Placeholder = (String, String);

pub fn item(name: &str, placeholders: &[Placeholder]) -> Option<CompletionItem> {
    if !args::is_letter_command_name(name) {
        return None;
    }
    let mut text = String::from(name);
    for (index, (open, close)) in placeholders.iter().enumerate() {
        text.push_str(open);
        text.push_str(&format!("${{{}:arg}}", index + 1));
        text.push_str(close);
    }
    Some(CompletionItem { kind: "command".to_string(), label: name.to_string(), detail: Some(DETAIL.to_string()), insert_text: text })
}

pub fn braces(count: i64) -> Vec<Placeholder> {
    (0..count.max(0)).map(|_| ("{".to_string(), "}".to_string())).collect()
}

fn parse_int(u: &[u16], r: Range<usize>) -> Option<i64> {
    utf16_string(u, r).parse::<i64>().ok()
}

/// `\newcommand{\foo}[2][default]{...}` and friends.
pub fn new_command(u: &[u16], from: usize) -> Option<CompletionItem> {
    let mut p = args::skip_whitespace(u, from);
    if p < u.len() && u[p] == 42 {
        p = args::skip_whitespace(u, p + 1);
    }
    let (name, after_name) = command_name(u, p)?;
    let mut q = args::skip_whitespace(u, after_name);
    let mut count: i64 = 0;
    let mut has_default = false;
    if q < u.len() && u[q] == 91 {
        if let Some((content, end)) = args::bracketed_limit(u, q, 20) {
            count = parse_int(u, args::trimmed(u, content)).unwrap_or(0);
            q = args::skip_whitespace(u, end);
            if q < u.len() && u[q] == 91 && args::bracketed(u, q).is_some() {
                has_default = true;
            }
        }
    }
    item(&name, &braces(count - if has_default { 1 } else { 0 }))
}

/// `\def\foo#1#2{...}`.
pub fn plain_def(u: &[u16], from: usize) -> Option<CompletionItem> {
    let p = args::skip_whitespace(u, from);
    if p >= u.len() || u[p] != 92 {
        return None;
    }
    let (name, cs_end) = args::control_sequence(u, p)?;
    let mut k = cs_end;
    let mut highest: i64 = 0;
    let stop = u.len().min(k + 80);
    while k < stop {
        let c = u[k];
        if c == 123 {
            return item(&name, &braces(highest));
        }
        if c == 35 && k + 1 < u.len() && is_ascii_digit(u[k + 1]) {
            highest = highest.max((u[k + 1] - 48) as i64);
            k += 2;
            continue;
        }
        if is_line_break(c) && k + 1 < u.len() && is_line_break(u[k + 1]) {
            return None;
        }
        k += 1;
    }
    None
}

/// `\DeclareMathOperator{\Hom}{Hom}`.
pub fn math_operator(u: &[u16], from: usize) -> Option<CompletionItem> {
    let mut p = args::skip_whitespace(u, from);
    if p < u.len() && u[p] == 42 {
        p = args::skip_whitespace(u, p + 1);
    }
    let (name, _) = command_name(u, p)?;
    item(&name, &[])
}

/// `\NewDocumentCommand{\foo}{m o m}{...}`.
pub fn document_command(u: &[u16], from: usize) -> Option<CompletionItem> {
    let p = args::skip_whitespace(u, from);
    let (name, after_name) = command_name(u, p)?;
    let q = args::skip_whitespace(u, after_name);
    if q < u.len() && u[q] == 123 {
        if let Some((content, _)) = args::braced_limit(u, q, 400) {
            return item(&name, &signature(u, content));
        }
    }
    item(&name, &[])
}

/// `\DeclarePairedDelimiter{\abs}{\lvert}{\rvert}` and the `X` variants.
pub fn paired_delimiter(u: &[u16], from: usize, has_count: bool) -> Option<CompletionItem> {
    let p = args::skip_whitespace(u, from);
    let (name, after_name) = command_name(u, p)?;
    let mut count: i64 = 1;
    if has_count {
        let q = args::skip_whitespace(u, after_name);
        count = 1;
        if q < u.len() && u[q] == 91 {
            if let Some((content, _)) = args::bracketed_limit(u, q, 20) {
                count = parse_int(u, args::trimmed(u, content)).unwrap_or(1);
            }
        }
    }
    item(&name, &braces(count))
}

fn unit_string(c: u16) -> String {
    String::from_utf16_lossy(&[c])
}

/// xparse argument specification -> placeholders for the mandatory arguments.
pub fn signature(u: &[u16], range: Range<usize>) -> Vec<Placeholder> {
    let mut result: Vec<Placeholder> = Vec::new();
    let mut k = range.start;
    let end = range.end;
    let skip_braced = |k: &mut usize| {
        while *k < end && is_tex_whitespace(u[*k]) {
            *k += 1;
        }
        if *k < end && u[*k] == 123 {
            if let Some((_, e)) = args::braced_limit(u, *k, 400) {
                *k = e;
            }
        }
    };
    while k < end {
        let c = u[k];
        k += 1;
        match c {
            109 | 118 => result.push(("{".to_string(), "}".to_string())), // m v
            114 | 82 => {
                // r R
                if k + 1 < end {
                    result.push((unit_string(u[k]), unit_string(u[k + 1])));
                    k += 2;
                }
                if c == 82 {
                    skip_braced(&mut k);
                }
            }
            79 => skip_braced(&mut k), // O{default}
            100 => k += 2,             // d<>
            68 => {
                k += 2;
                skip_braced(&mut k);
            } // D<>{default}
            116 => k += 1,                                  // t<token>
            101 | 117 | 62 => skip_braced(&mut k),          // e{..} u{..} >{..}
            69 => {
                skip_braced(&mut k);
                skip_braced(&mut k);
            } // E{..}{..}
            _ => {}
        }
    }
    result
}

/// Names defined by an environment-defining command: first mandatory argument (after star / options).
pub fn environment_names(u: &[u16], from: usize, is_list: bool) -> Vec<String> {
    let Some(call) = args::parse(u, from, 1, true) else {
        return Vec::new();
    };
    let Some(first) = call.mandatory.first().cloned() else {
        return Vec::new();
    };
    let ranges: Vec<Range<usize>> = if is_list {
        let mut parts: Vec<Range<usize>> = Vec::new();
        for it in args::split_list(u, first) {
            // thmtools allows "a b" too.
            let mut start = it.start;
            #[allow(clippy::needless_range_loop)]
            for k in it.start..=it.end {
                if k == it.end || is_tex_whitespace(u[k]) {
                    if k > start {
                        parts.push(start..k);
                    }
                    start = k + 1;
                }
            }
        }
        parts
    } else {
        vec![args::trimmed(u, first)]
    };
    ranges
        .into_iter()
        .filter_map(|range| {
            if range.is_empty() {
                return None;
            }
            let name = utf16_string(u, range);
            if name.contains('\\') || name.contains('#') || name.contains(' ') || name.contains('{') {
                return None;
            }
            Some(name)
        })
        .collect()
}

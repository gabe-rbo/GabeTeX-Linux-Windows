//! Parsing of command arguments on a masked buffer (port of `ArgumentParser.swift`, enum `TeXArguments`).

use super::syntax::*;
use std::ops::Range;

/// Longest argument we are willing to scan; bounds the cost of an unclosed brace.
pub const MAX_ARGUMENT_LENGTH: usize = 60_000;

/// Result of [`parse`].
#[derive(Debug, Clone, Default)]
pub struct Call {
    /// Position just after the control word (and optional star).
    pub start: usize,
    pub is_starred: bool,
    /// `[...]` groups (content ranges, brackets excluded), in order of appearance.
    pub optionals: Vec<Range<usize>>,
    /// `{...}` groups (content ranges, braces excluded), in order.
    pub mandatory: Vec<Range<usize>>,
    /// Position just after the last parsed group.
    pub end: usize,
}

/// Skips spaces, tabs and line breaks.
pub fn skip_whitespace(u: &[u16], from: usize) -> usize {
    let mut i = from;
    while i < u.len() && is_tex_whitespace(u[i]) {
        i += 1;
    }
    i
}

/// Content range of the `{...}` group opening at `open` and the index just past its `}`.
pub fn braced(u: &[u16], open: usize) -> Option<(Range<usize>, usize)> {
    braced_limit(u, open, MAX_ARGUMENT_LENGTH)
}

pub fn braced_limit(u: &[u16], open: usize, limit: usize) -> Option<(Range<usize>, usize)> {
    let n = u.len();
    if open >= n || u[open] != 123 {
        return None;
    }
    let mut depth: i32 = 0;
    let mut k = open;
    let stop = n.min(open + limit);
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
                return Some(((open + 1)..k, k + 1));
            }
        }
        k += 1;
    }
    None
}

/// Content range of the `[...]` group opening at `open` (braces inside are skipped).
pub fn bracketed(u: &[u16], open: usize) -> Option<(Range<usize>, usize)> {
    bracketed_limit(u, open, 4_000)
}

pub fn bracketed_limit(u: &[u16], open: usize, limit: usize) -> Option<(Range<usize>, usize)> {
    let n = u.len();
    if open >= n || u[open] != 91 {
        return None;
    }
    let mut braces: i32 = 0;
    let mut k = open + 1;
    let stop = n.min(open + limit);
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
            return Some(((open + 1)..k, k + 1));
        }
        k += 1;
    }
    None
}

/// Parses `[*] ([...])* {...}` repeated for `mandatory_count` mandatory groups. Optional groups may precede
/// each mandatory one. Returns `None` when not even one mandatory argument is present.
pub fn parse(u: &[u16], from: usize, mandatory_count: usize, allow_star: bool) -> Option<Call> {
    let mut i = skip_whitespace(u, from);
    let mut call = Call { start: from, end: from, ..Default::default() };
    if allow_star && i < u.len() && u[i] == 42 {
        call.is_starred = true;
        i += 1;
    }
    call.start = i;
    let mut found = 0;
    while found < mandatory_count {
        i = skip_whitespace(u, i);
        while i < u.len() && u[i] == 91 {
            match bracketed(u, i) {
                Some((content, end)) => {
                    call.optionals.push(content);
                    i = skip_whitespace(u, end);
                }
                None => break,
            }
        }
        if i < u.len() && u[i] == 123 {
            if let Some((content, end)) = braced(u, i) {
                call.mandatory.push(content);
                call.end = end;
                i = end;
                found += 1;
                continue;
            }
        }
        break;
    }
    if found == 0 {
        None
    } else {
        Some(call)
    }
}

/// Splits `range` at top-level commas, trimming whitespace; empty items are dropped.
pub fn split_list(u: &[u16], range: Range<usize>) -> Vec<Range<usize>> {
    let mut items: Vec<Range<usize>> = Vec::new();
    let mut depth: i32 = 0;
    let mut item_start = range.start;
    let mut k = range.start;
    let flush = |items: &mut Vec<Range<usize>>, start: usize, end: usize| {
        let mut a = start;
        let mut b = end;
        while a < b && is_tex_whitespace(u[a]) {
            a += 1;
        }
        while b > a && is_tex_whitespace(u[b - 1]) {
            b -= 1;
        }
        if a < b {
            items.push(a..b);
        }
    };
    while k < range.end {
        let c = u[k];
        if c == 92 {
            k += 2;
            continue;
        }
        if c == 123 {
            depth += 1;
        } else if c == 125 {
            depth -= 1;
        } else if c == 44 && depth == 0 {
            flush(&mut items, item_start, k);
            item_start = k + 1;
        }
        k += 1;
    }
    flush(&mut items, item_start, range.end);
    items
}

/// `range` without leading / trailing whitespace.
pub fn trimmed(u: &[u16], range: Range<usize>) -> Range<usize> {
    let mut a = range.start;
    let mut b = range.end;
    while a < b && is_tex_whitespace(u[a]) {
        a += 1;
    }
    while b > a && is_tex_whitespace(u[b - 1]) {
        b -= 1;
    }
    a..b
}

/// A control sequence name at `i` (`\foo` or `\,`): returns the name including the backslash and the end.
pub fn control_sequence(u: &[u16], i: usize) -> Option<(String, usize)> {
    let n = u.len();
    if i + 1 >= n || u[i] != 92 {
        return None;
    }
    if is_command_letter(u[i + 1]) {
        let mut j = i + 2;
        while j < n && is_command_letter(u[j]) {
            j += 1;
        }
        return Some((utf16_string(u, i..j), j));
    }
    Some((utf16_string(u, i..i + 2), i + 2))
}

/// A definable command name: a backslash followed by letters only.
pub fn is_letter_command_name(name: &str) -> bool {
    let mut chars = name.chars();
    if chars.next() != Some('\\') {
        return false;
    }
    let rest = chars.as_str();
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphabetic())
}

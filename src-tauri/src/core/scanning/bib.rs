//! Tolerant BibTeX/BibLaTeX parser (port of `BibParser.swift`): extracts entry keys plus title/author/year.
//! Skips `@comment`, `@preamble`, `@string` and free text between entries; handles braces/quotes nesting and
//! both `{}` and `()` delimiters. Top-level `%` comment lines are ignored. Values are returned without outer
//! delimiters, without the protective inner braces, and with whitespace collapsed.

use super::arguments as args;
use super::models::*;
use super::syntax::*;
use std::collections::HashMap;

pub fn entries(text: &str, file: &str) -> Vec<BibRecord> {
    let mut parser = Parser::new(units(text), FilePath::from(file));
    parser.run()
}

struct Parser {
    u: Vec<u16>,
    n: usize,
    file: FilePath,
    line_starts: Vec<usize>,
    entries: Vec<BibRecord>,
}

impl Parser {
    fn new(u: Vec<u16>, file: FilePath) -> Self {
        let n = u.len();
        let mut line_starts = vec![0usize];
        let mut i = 0;
        while i < n {
            if u[i] == 10 {
                line_starts.push(i + 1);
            } else if u[i] == 13 {
                if i + 1 < n && u[i + 1] == 10 {
                    i += 1;
                }
                line_starts.push(i + 1);
            }
            i += 1;
        }
        Parser { u, n, file, line_starts, entries: Vec::new() }
    }

    fn line_at(&self, offset: usize) -> usize {
        let (mut lo, mut hi) = (0usize, self.line_starts.len() - 1);
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

    fn run(&mut self) -> Vec<BibRecord> {
        let n = self.n;
        let mut i = 0;
        while i < n {
            let c = self.u[i];
            if c == 37 {
                // top-level comment line
                while i < n && !is_line_break(self.u[i]) {
                    i += 1;
                }
            } else if c == 64 {
                i = self.entry(i);
            } else {
                i += 1;
            }
        }
        std::mem::take(&mut self.entries)
    }

    /// True when `index` starts a line (after optional blanks) holding `@` + letter.
    fn starts_entry_line(&self, index: usize) -> bool {
        let (u, n) = (&self.u, self.n);
        let mut k = index;
        while k < n && is_hspace(u[k]) {
            k += 1;
        }
        k + 1 < n && u[k] == 64 && is_ascii_letter(u[k + 1])
    }

    /// Start of the next line that begins an entry, after `from`.
    fn next_entry_line_start(&self, from: usize) -> Option<usize> {
        let (u, n) = (&self.u, self.n);
        let mut k = from;
        while k < n {
            if is_line_break(u[k]) && self.starts_entry_line(k + 1) {
                let mut s = k + 1;
                while s < n && is_hspace(u[s]) {
                    s += 1;
                }
                return Some(s);
            }
            k += 1;
        }
        None
    }

    fn entry(&mut self, start: usize) -> usize {
        let n = self.n;
        let mut i = start + 1;
        while i < n && is_tex_whitespace(self.u[i]) {
            i += 1;
        }
        let type_start = i;
        while i < n && (is_ascii_letter(self.u[i]) || is_ascii_digit(self.u[i]) || self.u[i] == 95 || self.u[i] == 45) {
            i += 1;
        }
        let ty = utf16_string(&self.u, type_start..i).to_lowercase();
        while i < n && is_tex_whitespace(self.u[i]) {
            i += 1;
        }
        if ty.is_empty() || i >= n || !(self.u[i] == 123 || self.u[i] == 40) {
            return i.max(start + 1);
        }
        let open = self.u[i];
        let close: u16 = if open == 123 { 125 } else { 41 };
        if ty == "comment" || ty == "preamble" || ty == "string" {
            return self.skip_group(i, open).unwrap_or_else(|| self.next_entry_line_start(i).unwrap_or(n));
        }
        self.parse_entry(ty, start, i + 1, close)
    }

    /// Skips a balanced group starting at `from`; returns the index after the closing delimiter.
    fn skip_group(&self, from: usize, open: u16) -> Option<usize> {
        if open == 123 {
            return self.match_brace(from);
        }
        let (u, n) = (&self.u, self.n);
        let mut braces: i32 = 0;
        let mut in_quote = false;
        let mut i = from + 1;
        while i < n {
            let c = u[i];
            if c == 92 {
                i += 2;
                continue;
            }
            if c == 123 {
                braces += 1;
            } else if c == 125 {
                braces -= 1;
            } else if c == 34 && braces <= 0 {
                in_quote = !in_quote;
            } else if c == 41 && braces <= 0 && !in_quote {
                return Some(i + 1);
            } else if is_line_break(c) && (braces > 0 || in_quote) && self.starts_entry_line(i + 1) {
                return None;
            }
            i += 1;
        }
        None
    }

    fn parse_entry(&mut self, ty: String, offset: usize, body_start: usize, close: u16) -> usize {
        let n = self.n;
        let mut i = body_start;
        let key_start = i;
        while i < n && self.u[i] != 44 && self.u[i] != close && self.u[i] != 123 && self.u[i] != 61 {
            i += 1;
        }
        let key = utf16_string(&self.u, args::trimmed(&self.u, key_start..i));
        let mut fields: HashMap<String, String> = HashMap::new();
        if key.is_empty() || i >= n {
            return i;
        }
        if self.u[i] == close {
            self.append(&ty, &key, &fields, offset);
            return i + 1;
        }
        if self.u[i] != 44 {
            return i; // malformed header
        }
        i += 1;
        while i < n {
            while i < n && (is_tex_whitespace(self.u[i]) || self.u[i] == 44) {
                i += 1;
            }
            if i >= n {
                break;
            }
            if self.u[i] == close {
                i += 1;
                break;
            }
            if self.u[i] == 64 {
                break; // next entry starts: this one was left unterminated
            }
            let name_start = i;
            while i < n && self.u[i] != 61 && self.u[i] != 44 && self.u[i] != close && !is_tex_whitespace(self.u[i]) {
                i += 1;
            }
            let name = utf16_string(&self.u, name_start..i).to_lowercase();
            while i < n && is_tex_whitespace(self.u[i]) {
                i += 1;
            }
            if !(i < n && self.u[i] == 61) {
                while i < n && self.u[i] != 44 && self.u[i] != close && self.u[i] != 64 {
                    i += 1;
                }
                continue;
            }
            i += 1;
            match self.parse_value(i, close) {
                None => {
                    self.append(&ty, &key, &fields, offset);
                    return self.next_entry_line_start(i).unwrap_or(n);
                }
                Some((value, end)) => {
                    i = end;
                    fields.entry(name).or_insert(value);
                }
            }
        }
        self.append(&ty, &key, &fields, offset);
        i
    }

    fn append(&mut self, ty: &str, key: &str, fields: &HashMap<String, String>, offset: usize) {
        let mut year = fields.get("year").cloned();
        if year.is_none() {
            if let Some(date) = fields.get("date") {
                if date.chars().count() >= 4 {
                    year = Some(date.chars().take(4).collect());
                }
            }
        }
        let line = self.line_at(offset);
        self.entries.push(BibRecord {
            key: key.to_string(),
            entry_type: ty.to_string(),
            title: fields.get("title").cloned(),
            author: fields.get("author").or_else(|| fields.get("editor")).cloned(),
            year,
            file: self.file.clone(),
            line,
        });
    }

    /// Parses `value (# value)*`. Returns `None` when the text ends or the next entry starts inside it.
    fn parse_value(&self, start: usize, close: u16) -> Option<(String, usize)> {
        let (u, n) = (&self.u, self.n);
        let mut i = start;
        let mut pieces: Vec<String> = Vec::new();
        loop {
            while i < n && is_tex_whitespace(u[i]) {
                i += 1;
            }
            if i >= n {
                return None;
            }
            let c = u[i];
            if c == 123 {
                let end = self.match_brace(i)?;
                pieces.push(self.clean(i + 1, end - 1, true));
                i = end;
            } else if c == 34 {
                let mut k = i + 1;
                let mut depth: i32 = 0;
                while k < n {
                    let d = u[k];
                    if d == 92 {
                        k += 2;
                        continue;
                    }
                    if d == 123 {
                        depth += 1;
                    } else if d == 125 {
                        depth -= 1;
                    } else if d == 34 && depth <= 0 {
                        break;
                    } else if is_line_break(d) && depth > 0 && self.starts_entry_line(k + 1) {
                        return None;
                    }
                    k += 1;
                }
                if k >= n {
                    return None;
                }
                pieces.push(self.clean(i + 1, k, true));
                i = k + 1;
            } else {
                let s = i;
                while i < n && u[i] != 44 && u[i] != 35 && u[i] != close && !is_tex_whitespace(u[i]) {
                    i += 1;
                }
                pieces.push(self.clean(s, i, true));
            }
            while i < n && is_tex_whitespace(u[i]) {
                i += 1;
            }
            if i < n && u[i] == 35 {
                i += 1;
                continue;
            }
            let value = if pieces.len() == 1 { pieces[0].trim().to_string() } else { collapse(&pieces.concat()) };
            return Some((value, i));
        }
    }

    /// Index just past the `}` matching the `{` at `open`; `None` when unbalanced up to the next entry / EOF.
    fn match_brace(&self, open: usize) -> Option<usize> {
        let (u, n) = (&self.u, self.n);
        let mut depth: i32 = 0;
        let mut i = open;
        while i < n {
            let c = u[i];
            if c == 92 {
                i += 2;
                continue;
            }
            if c == 123 {
                depth += 1;
            } else if c == 125 {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            } else if is_line_break(c) && self.starts_entry_line(i + 1) {
                return None;
            }
            i += 1;
        }
        None
    }

    /// Text of `[a, b)` without brace characters, whitespace collapsed. With `keep_edges`, leading and
    /// trailing whitespace survives as a single space (needed when pieces are concatenated with `#`).
    fn clean(&self, a: usize, b: usize, keep_edges: bool) -> String {
        let u = &self.u;
        let mut out: Vec<u16> = Vec::with_capacity(b.saturating_sub(a));
        let mut pending_space = keep_edges && a < b && is_tex_whitespace(u[a]);
        let mut i = a;
        while i < b {
            let c = u[i];
            if c == 92 && i + 1 < b && (u[i + 1] == 123 || u[i + 1] == 125) {
                // \{ \} keep the brace
                if pending_space {
                    out.push(32);
                    pending_space = false;
                }
                out.push(u[i + 1]);
                i += 2;
                continue;
            }
            if c == 123 || c == 125 {
                i += 1;
                continue;
            }
            if is_tex_whitespace(c) {
                pending_space = keep_edges || !out.is_empty();
            } else {
                if pending_space {
                    out.push(32);
                    pending_space = false;
                }
                out.push(c);
            }
            i += 1;
        }
        if keep_edges && pending_space {
            out.push(32);
        }
        String::from_utf16_lossy(&out)
    }
}

/// Collapses runs of whitespace and trims the ends.
fn collapse(text: &str) -> String {
    let mut out: Vec<u16> = Vec::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.encode_utf16() {
        if is_tex_whitespace(c) {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(32);
                pending_space = false;
            }
            out.push(c);
        }
    }
    String::from_utf16_lossy(&out)
}

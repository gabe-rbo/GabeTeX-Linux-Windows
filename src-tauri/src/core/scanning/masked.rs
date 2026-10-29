//! `MaskedSource` (port of `Scanning/MaskedSource.swift`) and `LineIndex` (port of `Text/LineIndex.swift`).

use super::syntax::*;
use std::ops::Range;

/// A UTF-16 snapshot of a LaTeX source together with a "masked" copy in which everything that must not be
/// interpreted as TeX is blanked out with spaces (line breaks are kept, so offsets and line numbers are
/// identical in both copies): comments, bodies of verbatim-like environments, `\verb|...|` /
/// `\lstinline|...|` / `\mintinline{..}|...|` contents, raw `\url{...}` / `\href{...}` first arguments and
/// `\iffalse ... \fi` blocks that start a line.
#[derive(Debug, Clone)]
pub struct MaskedSource {
    pub original: Vec<u16>,
    pub masked: Vec<u16>,
    /// Blanked comment ranges (excluding the line terminator), in order.
    pub comments: Vec<Range<usize>>,
    /// UTF-16 offset of the first character of each line (`\n`, `\r\n` and `\r` terminators).
    pub line_starts: Vec<usize>,
    /// True when the source changes category codes (`\catcode`): structural checks become unreliable.
    pub has_catcode: bool,
}

impl MaskedSource {
    pub fn new(text: &str) -> Self {
        Self::from_units(units(text))
    }

    pub fn from_units(units: Vec<u16>) -> Self {
        let mut s = MaskedSource {
            masked: units.clone(),
            original: units,
            comments: Vec::new(),
            line_starts: vec![0],
            has_catcode: false,
        };
        s.build();
        s
    }

    pub fn len(&self) -> usize {
        self.original.len()
    }

    pub fn is_empty(&self) -> bool {
        self.original.is_empty()
    }

    // ----- Queries -----

    /// 1-based line containing `offset`.
    pub fn line_at(&self, offset: usize) -> usize {
        let ls = &self.line_starts;
        let mut lo = 0usize;
        let mut hi = ls.len() - 1;
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if ls[mid] <= offset {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo + 1
    }

    /// 1-based (line, column) of `offset`.
    pub fn position_at(&self, offset: usize) -> (usize, usize) {
        let l = self.line_at(offset);
        (l, offset - self.line_starts[l - 1] + 1)
    }

    /// Text of a range taken from the masked copy.
    pub fn masked_string(&self, r: Range<usize>) -> String {
        utf16_string(&self.masked, r)
    }

    /// Text of a range taken from the original, with comments replaced by a single space (so titles that
    /// span a comment still read naturally) but verbatim contents kept.
    pub fn text_without_comments(&self, r: Range<usize>) -> Vec<u16> {
        let mut out: Vec<u16> = Vec::with_capacity(r.len());
        let mut pos = r.start;
        // First comment that ends after r.start.
        let mut lo = 0usize;
        let mut hi = self.comments.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.comments[mid].end <= r.start {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut idx = lo;
        while idx < self.comments.len() && self.comments[idx].start < r.end {
            let c = &self.comments[idx];
            if c.start > pos {
                out.extend_from_slice(&self.original[pos..c.start]);
            }
            out.push(32);
            pos = pos.max(c.end);
            idx += 1;
        }
        if pos < r.end {
            out.extend_from_slice(&self.original[pos..r.end]);
        }
        out
    }

    /// True when the line break at `index` (a `\n` or `\r`) is followed by a whitespace-only line in the
    /// *original* text, i.e. a paragraph break. A line holding only a comment is not blank.
    pub fn is_paragraph_break(&self, index: usize) -> bool {
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

    // ----- Construction -----

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
                    self.comments.push(i..j);
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
                // "\r\n" registers its line start at the "\n".
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
        match d {
            98 | 118 | 86 | 108 | 109 | 117 | 104 | 105 | 99 => {}
            _ => return j,
        }
        let mut buf = [0u8; 48];
        let name = ascii_name(&self.original, (i + 1)..j, &mut buf).into_owned();
        match name.as_str() {
            "begin" => self.mask_verbatim_environment(j),
            "verb" | "Verb" => self.mask_delimited(j, true).unwrap_or(j),
            "lstinline" => self.mask_lstinline(j).unwrap_or(j),
            "mintinline" => self.mask_mintinline(j).unwrap_or(j),
            "url" | "href" => self.mask_raw_group(j).unwrap_or(j),
            "iffalse" => self.mask_iffalse(i, j).unwrap_or(j),
            "catcode" => {
                self.has_catcode = true;
                j
            }
            _ => j,
        }
    }

    fn mask_verbatim_environment(&mut self, j: usize) -> usize {
        let n = self.original.len();
        let mut k = j;
        while k < n && is_hspace(self.original[k]) {
            k += 1;
        }
        let Some((name_range, parsed_end)) = environment_name(&self.original, k) else {
            return j;
        };
        let name = utf16_string(&self.original, name_range);
        if !is_verbatim_environment(&name) {
            return j;
        }
        let opening = verbatim_opening(&self.original, &name, parsed_end);
        let end = verbatim_end(&self.original, &name, opening.body_start);
        if end > opening.body_start {
            self.blank_body(opening.body_start, end);
        }
        end.max(parsed_end)
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
        if m >= n || self.original[m] != delimiter {
            return None;
        }
        self.blank(j, m + 1);
        Some(m + 1)
    }

    fn mask_lstinline(&mut self, j: usize) -> Option<usize> {
        let n = self.original.len();
        let mut k = j;
        if k < n && self.original[k] == 91 {
            if let Some(close) = closing_bracket_on_line(&self.original, k, 300) {
                k = close + 1;
            }
        }
        if k >= n || self.original[k] == 123 {
            return None;
        }
        self.mask_delimited(k, false)
    }

    fn mask_mintinline(&mut self, j: usize) -> Option<usize> {
        let n = self.original.len();
        let mut k = j;
        if k < n && self.original[k] == 91 {
            if let Some(close) = closing_bracket_on_line(&self.original, k, 300) {
                k = close + 1;
            }
        }
        if k >= n || self.original[k] != 123 {
            return None;
        }
        let lang_close = closing_brace_on_line(&self.original, k, 200)?;
        k = lang_close + 1;
        if k >= n || self.original[k] == 123 {
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
        if k >= n || self.original[k] != 123 {
            return None;
        }
        let mut depth: i32 = 0;
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

    fn opens_conditional(name: &str) -> bool {
        if !name.starts_with("if") || name.len() <= 2 {
            return false;
        }
        if name == "iff" || name == "ifthenelse" {
            return false;
        }
        const PREFIXES: [&str; 18] = [
            "ifdef", "ifundef", "ifstr", "ifbool", "ifblank", "ifnumcomp", "ifnumequal", "ifnumgreater", "ifnumless",
            "ifdimcomp", "ifdimequal", "ifdimgreater", "ifdimless", "ifcsdef", "ifcsundef", "ifcsstring", "ifcsempty",
            "iffontchar",
        ];
        for p in PREFIXES {
            if name.starts_with(p) {
                return false;
            }
        }
        true
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
                    let mut buf = [0u8; 48];
                    let name = ascii_name(&self.original, (m + 1)..e, &mut buf);
                    if name == "fi" {
                        depth -= 1;
                        if depth == 0 {
                            self.blank_body(name_end, m);
                            // Comments inside the blanked block no longer count as comments.
                            return Some(m);
                        }
                    } else if Self::opens_conditional(&name) {
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

// ---------------------------------------------------------------------------------------------------
// LineIndex
// ---------------------------------------------------------------------------------------------------

/// A position in a source file. Both components are **1-based**; `column` counts UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourcePosition {
    pub line: usize,
    pub column: usize,
}

impl SourcePosition {
    pub fn new(line: usize, column: usize) -> Self {
        SourcePosition { line, column }
    }
}

/// Maps between UTF-16 offsets and 1-based (line, column) positions. Recognizes `\n`, `\r\n` and `\r`.
/// Build once per text snapshot: O(n) build, O(log n) queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    /// UTF-16 offset of the first character of each line.
    pub line_starts: Vec<usize>,
    /// UTF-16 offset just past the last character of each line, *excluding* its terminator.
    pub line_ends: Vec<usize>,
    /// Total UTF-16 length of the text.
    pub length: usize,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        Self::from_units(&units(text))
    }

    pub fn from_units(buffer: &[u16]) -> Self {
        let len = buffer.len();
        let mut starts = vec![0usize];
        let mut ends: Vec<usize> = Vec::new();
        let mut i = 0;
        while i < len {
            let c = buffer[i];
            if c == 10 {
                ends.push(i);
                starts.push(i + 1);
            } else if c == 13 {
                ends.push(i);
                if i + 1 < len && buffer[i + 1] == 10 {
                    i += 1;
                }
                starts.push(i + 1);
            }
            i += 1;
        }
        ends.push(len);
        LineIndex { line_starts: starts, line_ends: ends, length: len }
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// 1-based line containing `offset` (offsets past the end map to the last line).
    pub fn line_of_offset(&self, offset: usize) -> usize {
        let target = offset.min(self.length);
        let (mut lo, mut hi) = (0usize, self.line_starts.len() - 1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.line_starts[mid] <= target {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo + 1
    }

    pub fn position_of_offset(&self, offset: usize) -> SourcePosition {
        let line = self.line_of_offset(offset);
        let start = self.line_starts[line - 1];
        SourcePosition { line, column: offset.min(self.length) - start + 1 }
    }

    /// UTF-16 offset for a 1-based position; line and column are clamped into the text.
    pub fn offset_of(&self, position: SourcePosition) -> usize {
        let l = position.line.max(1).min(self.line_count());
        let start = self.line_starts[l - 1];
        let end = self.line_ends[l - 1];
        let want = (start + position.column.max(1) - 1).min(end);
        want.max(start)
    }

    /// Range of the line's contents, excluding the terminator. Line is 1-based and clamped.
    pub fn contents_range(&self, line: usize) -> Range<usize> {
        let l = line.max(1).min(self.line_count());
        self.line_starts[l - 1]..self.line_ends[l - 1]
    }

    /// Range of the line including its terminator.
    pub fn full_range(&self, line: usize) -> Range<usize> {
        let l = line.max(1).min(self.line_count());
        let end = if l < self.line_count() { self.line_starts[l] } else { self.length };
        self.line_starts[l - 1]..end
    }
}

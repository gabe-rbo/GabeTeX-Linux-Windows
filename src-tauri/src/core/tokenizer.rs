//! Fast single-pass LaTeX lexer for highlighting (port of `Syntax/LaTeXTokenizer.swift` and
//! `TokenizerEngine.swift`).
//!
//! O(n) over UTF-16; tracks math mode across lines (`$…$`, `$$…$$`, `\(…\)`, `\[…\]`, and math
//! environments such as equation, align, gather, multline, flalign, alignat, eqnarray, math, displaymath —
//! starred too); tokens are sorted, non-overlapping; plain text produces no tokens. Token ranges are UTF-16
//! offsets in the tokenized text (CodeMirror positions).

use crate::core::scanner::syntax::*;
use serde::{Deserialize, Serialize};

/// Token classes used for syntax highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TokenKind {
    /// `\foo`, `\\`, `\{` — outside math
    Command,
    /// `\part \chapter \section ... \paragraph`, `\begin`, `\end`, `\documentclass`, `\usepackage`
    StructureCommand,
    /// "itemize" in `\begin{itemize}` / `\end{itemize}`
    EnvironmentName,
    /// `$  $$  \(  \)  \[  \]` and `\begin`/`\end` of math environments
    MathDelimiter,
    /// ordinary content inside math mode
    Math,
    /// `\alpha`, `\frac` ... inside math mode
    MathCommand,
    /// `% ...` end of line
    Comment,
    /// `% !TEX ...` lines
    MagicComment,
    /// `{ }`
    Brace,
    /// `[ ]`
    Bracket,
    /// `& ~ ^ _ #`
    SpecialCharacter,
    /// `\verb|..|`, verbatim / lstlisting / minted / comment environments' bodies
    Verbatim,
    /// argument of `\label \ref \eqref \cref \cite* \input \include \includegraphics \usepackage ...`
    ReferenceArgument,
}

impl TokenKind {
    /// The Swift raw value (identical to the serde name).
    pub fn raw(self) -> &'static str {
        match self {
            TokenKind::Command => "command",
            TokenKind::StructureCommand => "structureCommand",
            TokenKind::EnvironmentName => "environmentName",
            TokenKind::MathDelimiter => "mathDelimiter",
            TokenKind::Math => "math",
            TokenKind::MathCommand => "mathCommand",
            TokenKind::Comment => "comment",
            TokenKind::MagicComment => "magicComment",
            TokenKind::Brace => "brace",
            TokenKind::Bracket => "bracket",
            TokenKind::SpecialCharacter => "specialCharacter",
            TokenKind::Verbatim => "verbatim",
            TokenKind::ReferenceArgument => "referenceArgument",
        }
    }
}

/// A highlighted span: `start..start + length` in UTF-16 units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Token {
    pub kind: TokenKind,
    /// UTF-16 offset.
    pub start: usize,
    /// Length in UTF-16 units.
    pub length: usize,
}

impl Token {
    pub fn end(&self) -> usize {
        self.start + self.length
    }
}

/// Tokenizes `text` (UTF-16 offsets refer to this string).
pub fn tokenize(text: &str) -> Vec<Token> {
    tokenize_units(&units(text))
}

/// Tokenizes a UTF-16 buffer.
pub fn tokenize_units(u: &[u16]) -> Vec<Token> {
    Engine::new(u).run()
}

/// `kind:text` description of every token (test / debugging helper).
pub fn describe(text: &str) -> Vec<String> {
    let u = units(text);
    tokenize_units(&u)
        .into_iter()
        .map(|t| format!("{}:{}", t.kind.raw(), String::from_utf16_lossy(&u[t.start..t.end()])))
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArgKind {
    Reference,
    Verbatim,
}

#[derive(Clone, Copy)]
struct PendingArg {
    open: usize,
    close: usize,
    kind: ArgKind,
}

struct Engine<'a> {
    u: &'a [u16],
    n: usize,
    tokens: Vec<Token>,

    // Math state: 0 none, 1 `$`, 2 `$$`, 3 `\(`, 4 `\[`, 5 math environment.
    math_mode: u8,
    math_env_name: String,
    run_start: usize,
    /// Number of open `{` groups when the current `$…$` started: a `$` seen inside a deeper group
    /// (`$a \text{if $b$ then} c$`) belongs to a nested formula and must not close the outer one.
    math_brace_depth: usize,

    // "Argument chaining": whether a `[` / `{` right here is an argument of the previous command.
    // 0 none, 1 after a control word (spaces allowed), 2 after an argument group (no spaces).
    chain: u8,
    brace_flags: Vec<bool>,
    bracket_depths: Vec<usize>,
    pending_args: Vec<PendingArg>,
}

impl<'a> Engine<'a> {
    fn new(u: &'a [u16]) -> Self {
        Engine {
            u,
            n: u.len(),
            tokens: Vec::with_capacity(u.len() / 6 + 4),
            math_mode: 0,
            math_env_name: String::new(),
            run_start: 0,
            math_brace_depth: 0,
            chain: 0,
            brace_flags: Vec::new(),
            bracket_depths: Vec::new(),
            pending_args: Vec::new(),
        }
    }

    fn run(mut self) -> Vec<Token> {
        let (u, n) = (self.u, self.n);
        let mut i = 0;
        while i < n {
            match u[i] {
                92 => i = self.backslash(i),
                37 => i = self.comment(i),
                36 => i = self.dollar(i),
                123 => i = self.open_brace(i),
                125 => i = self.close_brace(i),
                91 => i = self.open_bracket(i),
                93 => i = self.close_bracket(i),
                38 | 126 | 94 | 95 | 35 => {
                    self.add(TokenKind::SpecialCharacter, i, i + 1);
                    self.chain = 0;
                    i += 1;
                }
                10 | 13 => i = self.newline(i),
                32 | 9 => {
                    if self.chain == 2 {
                        self.chain = 0;
                    }
                    i += 1;
                }
                _ => {
                    self.chain = 0;
                    i += 1;
                }
            }
        }
        if self.math_mode != 0 {
            self.flush_math_run(n);
        }
        self.tokens
    }

    // ----- Emission -----

    #[inline]
    fn add(&mut self, kind: TokenKind, start: usize, end: usize) {
        if self.math_mode != 0 {
            self.flush_math_run(start);
        }
        self.tokens.push(Token { kind, start, length: end - start });
        if self.math_mode != 0 {
            self.run_start = end;
        }
    }

    fn flush_math_run(&mut self, stop: usize) {
        let u = self.u;
        let mut a = self.run_start;
        let mut b = stop;
        while a < b && is_tex_whitespace(u[a]) {
            a += 1;
        }
        while b > a && is_tex_whitespace(u[b - 1]) {
            b -= 1;
        }
        if a < b {
            self.tokens.push(Token { kind: TokenKind::Math, start: a, length: b - a });
        }
        self.run_start = stop;
    }

    fn command_kind(&self) -> TokenKind {
        if self.math_mode != 0 {
            TokenKind::MathCommand
        } else {
            TokenKind::Command
        }
    }

    // ----- Comments and line breaks -----

    fn comment(&mut self, i: usize) -> usize {
        let (u, n) = (self.u, self.n);
        let mut j = i + 1;
        while j < n && u[j] != 10 && u[j] != 13 {
            j += 1;
        }
        let kind = if self.is_magic_comment(i, j) { TokenKind::MagicComment } else { TokenKind::Comment };
        self.add(kind, i, j);
        self.chain = 0;
        j
    }

    fn is_magic_comment(&self, start: usize, end: usize) -> bool {
        let u = self.u;
        let mut k = start as isize - 1;
        while k >= 0 && is_hspace(u[k as usize]) {
            k -= 1;
        }
        if k >= 0 && !is_line_break(u[k as usize]) {
            return false;
        }
        let mut k = start + 1;
        while k < end && is_hspace(u[k]) {
            k += 1;
        }
        if !(k < end && u[k] == 33) {
            return false;
        }
        k += 1;
        let word_start = k;
        while k < end && is_ascii_letter(u[k]) {
            k += 1;
        }
        if k - word_start != 3 {
            return false;
        }
        let (w0, w1, w2) = (ascii_lower(u[word_start]), ascii_lower(u[word_start + 1]), ascii_lower(u[word_start + 2]));
        let is_tex = w0 == 116 && w1 == 101 && w2 == 120; // tex
        let is_bib = w0 == 98 && w1 == 105 && w2 == 98; // bib
        is_tex || is_bib
    }

    fn newline(&mut self, i: usize) -> usize {
        let (u, n) = (self.u, self.n);
        let mut j = i + 1;
        if u[i] == 13 && j < n && u[j] == 10 {
            j += 1;
        }
        let mut k = j;
        while k < n && is_hspace(u[k]) {
            k += 1;
        }
        if k < n && is_line_break(u[k]) {
            // Blank line: paragraph break.
            self.chain = 0;
            self.bracket_depths.clear();
            if self.math_mode == 1 || self.math_mode == 3 {
                self.flush_math_run(i);
                self.math_mode = 0;
            }
        } else if self.chain == 2 {
            self.chain = 0;
        }
        j
    }

    // ----- Math delimiters -----

    fn dollar(&mut self, i: usize) -> usize {
        let (u, n) = (self.u, self.n);
        self.chain = 0;
        match self.math_mode {
            0 => {
                if i + 1 < n && u[i + 1] == 36 {
                    self.add(TokenKind::MathDelimiter, i, i + 2);
                    self.math_mode = 2;
                    self.run_start = i + 2;
                    return i + 2;
                }
                self.add(TokenKind::MathDelimiter, i, i + 1);
                self.math_mode = 1;
                self.math_brace_depth = self.brace_flags.len();
                self.run_start = i + 1;
                i + 1
            }
            1 => {
                self.add(TokenKind::MathDelimiter, i, i + 1);
                if self.brace_flags.len() <= self.math_brace_depth {
                    self.math_mode = 0;
                }
                i + 1
            }
            2 => {
                if i + 1 < n && u[i + 1] == 36 {
                    self.add(TokenKind::MathDelimiter, i, i + 2);
                    self.math_mode = 0;
                    return i + 2;
                }
                self.add(TokenKind::MathDelimiter, i, i + 1);
                i + 1
            }
            _ => {
                self.add(TokenKind::MathDelimiter, i, i + 1); // nested inline math inside \text{...}
                i + 1
            }
        }
    }

    // ----- Braces and brackets -----

    fn open_brace(&mut self, i: usize) -> usize {
        if !self.pending_args.is_empty() {
            while let Some(top) = self.pending_args.last() {
                if top.open < i {
                    self.pending_args.pop();
                } else {
                    break;
                }
            }
            if let Some(&top) = self.pending_args.last() {
                if top.open == i {
                    self.pending_args.pop();
                    self.add(TokenKind::Brace, i, i + 1);
                    if top.close > i + 1 {
                        let kind = if top.kind == ArgKind::Reference { TokenKind::ReferenceArgument } else { TokenKind::Verbatim };
                        self.add(kind, i + 1, top.close);
                    }
                    self.add(TokenKind::Brace, top.close, top.close + 1);
                    self.chain = 2;
                    return top.close + 1;
                }
            }
        }
        self.brace_flags.push(self.chain != 0);
        self.add(TokenKind::Brace, i, i + 1);
        self.chain = 0;
        i + 1
    }

    fn close_brace(&mut self, i: usize) -> usize {
        let was_argument = self.brace_flags.pop().unwrap_or(false);
        self.add(TokenKind::Brace, i, i + 1);
        self.chain = if was_argument { 2 } else { 0 };
        i + 1
    }

    fn open_bracket(&mut self, i: usize) -> usize {
        if self.chain != 0 {
            self.add(TokenKind::Bracket, i, i + 1);
            self.bracket_depths.push(self.brace_flags.len());
        }
        self.chain = 0;
        i + 1
    }

    fn close_bracket(&mut self, i: usize) -> usize {
        while let Some(&d) = self.bracket_depths.last() {
            if d > self.brace_flags.len() {
                self.bracket_depths.pop();
            } else {
                break;
            }
        }
        if let Some(&d) = self.bracket_depths.last() {
            if d == self.brace_flags.len() {
                self.bracket_depths.pop();
                self.add(TokenKind::Bracket, i, i + 1);
                self.chain = 2;
                return i + 1;
            }
        }
        self.chain = 0;
        i + 1
    }

    // ----- Commands -----

    fn backslash(&mut self, i: usize) -> usize {
        let (u, n) = (self.u, self.n);
        if i + 1 >= n {
            let k = self.command_kind();
            self.add(k, i, i + 1);
            self.chain = 0;
            return i + 1;
        }
        let d = u[i + 1];
        if is_command_letter(d) {
            let mut j = i + 2;
            while j < n && is_command_letter(u[j]) {
                j += 1;
            }
            return self.control_word(i, j);
        }
        if is_line_break(d) {
            let k = self.command_kind();
            self.add(k, i, i + 1);
            self.chain = 0;
            return i + 1;
        }
        let mut end = i + 2;
        if is_high_surrogate(d) && end < n && is_low_surrogate(u[end]) {
            end += 1;
        }
        match d {
            40 if self.math_mode == 0 => {
                // \(
                self.add(TokenKind::MathDelimiter, i, end);
                self.math_mode = 3;
                self.run_start = end;
                self.chain = 0;
                return end;
            }
            41 if self.math_mode == 3 => {
                // \)
                self.add(TokenKind::MathDelimiter, i, end);
                self.math_mode = 0;
                self.chain = 0;
                return end;
            }
            91 if self.math_mode == 0 => {
                // \[
                self.add(TokenKind::MathDelimiter, i, end);
                self.math_mode = 4;
                self.run_start = end;
                self.chain = 0;
                return end;
            }
            93 if self.math_mode == 4 => {
                // \]
                self.add(TokenKind::MathDelimiter, i, end);
                self.math_mode = 0;
                self.chain = 0;
                return end;
            }
            _ => {}
        }
        let k = self.command_kind();
        self.add(k, i, end);
        self.chain = if d == 92 { 1 } else { 0 }; // `\\[2pt]` takes an optional argument
        end
    }

    fn control_word(&mut self, i: usize, j: usize) -> usize {
        let (u, n) = (self.u, self.n);
        let mut buf = [0u8; 48];
        let (info, name_is_lstinline) = {
            let name = ascii_name(u, (i + 1)..j, &mut buf);
            (command_info(&name), name == "lstinline")
        };
        let Some(info) = info else {
            let k = self.command_kind();
            self.add(k, i, j);
            self.chain = 1;
            return j;
        };
        match info.special {
            Special::Begin => return self.begin_or_end(i, j, true),
            Special::End => return self.begin_or_end(i, j, false),
            Special::Verb => {
                if let Some(next) = self.inline_verbatim(i, j, name_is_lstinline) {
                    return next;
                }
            }
            Special::Url => {
                let k = self.command_kind();
                self.add(k, i, j);
                self.chain = 1;
                if let Some((open, close)) = self.raw_brace_group(j) {
                    self.pending_args.push(PendingArg { open, close, kind: ArgKind::Verbatim });
                }
                return j;
            }
            Special::None => {}
        }
        let mut end = j;
        if info.allows_star && end < n && u[end] == 42 {
            end += 1;
        }
        let kind = if info.is_structure { TokenKind::StructureCommand } else { self.command_kind() };
        self.add(kind, i, end);
        self.chain = 1;
        if info.takes_reference {
            if let Some((open, close)) = self.reference_group(end) {
                self.pending_args.push(PendingArg { open, close, kind: ArgKind::Reference });
            }
        }
        end
    }

    // ----- \begin / \end -----

    fn begin_or_end(&mut self, i: usize, j: usize, is_begin: bool) -> usize {
        let (u, n) = (self.u, self.n);
        let mut k = j;
        while k < n && is_hspace(u[k]) {
            k += 1;
        }
        let Some((name_range, parsed_end)) = environment_name(u, k) else {
            self.add(TokenKind::StructureCommand, i, j);
            self.chain = 1;
            return j;
        };
        let name = utf16_string(u, name_range.clone());
        let mut command_kind = TokenKind::StructureCommand;
        let mut enters_math = false;
        let mut leaves_math = false;
        if is_begin {
            if self.math_mode == 0 && is_math_environment(&name) {
                command_kind = TokenKind::MathDelimiter;
                enters_math = true;
            }
        } else if self.math_mode == 5 && self.math_env_name == name {
            command_kind = TokenKind::MathDelimiter;
            leaves_math = true;
        }
        self.add(command_kind, i, j);
        self.add(TokenKind::Brace, k, k + 1);
        if !name_range.is_empty() {
            self.add(TokenKind::EnvironmentName, name_range.start, name_range.end);
        }
        self.add(TokenKind::Brace, parsed_end - 1, parsed_end);
        if enters_math {
            self.math_mode = 5;
            self.math_env_name = name.clone();
            self.run_start = parsed_end;
        } else if leaves_math {
            self.math_mode = 0;
            self.math_env_name.clear();
        }
        self.chain = if is_begin { 2 } else { 0 };
        if is_begin && is_verbatim_environment(&name) {
            return self.verbatim_body(&name, parsed_end);
        }
        parsed_end
    }

    fn verbatim_body(&mut self, name: &str, p: usize) -> usize {
        let u = self.u;
        let opening = verbatim_opening(u, name, p);
        if let Some(options) = &opening.options {
            self.add(TokenKind::Bracket, options.start, options.start + 1);
            self.add(TokenKind::Bracket, options.end - 1, options.end);
        }
        if let Some(language) = &opening.language {
            self.add(TokenKind::Brace, language.start, language.start + 1);
            self.add(TokenKind::Brace, language.end - 1, language.end);
        }
        let end = verbatim_end(u, name, opening.body_start);
        if end > opening.body_start {
            self.add(TokenKind::Verbatim, opening.body_start, end);
        }
        self.chain = 0;
        end
    }

    // ----- \verb -----

    fn inline_verbatim(&mut self, i: usize, j: usize, is_lstinline: bool) -> Option<usize> {
        let (u, n) = (self.u, self.n);
        let mut k = j;
        if !is_lstinline && k < n && u[k] == 42 {
            k += 1; // \verb*
        }
        if k >= n {
            return None;
        }
        let delimiter = u[k];
        if is_tex_whitespace(delimiter) {
            return None;
        }
        if is_lstinline && (delimiter == 123 || delimiter == 91) {
            return None;
        }
        let mut m = k + 1;
        while m < n && u[m] != delimiter && !is_line_break(u[m]) {
            m += 1;
        }
        if m >= n || u[m] != delimiter {
            return None;
        }
        let kind = self.command_kind();
        self.add(kind, i, k);
        self.add(TokenKind::Verbatim, k, m + 1);
        self.chain = 0;
        Some(m + 1)
    }

    // ----- Look-ahead for special arguments -----

    /// `{...}` following (optional whitespace, then) an arbitrary run of `[...]` groups, on a best-effort basis:
    /// gives up at comments, blank lines and very long groups.
    fn reference_group(&self, start: usize) -> Option<(usize, usize)> {
        let (u, n) = (self.u, self.n);
        let mut j = start;
        let mut groups = 0;
        while groups < 4 {
            let mut newlines = 0;
            while j < n && is_tex_whitespace(u[j]) {
                if u[j] == 10 {
                    newlines += 1;
                }
                j += 1;
            }
            if newlines > 1 || j >= n {
                return None;
            }
            if u[j] == 91 {
                let close = self.matching(j, 91, 93, 600)?;
                j = close + 1;
                groups += 1;
                continue;
            }
            if u[j] == 123 {
                let close = self.matching(j, 123, 125, 3000)?;
                return Some((j, close));
            }
            return None;
        }
        None
    }

    /// Raw `{...}` group (URLs): braces counted, nothing else interpreted.
    fn raw_brace_group(&self, start: usize) -> Option<(usize, usize)> {
        let (u, n) = (self.u, self.n);
        let mut j = start;
        while j < n && is_hspace(u[j]) {
            j += 1;
        }
        if !(j < n && u[j] == 123) {
            return None;
        }
        let mut depth: i32 = 0;
        let mut k = j;
        let stop = n.min(j + 2000);
        while k < stop {
            let c = u[k];
            if c == 123 {
                depth += 1;
            } else if c == 125 {
                depth -= 1;
                if depth == 0 {
                    return Some((j, k));
                }
            } else if is_line_break(c) {
                // A URL group never contains a blank line.
                let mut t = k + 1;
                if c == 13 && t < n && u[t] == 10 {
                    t += 1;
                }
                while t < n && is_hspace(u[t]) {
                    t += 1;
                }
                if t >= n || is_line_break(u[t]) {
                    return None;
                }
            }
            k += 1;
        }
        None
    }

    /// Index of the delimiter closing the group opened at `open`; honours `\x` escapes and nested braces,
    /// gives up at an unescaped `%` or a blank line.
    fn matching(&self, open: usize, opener: u16, closer: u16, limit: usize) -> Option<usize> {
        let (u, n) = (self.u, self.n);
        let mut depth: i32 = 0;
        let mut braces: i32 = 0;
        let mut k = open;
        let stop = n.min(open + limit);
        while k < stop {
            let c = u[k];
            if c == 92 {
                k += 2;
                continue;
            }
            if c == 37 {
                return None;
            }
            if c == 123 && opener != 123 {
                braces += 1;
            } else if c == 125 && opener != 123 {
                braces -= 1;
            } else if c == opener {
                depth += 1;
            } else if c == closer && (opener == 123 || braces <= 0) {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            } else if c == 10 || c == 13 {
                let mut t = k + 1;
                if c == 13 && t < n && u[t] == 10 {
                    t += 1;
                }
                while t < n && is_hspace(u[t]) {
                    t += 1;
                }
                if t >= n || is_line_break(u[t]) {
                    return None;
                }
            }
            k += 1;
        }
        None
    }
}

#[cfg(test)]
#[path = "scanning/tests_tokenizer.rs"]
mod tests;

// Allocation-light scanning helpers over UTF-16 strings (port of EditorKit/TextScanning.swift).
// Most functions work on one line's text with line-relative offsets: LaTeX escapes never span lines.

export const BACKSLASH = 92, PERCENT = 37, LBRACE = 123, RBRACE = 125, LBRACKET = 91, RBRACKET = 93;
export const LPAREN = 40, RPAREN = 41, DOLLAR = 36, SPACE = 32, TAB = 9, STAR = 42, COMMA = 44;

export const isHSpace = (c: number) => c === 32 || c === 9;
export const isLineBreak = (c: number) => c === 10 || c === 13;
export const isWhitespace = (c: number) => c === 32 || c === 9 || c === 10 || c === 13;
export const isASCIILetter = (c: number) => (c >= 65 && c <= 90) || (c >= 97 && c <= 122);
export const isCommandLetter = (c: number) => isASCIILetter(c) || c === 64;
export const isDigit = (c: number) => c >= 48 && c <= 57;

const wordRe = /[\p{L}\p{N}\p{Mn}]/u;
/** Word-like character: letters, digits, combining marks, `@`, `_`. */
export function isWordChar(c: number): boolean {
  if (isASCIILetter(c) || isDigit(c) || c === 64 || c === 95) return true;
  if (c < 128) return false;
  if (c >= 0xd800 && c <= 0xdfff) return true;
  return wordRe.test(String.fromCharCode(c));
}

/** Characters before which auto-inserting a closer makes sense. */
export function allowsAutoCloseBefore(c: number): boolean {
  return isWhitespace(c) || c === RBRACE || c === RBRACKET || c === RPAREN || c === DOLLAR || c === COMMA ||
    c === 46 || c === 59 || c === 58 || c === PERCENT;
}

/** True when the character at `i` is escaped by an odd number of backslashes. */
export function isEscaped(s: string, i: number): boolean {
  let n = 0;
  for (let k = i - 1; k >= 0 && s.charCodeAt(k) === BACKSLASH; k--) n++;
  return (n & 1) === 1;
}

/** Offset of the first unescaped `%` in `s[from, to)`, or -1. */
export function commentStart(s: string, from = 0, to = s.length): number {
  for (let i = from; i < to; i++) if (s.charCodeAt(i) === PERCENT && !isEscaped(s, i)) return i;
  return -1;
}

export function leadingWhitespace(s: string): number {
  let i = 0;
  while (i < s.length && isHSpace(s.charCodeAt(i))) i++;
  return i;
}

export const isBlank = (s: string) => leadingWhitespace(s) === s.length;

/** If `head` (line text up to the caret) ends with `\begin{name}` plus argument groups, returns `name`. */
export function environmentOpenedAtEndOf(head: string): string | null {
  let end = head.length;
  while (end > 0 && isHSpace(head.charCodeAt(end - 1))) end--;
  if (end === 0) return null;
  for (let guard = 0; end > 0 && guard < 16; guard++) {
    const last = head.charCodeAt(end - 1);
    let open: number;
    if (last === RBRACKET) open = LBRACKET; else if (last === RBRACE) open = LBRACE; else break;
    let depth = 0, found = -1;
    for (let i = end - 1; i >= 0; i--) {
      const c = head.charCodeAt(i);
      if (c === last && !isEscaped(head, i)) depth++;
      else if (c === open && !isEscaped(head, i)) {
        depth--;
        if (depth === 0) { found = i; break; }
      }
    }
    if (found < 0) return null;
    if (open === LBRACE && found >= 6 && head.slice(found - 6, found) === "\\begin" && !isEscaped(head, found - 6)) {
      const name = head.slice(found + 1, end - 1).trim();
      if (!name || /[{}\\]/.test(name)) return null;
      if (commentStart(head, 0, found - 6) >= 0) return null;
      return name;
    }
    end = found;
    while (end > 0 && isHSpace(head.charCodeAt(end - 1))) end--;
  }
  return null;
}

/** True when an unescaped `%` precedes `offset` in the line `s`. */
export const isInComment = (s: string, offset: number) => commentStart(s, 0, offset) >= 0;

/** Counts uncommented `\begin{name}` and `\end{name}` in the whole text. */
export function environmentBalance(name: string, text: string): { begins: number; ends: number } {
  const count = (needle: string) => {
    let total = 0;
    for (let i = text.indexOf(needle); i >= 0; i = text.indexOf(needle, i + needle.length)) {
      if (isEscaped(text, i)) continue;
      const lineStart = text.lastIndexOf("\n", i - 1) + 1;
      if (commentStart(text, lineStart, i) >= 0) continue;
      total++;
    }
    return total;
  };
  return { begins: count(`\\begin{${name}}`), ends: count(`\\end{${name}}`) };
}

/** Name of the command owning an argument that starts at `position` (skips `*` and `[…]` backwards). */
export function commandNameEndingBefore(s: string, position: number, lower = 0): string | null {
  let k = position - 1;
  for (let guard = 0; k >= lower && guard < 8; guard++) {
    const c = s.charCodeAt(k);
    if (c === RBRACKET) {
      let d = 0, found = -1;
      for (let m = k; m >= lower; m--) {
        const ch = s.charCodeAt(m);
        if (ch === RBRACKET) d++;
        else if (ch === LBRACKET && --d === 0) { found = m; break; }
      }
      if (found < 0) return null;
      k = found - 1;
    } else if (c === STAR || isHSpace(c)) {
      k--;
    } else break;
  }
  let start = k;
  while (start >= lower && isCommandLetter(s.charCodeAt(start))) start--;
  if (start < lower || start >= k || s.charCodeAt(start) !== BACKSLASH || isEscaped(s, start)) return null;
  return s.slice(start + 1, k + 1);
}

/** The command whose brace argument contains `offset` within the line: (`ref`, content range). */
export function enclosingArgument(s: string, offset: number): { command: string; from: number; to: number } | null {
  let depth = 0, open = -1;
  for (let i = offset - 1; i >= 0; i--) {
    const c = s.charCodeAt(i);
    if (c === RBRACE && !isEscaped(s, i)) depth++;
    else if (c === LBRACE && !isEscaped(s, i)) {
      if (depth === 0) { open = i; break; }
      depth--;
    }
  }
  if (open < 0) return null;
  let close = s.length;
  depth = 0;
  for (let j = offset; j < s.length; j++) {
    const c = s.charCodeAt(j);
    if (c === LBRACE && !isEscaped(s, j)) depth++;
    else if (c === RBRACE && !isEscaped(s, j)) {
      if (depth === 0) { close = j; break; }
      depth--;
    }
  }
  const command = commandNameEndingBefore(s, open);
  return command ? { command, from: open + 1, to: close } : null;
}

/** The comma-separated item of `s[from, to)` containing `offset`, trimmed. */
export function listItem(s: string, offset: number, from: number, to: number): string | null {
  let a = offset, b = offset;
  while (a > from && s.charCodeAt(a - 1) !== COMMA) a--;
  while (b < to && s.charCodeAt(b) !== COMMA) b++;
  const item = s.slice(a, b).trim();
  return item || null;
}

/** If the caret directly follows `\` + at least one letter, the offset of that backslash, else -1. */
export function commandPrefixStart(s: string, caret: number): number {
  let i = caret;
  while (i > 0 && isCommandLetter(s.charCodeAt(i - 1))) i--;
  if (i < caret && i > 0 && s.charCodeAt(i - 1) === BACKSLASH && !isEscaped(s, i - 1)) return i - 1;
  return -1;
}

/** Caret inside an (unclosed so far) brace argument of a command: `\ref{eq:`, `\cite{a,b`, `\begin{ite`. */
export function isInCommandArgument(s: string, caret: number): boolean {
  for (let i = caret - 1; i >= 0; i--) {
    const c = s.charCodeAt(i);
    if (c === LBRACE && !isEscaped(s, i)) return commandNameEndingBefore(s, i) !== null;
    if (c === RBRACE || c === BACKSLASH || c === PERCENT || c === DOLLAR) return false;
  }
  return false;
}

/** Token at `offset` for diagnostics without a length: a `\command`, a word, or one character. */
export function tokenExtent(s: string, offset: number): [number, number] {
  const limit = s.length;
  if (offset >= limit) return [offset, offset];
  const c = s.charCodeAt(offset);
  const one = (i: number) => {
    const h = s.charCodeAt(i);
    return h >= 0xd800 && h <= 0xdbff && i + 1 < limit ? 2 : 1;
  };
  if (c === BACKSLASH) {
    let j = offset + 1;
    while (j < limit && isCommandLetter(s.charCodeAt(j))) j++;
    if (j === offset + 1 && j < limit) j += one(j);
    return [offset, Math.min(j, limit)];
  }
  if (isWordChar(c)) {
    let j = offset;
    while (j < limit && isWordChar(s.charCodeAt(j))) j++;
    return [offset, j];
  }
  return [offset, offset + one(offset)];
}

/** Detects the first line terminator of `text` (default `\n`). */
export function lineEnding(text: string): "\n" | "\r\n" | "\r" {
  const i = text.search(/[\r\n]/);
  if (i < 0 || text.charCodeAt(i) === 10) return "\n";
  return text.charCodeAt(i + 1) === 10 ? "\r\n" : "\r";
}

// LaTeX highlighting: an incremental CodeMirror StreamLanguage that mirrors the token kinds and rules of
// Sources/TeXCore/Syntax/TokenizerEngine.swift (commands, structure commands, environment names, math
// across lines, comments, magic comments, braces, argument brackets, special characters, verbatim bodies and
// reference arguments). CodeMirror caches the parser state per line, so a keystroke re-tokenizes only the
// edited region and what follows in the viewport — never the whole file.
import { HighlightStyle, StreamLanguage, syntaxHighlighting, type StringStream } from "@codemirror/language";
import { Tag } from "@lezer/highlight";

// MARK: Token kinds → highlight tags

const kinds = [
  "command", "structureCommand", "environmentName", "mathDelimiter", "math", "mathCommand", "comment",
  "magicComment", "brace", "bracket", "specialCharacter", "verbatim", "referenceArgument",
] as const;
export type TokenKind = (typeof kinds)[number];

const tagFor = Object.fromEntries(kinds.map((k) => [k, Tag.define()])) as Record<TokenKind, Tag>;

/** CSS class per kind (colours live in editor.css, from EditorKit/Theme.swift). */
const classFor: Record<TokenKind, string> = {
  command: "tx-cmd", structureCommand: "tx-struct", environmentName: "tx-env", mathDelimiter: "tx-mdelim",
  math: "tx-math", mathCommand: "tx-mcmd", comment: "tx-comment", magicComment: "tx-magic", brace: "tx-brace",
  bracket: "tx-bracket", specialCharacter: "tx-special", verbatim: "tx-verb", referenceArgument: "tx-ref",
};

// Opening and closing math delimiters get distinct token names (same colour) so the editor can tell from the
// syntax tree whether a position is inside math (`$` auto-pairing).
const tokenTable: Record<string, Tag> = { mathOpen: tagFor.mathDelimiter, mathClose: tagFor.mathDelimiter };
for (const k of kinds) tokenTable[k] = tagFor[k];

export const latexHighlightStyle = HighlightStyle.define(kinds.map((k) => ({ tag: tagFor[k], class: classFor[k] })));

// MARK: Static knowledge (LaTeXSyntax.swift)

const verbatimEnvironments = new Set([
  "verbatim", "verbatim*", "Verbatim", "Verbatim*", "BVerbatim", "BVerbatim*", "LVerbatim", "LVerbatim*",
  "lstlisting", "minted", "comment", "filecontents", "filecontents*", "spverbatim", "verbatimtab",
]);
const verbatimWithOptions = new Set([
  "Verbatim", "Verbatim*", "BVerbatim", "BVerbatim*", "LVerbatim", "LVerbatim*", "lstlisting", "minted",
]);
const mathEnvironments = new Set<string>();
for (const b of ["equation", "align", "alignat", "flalign", "gather", "multline", "eqnarray", "math", "displaymath", "dmath"]) {
  mathEnvironments.add(b);
  mathEnvironments.add(b + "*");
}

const enum Special { None, Begin, End, Verb, Url }
interface CommandInfo { special: Special; structure: boolean; star: boolean; reference: boolean }

const commandTable = new Map<string, CommandInfo>();
const info = (name: string) => {
  let i = commandTable.get(name);
  if (!i) commandTable.set(name, (i = { special: Special.None, structure: false, star: false, reference: false }));
  return i;
};
for (const n of ["label", "ref", "eqref", "cref", "Cref", "autoref", "pageref", "nameref", "vref", "Vref", "labelcref",
  "cpageref", "Cpageref", "input", "include", "subfile", "includegraphics", "includepdf", "usepackage", "RequirePackage",
  "documentclass", "LoadClass", "bibliography", "addbibresource", "bibliographystyle", "lstinputlisting", "bibitem",
  "includesvg"]) info(n).reference = true;
for (const n of ["begin", "end", "documentclass", "usepackage", "RequirePackage", "input", "include", "subfile",
  "maketitle", "tableofcontents", "appendix", "frontmatter", "mainmatter", "backmatter", "bibliography",
  "printbibliography"]) info(n).structure = true;
for (const n of ["part", "chapter", "section", "subsection", "subsubsection", "paragraph", "subparagraph"]) {
  info(n).structure = true;
  info(n).star = true;
}
info("begin").special = Special.Begin;
info("end").special = Special.End;
info("verb").special = Special.Verb;
info("Verb").special = Special.Verb;
info("lstinline").special = Special.Verb;
info("url").special = Special.Url;
info("href").special = Special.Url;
const citeInfo: CommandInfo = { special: Special.None, structure: false, star: false, reference: true };

function isCiteLike(name: string): boolean {
  if (name.length < 4) return false;
  const first = name.charCodeAt(0);
  if (first === 99 || first === 67) {
    const lower = name.toLowerCase();
    return lower.startsWith("cite") && lower !== "citestyle";
  }
  return name.charCodeAt(name.length - 1) === 101 && name.toLowerCase().endsWith("cite");
}
const commandInfo = (name: string) => commandTable.get(name) ?? (isCiteLike(name) ? citeInfo : undefined);

// MARK: Parser state

const enum MathMode { None, Dollar, Double, Paren, Bracket, Env }
const enum Arg { None, Reference, Verbatim }

interface TexState {
  math: MathMode;
  mathEnv: string;
  mathBraceDepth: number;
  /** 0 none, 1 after a control word (spaces allowed), 2 after an argument group (no spaces). */
  chain: number;
  braces: boolean[];
  brackets: number[];
  /** A reference/verbatim argument is expected at the next `{` (after optional `[…]` groups). */
  pend: Arg;
  pendBrackets: number;
  /** Inside a reference / raw verbatim argument. */
  arg: Arg;
  argDepth: number;
  /** Body of a verbatim environment (its name), or "". */
  verbEnv: string;
  /** Segments already decided by a same-line look-ahead: [end column, token name]. */
  queue: [number, string | null][];
}

const SPECIAL = /[\\%$\{\}\[\]&~^_#]/;

function stepQueue(stream: StringStream, s: TexState): string | null {
  const [end, style] = s.queue.shift()!;
  stream.pos = Math.max(end, stream.pos + 1);
  return style;
}

/** Matches `{name}` (same line, no `{ } \ %`) at `pos` after optional spaces; [braceStart, nameEnd] or null. */
function envNameAt(line: string, pos: number): [number, number] | null {
  let k = pos;
  while (k < line.length && (line.charCodeAt(k) === 32 || line.charCodeAt(k) === 9)) k++;
  if (line.charCodeAt(k) !== 123) return null;
  const stop = Math.min(line.length, k + 120);
  for (let j = k + 1; j < stop; j++) {
    const c = line.charCodeAt(j);
    if (c === 125) return [k, j];
    if (c === 123 || c === 92 || c === 37) return null;
  }
  return null;
}

function closingOnLine(line: string, open: number, closer: number, limit: number): number {
  let depth = 0;
  const stop = Math.min(line.length, open + limit);
  for (let k = open + 1; k < stop; k++) {
    const c = line.charCodeAt(k);
    if (c === 92) { k++; continue; }
    if (c === 123) depth++;
    else if (c === 125) {
      if (closer === 125 && depth === 0) return k;
      depth--;
    } else if (c === closer && depth <= 0) return k;
  }
  return -1;
}

function beginOrEnd(stream: StringStream, s: TexState, isBegin: boolean): string {
  const line = stream.string;
  const parsed = envNameAt(line, stream.pos);
  if (!parsed) {
    s.chain = 1;
    return "structureCommand";
  }
  const [open, close] = parsed;
  const name = line.slice(open + 1, close);
  let style = "structureCommand";
  if (isBegin) {
    if (s.math === MathMode.None && mathEnvironments.has(name)) {
      style = "mathOpen";
      s.math = MathMode.Env;
      s.mathEnv = name;
    }
  } else if (s.math === MathMode.Env && s.mathEnv === name) {
    style = "mathClose";
    s.math = MathMode.None;
    s.mathEnv = "";
  }
  if (open > stream.pos) s.queue.push([open, null]);
  s.queue.push([open + 1, "brace"]);
  if (close > open + 1) s.queue.push([close, "environmentName"]);
  s.queue.push([close + 1, "brace"]);
  s.chain = isBegin ? 2 : 0;
  if (isBegin && verbatimEnvironments.has(name)) {
    s.verbEnv = name;
    s.chain = 0;
    let p = close + 1;
    if (verbatimWithOptions.has(name)) {
      if (line.charCodeAt(p) === 91) {
        const end = closingOnLine(line, p, 93, 300);
        if (end > 0) {
          s.queue.push([p + 1, "bracket"]);
          if (end > p + 1) s.queue.push([end, null]);
          s.queue.push([end + 1, "bracket"]);
          p = end + 1;
        }
      }
      if (name === "minted") {
        let k = p;
        while (k < line.length && (line.charCodeAt(k) === 32 || line.charCodeAt(k) === 9)) k++;
        if (line.charCodeAt(k) === 123) {
          const end = closingOnLine(line, k, 125, 200);
          if (end > 0) {
            if (k > p) s.queue.push([k, null]);
            s.queue.push([k + 1, "brace"]);
            if (end > k + 1) s.queue.push([end, null]);
            s.queue.push([end + 1, "brace"]);
          }
        }
      }
    }
  }
  return style;
}

function backslash(stream: StringStream, s: TexState): string {
  const start = stream.pos;
  const line = stream.string;
  stream.next(); // the backslash
  const cmdStyle = s.math !== MathMode.None ? "mathCommand" : "command";
  if (stream.eol()) {
    s.chain = 0;
    return cmdStyle;
  }
  if (stream.eatWhile(/[A-Za-z@]/)) {
    const name = line.slice(start + 1, stream.pos);
    const ci = commandInfo(name);
    if (!ci) {
      s.chain = 1;
      return cmdStyle;
    }
    switch (ci.special) {
      case Special.Begin: return beginOrEnd(stream, s, true);
      case Special.End: return beginOrEnd(stream, s, false);
      case Special.Verb: {
        let k = stream.pos;
        if (name !== "lstinline" && line.charCodeAt(k) === 42) k++;
        if (k < line.length) {
          const d = line.charCodeAt(k);
          const ws = d === 32 || d === 9;
          if (!ws && !(name === "lstinline" && (d === 123 || d === 91))) {
            const m = line.indexOf(line[k]!, k + 1);
            if (m > 0) {
              stream.pos = k;
              s.queue.push([m + 1, "verbatim"]);
              s.chain = 0;
              return cmdStyle;
            }
          }
        }
        break;
      }
      case Special.Url:
        s.chain = 1;
        s.pend = Arg.Verbatim;
        s.pendBrackets = s.brackets.length;
        return cmdStyle;
      default:
        break;
    }
    if (ci.star) stream.eat("*");
    s.chain = 1;
    if (ci.reference) {
      s.pend = Arg.Reference;
      s.pendBrackets = s.brackets.length;
    }
    return ci.structure ? "structureCommand" : cmdStyle;
  }
  // Control symbol.
  const d = stream.next()!;
  const code = d.charCodeAt(0);
  if (code >= 0xd800 && code <= 0xdbff) stream.next();
  s.chain = 0;
  switch (d) {
    case "(":
      if (s.math === MathMode.None) { s.math = MathMode.Paren; return "mathOpen"; }
      break;
    case ")":
      if (s.math === MathMode.Paren) { s.math = MathMode.None; return "mathClose"; }
      break;
    case "[":
      if (s.math === MathMode.None) { s.math = MathMode.Bracket; return "mathOpen"; }
      break;
    case "]":
      if (s.math === MathMode.Bracket) { s.math = MathMode.None; return "mathClose"; }
      break;
    case "\\":
      s.chain = 1; // `\\[2pt]`
      break;
  }
  return cmdStyle;
}

function isMagicComment(line: string, start: number): boolean {
  for (let k = 0; k < start; k++) {
    const c = line.charCodeAt(k);
    if (c !== 32 && c !== 9) return false;
  }
  return /^%[ \t]*!(tex|bib)(?![A-Za-z])/i.test(line.slice(start));
}

function argument(stream: StringStream, s: TexState): string | null {
  const line = stream.string;
  const raw = s.arg === Arg.Verbatim;
  let k = stream.pos;
  while (k < line.length) {
    const c = line.charCodeAt(k);
    if (!raw && c === 92) { k += 2; continue; }
    if (!raw && c === 37) break;
    if (c === 123) s.argDepth++;
    else if (c === 125) {
      if (s.argDepth === 0) break;
      s.argDepth--;
    }
    k++;
  }
  k = Math.min(k, line.length);
  if (k > stream.pos) {
    stream.pos = k;
    return raw ? "verbatim" : "referenceArgument";
  }
  if (line.charCodeAt(k) === 125) {
    stream.next();
    s.arg = Arg.None;
    s.braces.pop();
    s.chain = 2;
    return "brace";
  }
  // `%` inside a reference argument: give up (it is ordinary text after all).
  s.arg = Arg.None;
  return null;
}

const parser = {
  name: "latex",
  startState(): TexState {
    return {
      math: MathMode.None, mathEnv: "", mathBraceDepth: 0, chain: 0, braces: [], brackets: [], pend: Arg.None,
      pendBrackets: 0, arg: Arg.None, argDepth: 0, verbEnv: "", queue: [],
    };
  },
  copyState(s: TexState): TexState {
    return { ...s, braces: s.braces.slice(), brackets: s.brackets.slice(), queue: s.queue.slice() };
  },
  blankLine(s: TexState) {
    if (s.verbEnv) return;
    s.chain = 0;
    s.brackets.length = 0;
    s.pend = Arg.None;
    s.arg = Arg.None;
    if (s.math === MathMode.Dollar || s.math === MathMode.Paren) s.math = MathMode.None;
  },
  token(stream: StringStream, s: TexState): string | null {
    if (s.queue.length) return stepQueue(stream, s);
    if (stream.sol() && s.chain === 2) s.chain = 0;

    if (s.verbEnv) {
      const end = stream.string.indexOf(`\\end{${s.verbEnv}}`, stream.pos);
      if (end !== stream.pos) {
        stream.pos = end < 0 ? stream.string.length : end;
        return "verbatim";
      }
      s.verbEnv = "";
    }
    if (s.arg !== Arg.None) {
      const style = argument(stream, s);
      if ((s.arg as Arg) !== Arg.None || style !== null) return style;
    }

    const ch = stream.peek()!;
    if (s.pend !== Arg.None && s.brackets.length <= s.pendBrackets && ch !== " " && ch !== "\t" && ch !== "[" &&
      ch !== "]" && ch !== "{") s.pend = Arg.None;

    const inMath = s.math !== MathMode.None;
    switch (ch) {
      case "\\":
        return backslash(stream, s);
      case "%": {
        const magic = isMagicComment(stream.string, stream.pos);
        stream.skipToEnd();
        s.chain = 0;
        return magic ? "magicComment" : "comment";
      }
      case "$": {
        stream.next();
        s.chain = 0;
        switch (s.math) {
          case MathMode.None:
            if (stream.eat("$")) { s.math = MathMode.Double; return "mathOpen"; }
            s.math = MathMode.Dollar;
            s.mathBraceDepth = s.braces.length;
            return "mathOpen";
          case MathMode.Dollar:
            if (s.braces.length <= s.mathBraceDepth) { s.math = MathMode.None; return "mathClose"; }
            return "mathDelimiter";
          case MathMode.Double:
            if (stream.eat("$")) { s.math = MathMode.None; return "mathClose"; }
            return "mathDelimiter";
          default:
            return "mathDelimiter";
        }
      }
      case "{":
        stream.next();
        if (s.pend !== Arg.None && s.brackets.length <= s.pendBrackets) {
          s.arg = s.pend;
          s.argDepth = 0;
          s.pend = Arg.None;
          s.braces.push(true);
          s.chain = 0;
          return "brace";
        }
        s.braces.push(s.chain !== 0);
        s.chain = 0;
        return "brace";
      case "}": {
        stream.next();
        const wasArgument = s.braces.pop() ?? false;
        s.chain = wasArgument ? 2 : 0;
        return "brace";
      }
      case "[":
        stream.next();
        if (s.chain !== 0) {
          s.brackets.push(s.braces.length);
          s.chain = 0;
          return "bracket";
        }
        s.chain = 0;
        return inMath ? "math" : null;
      case "]": {
        stream.next();
        while (s.brackets.length && s.brackets[s.brackets.length - 1]! > s.braces.length) s.brackets.pop();
        if (s.brackets.length && s.brackets[s.brackets.length - 1] === s.braces.length) {
          s.brackets.pop();
          s.chain = 2;
          return "bracket";
        }
        s.chain = 0;
        return inMath ? "math" : null;
      }
      case "&": case "~": case "^": case "_": case "#":
        stream.next();
        s.chain = 0;
        return "specialCharacter";
      case " ": case "\t":
        stream.eatSpace();
        if (s.chain === 2) s.chain = 0;
        return null;
    }
    s.chain = 0;
    stream.next();
    while (!stream.eol()) {
      const c = stream.peek()!;
      if (SPECIAL.test(c) || (!inMath && (c === " " || c === "\t"))) break;
      stream.next();
    }
    return inMath ? "math" : null;
  },
  languageData: { commentTokens: { line: "%" } },
  tokenTable,
};

export const latexLanguage = StreamLanguage.define<TexState>(parser);

/** Language + highlighting. */
export const latex = () => [latexLanguage, syntaxHighlighting(latexHighlightStyle)];

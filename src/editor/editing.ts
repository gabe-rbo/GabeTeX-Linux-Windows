// Editing behaviours (port of EditorKit/TextViewEditing.swift): auto-pairing with type-over, wrapping a
// selection, Enter (auto-indent, `\begin{env}` → `\end{env}` auto-close, `{|}` split), Backspace (empty pair,
// soft tabs), Tab / ⇧Tab (snippet fields, indent / outdent lines) and comment toggling.
import { EditorSelection, StateEffect, StateField, type ChangeSpec, type EditorState } from "@codemirror/state";
import { EditorView, type Command } from "@codemirror/view";
import { ensureSyntaxTree, getIndentUnit, indentUnit } from "@codemirror/language";
import {
  allowsAutoCloseBefore, commentStart, environmentBalance, environmentOpenedAtEndOf, isASCIILetter, isBlank,
  isDigit, isEscaped, isHSpace, isInComment, leadingWhitespace,
} from "./scan";
import { advanceSnippet, hasSnippetSession, retreatSnippet } from "./snippet";

const pairs: Record<string, string> = { "{": "}", "[": "]", "(": ")", $: "$" };
const closers = new Set(["}", "]", ")", "$"]);

export const unitOf = (state: EditorState) => state.facet(indentUnit);

// MARK: Auto-inserted closers ("live" only on the caret's line, after the caret)

const addCloser = StateEffect.define<number>();
const removeCloser = StateEffect.define<number>();

export const autoClosers = StateField.define<readonly number[]>({
  create: () => [],
  update(list, tr) {
    let next = list;
    if (tr.docChanged && next.length) next = next.map((p) => tr.changes.mapPos(p, 1));
    for (const e of tr.effects) {
      if (e.is(addCloser)) next = [...next, e.value];
      else if (e.is(removeCloser)) next = next.filter((p) => p !== e.value);
    }
    if (next.length && (tr.selection || tr.docChanged)) {
      const sel = tr.state.selection;
      if (sel.ranges.length !== 1 || !sel.main.empty) return [];
      const caret = sel.main.head;
      const line = tr.state.doc.lineAt(caret);
      next = next.filter((p) => p >= caret && p <= line.to);
    }
    return next;
  },
});

// MARK: Math detection (from the highlighter's syntax tree)

/** True when `pos` lies inside math according to the highlighter. */
export function isMathAt(state: EditorState, pos: number): boolean {
  const tree = ensureSyntaxTree(state, pos, 30);
  if (!tree) return false;
  const line = state.doc.lineAt(pos);
  let p = pos;
  for (let guard = 0; guard < 40 && p > line.from; guard++) {
    const node = tree.resolveInner(p, -1);
    if (node.from >= p || node.type.isTop) {
      // No token right before `p`: plain text (math runs are always tokens) or whitespace.
      const ch = line.text.charCodeAt(p - 1 - line.from);
      if (!isHSpace(ch)) return false;
      p--;
      continue;
    }
    switch (node.name) {
      case "math": case "mathCommand": case "mathOpen": return true;
      case "mathClose": case "command": case "structureCommand": case "comment": case "magicComment":
      case "verbatim": case "referenceArgument": case "environmentName": return false;
      default: p = node.from;
    }
  }
  return false;
}

// MARK: Typing

/** inputHandler: auto-pair / type-over / wrap selection for a single typed character. */
export const autoPairInput = EditorView.inputHandler.of((view, from, to, text) => {
  if (text.length !== 1 || view.composing) return false;
  const state = view.state;
  if (state.selection.ranges.length !== 1) return false;
  const sel = state.selection.main;
  if (sel.from !== from || sel.to !== to) return false;
  const line = state.doc.lineAt(from);
  const lineText = line.text;
  const col = from - line.from;

  if (closers.has(text) && sel.empty && state.doc.sliceString(from, from + 1) === text &&
    state.field(autoClosers).includes(from)) {
    view.dispatch({ selection: EditorSelection.cursor(from + 1), effects: removeCloser.of(from), userEvent: "input.type" });
    return true;
  }
  const closer = pairs[text];
  if (!closer) return false;
  if (isEscaped(lineText, col)) return false; // \{ \[ \( \$ are literal

  if (!sel.empty) {
    const selected = state.sliceDoc(from, to);
    if (/[\r\n]/.test(selected)) return false;
    view.dispatch({
      changes: { from, to, insert: text + selected + closer },
      selection: EditorSelection.range(from + 1, to + 1),
      userEvent: "input.type",
    });
    return true;
  }
  if (col < lineText.length && !allowsAutoCloseBefore(lineText.charCodeAt(col))) return false;
  if (text === "$") {
    if (isMathAt(state, from)) return false;
    if (col > 0) {
      const prev = lineText.charCodeAt(col - 1);
      if (isASCIILetter(prev) || isDigit(prev) || prev === 36) return false;
    }
  }
  if (isInComment(lineText, col)) return false;
  view.dispatch({
    changes: { from, insert: text + closer },
    selection: EditorSelection.cursor(from + 1),
    effects: addCloser.of(from + 1),
    userEvent: "input.type",
  });
  return true;
});

// MARK: Backspace

export const smartBackspace: Command = (view) => {
  const state = view.state;
  if (state.selection.ranges.length !== 1 || state.readOnly) return false;
  const sel = state.selection.main;
  if (!sel.empty || sel.head === 0) return false;
  const caret = sel.head;
  const line = state.doc.lineAt(caret);
  const col = caret - line.from;
  const before = line.text[col - 1];
  const after = line.text[col];
  if (before && after && pairs[before] === after && state.field(autoClosers).includes(caret) &&
    !isEscaped(line.text, col - 1)) {
    view.dispatch({ changes: { from: caret - 1, to: caret + 1 }, effects: removeCloser.of(caret), userEvent: "delete.backward" });
    return true;
  }
  // Soft tabs: in leading spaces, delete back to the previous indentation stop.
  const width = getIndentUnit(state);
  if (width > 1 && col > 0 && isBlank(line.text.slice(0, col)) && before === " ") {
    let count = col % width === 0 ? width : col % width;
    let k = 0;
    while (k < count && line.text[col - 1 - k] === " ") k++;
    count = Math.max(1, k);
    if (count > 1) {
      view.dispatch({ changes: { from: caret - count, to: caret }, userEvent: "delete.backward" });
      return true;
    }
  }
  return false;
};

// MARK: Enter

export const smartEnter: Command = (view) => {
  const state = view.state;
  if (state.readOnly || state.selection.ranges.length !== 1) return false;
  const sel = state.selection.main;
  const line = state.doc.lineAt(sel.from);
  const headLength = sel.from - line.from;
  const head = line.text.slice(0, headLength);
  const indentLength = Math.min(leadingWhitespace(line.text), headLength);
  const indent = line.text.slice(0, indentLength);
  const unit = unitOf(state);
  const tailTo = state.doc.lineAt(sel.to).number === line.number ? line.to : sel.to;
  const tail = state.sliceDoc(sel.to, tailTo);

  const env = environmentOpenedAtEndOf(head);
  if (env) {
    const { begins, ends } = environmentBalance(env, state.doc.toString());
    if (begins > ends) {
      const body = "\n" + indent + unit;
      view.dispatch({
        changes: { from: sel.from, to: tailTo, insert: body + tail.trim() + "\n" + indent + `\\end{${env}}` },
        selection: EditorSelection.cursor(sel.from + body.length),
        scrollIntoView: true,
        userEvent: "input",
      });
      return true;
    }
    return insert(view, "\n" + indent + unit);
  }

  // Between an opener and its closer: {|} → {\n  |\n}
  if (sel.empty && headLength > 0 && tail.length > 0) {
    const before = head[headLength - 1]!;
    if (before !== "$" && pairs[before] === tail[0] && !isEscaped(head, headLength - 1)) {
      const first = "\n" + indent + unit;
      view.dispatch({
        changes: { from: sel.from, insert: first + "\n" + indent },
        selection: EditorSelection.cursor(sel.from + first.length),
        scrollIntoView: true,
        userEvent: "input",
      });
      return true;
    }
  }

  // A whitespace-only line: don't leave trailing whitespace behind.
  if (sel.empty && headLength > 0 && indentLength === headLength && tail.length === 0) {
    view.dispatch({
      changes: { from: line.from, to: sel.from, insert: "\n" + indent },
      selection: EditorSelection.cursor(line.from + 1 + indent.length),
      scrollIntoView: true,
      userEvent: "input",
    });
    return true;
  }

  // Copy the indentation; one level deeper after an opening brace at the end of the line.
  let newIndent = indent;
  if (headLength > indentLength) {
    let k = headLength;
    while (k > 0 && isHSpace(head.charCodeAt(k - 1))) k--;
    if (k > 0 && head[k - 1] === "{" && !isEscaped(head, k - 1) && commentStart(head, 0, k) < 0) newIndent += unit;
  }
  return insert(view, "\n" + newIndent);
};

function insert(view: EditorView, text: string): boolean {
  view.dispatch(view.state.replaceSelection(text), { scrollIntoView: true, userEvent: "input" });
  return true;
}

// MARK: Tab / ⇧Tab

/** 1-based line numbers touched by the main selection (a selection ending at column 0 excludes that line). */
function selectedLines(state: EditorState): [number, number] {
  const sel = state.selection.main;
  const first = state.doc.lineAt(sel.from).number;
  let last = state.doc.lineAt(sel.to).number;
  if (!sel.empty && last > first && state.doc.line(last).from === sel.to) last--;
  return [first, last];
}

/** Indents (or outdents) every selected line by one unit. Blank lines are not indented. */
export function shiftLines(view: EditorView, outdent: boolean): boolean {
  const state = view.state;
  if (state.readOnly) return false;
  const [first, last] = selectedLines(state);
  const unit = unitOf(state);
  const width = getIndentUnit(state);
  const changes: ChangeSpec[] = [];
  for (let n = first; n <= last; n++) {
    const line = state.doc.line(n);
    if (outdent) {
      if (!line.length) continue;
      if (line.text[0] === "\t") changes.push({ from: line.from, to: line.from + 1 });
      else {
        let count = 0;
        while (count < width && line.text[count] === " ") count++;
        if (count) changes.push({ from: line.from, to: line.from + count });
      }
    } else if (!isBlank(line.text)) {
      changes.push({ from: line.from, insert: unit });
    }
  }
  if (!changes.length) return true;
  view.dispatch({ changes, userEvent: outdent ? "delete.dedent" : "input.indent" });
  return true;
}

/** Tab without completion: next snippet field, indent the selected lines, or insert spaces to the next stop. */
export const plainTab: Command = (view) => {
  if (hasSnippetSession(view.state)) return advanceSnippet(view);
  const state = view.state;
  if (state.readOnly) return false;
  const sel = state.selection.main;
  if (!sel.empty || state.selection.ranges.length > 1) return shiftLines(view, false);
  const unit = unitOf(state);
  if (unit === "\t") return insert(view, "\t");
  const line = state.doc.lineAt(sel.head);
  const width = Math.max(1, getIndentUnit(state));
  let column = 0;
  for (let i = 0; i < sel.head - line.from; i++) column += line.text[i] === "\t" ? width - (column % width) : 1;
  return insert(view, " ".repeat(width - (column % width)));
};

export const backTab: Command = (view) => (hasSnippetSession(view.state) ? retreatSnippet(view) : shiftLines(view, true));

// MARK: Comments

/** Toggles `% ` on the selected lines (uncomments when every non-blank line is commented). */
export const toggleComment: Command = (view) => {
  const state = view.state;
  if (state.readOnly) return false;
  const [first, last] = selectedLines(state);
  const lines = [];
  for (let n = first; n <= last; n++) {
    const line = state.doc.line(n);
    if (!isBlank(line.text)) lines.push(line);
  }
  const changes: ChangeSpec[] = [];
  if (!lines.length) {
    const line = state.doc.lineAt(state.selection.main.from);
    changes.push({ from: line.from + leadingWhitespace(line.text), insert: "% " });
  } else if (lines.every((l) => l.text[leadingWhitespace(l.text)] === "%")) {
    for (const l of lines) {
      const p = leadingWhitespace(l.text);
      changes.push({ from: l.from + p, to: l.from + p + (l.text[p + 1] === " " ? 2 : 1) });
    }
  } else {
    const column = Math.min(...lines.map((l) => leadingWhitespace(l.text)));
    for (const l of lines) changes.push({ from: l.from + column, insert: "% " });
  }
  view.dispatch({ changes, userEvent: "input.comment" });
  return true;
};

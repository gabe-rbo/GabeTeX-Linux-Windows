// Ctrl-click navigation (port of EditorKit/TextViewMouse.swift): an include argument opens the file, a
// `\ref`-family key jumps to its label, a `\cite` key is left alone, anything else is ordinary text → the caret
// moves there and "Show in PDF" (forward SyncTeX) fires. A consumed click never starts a selection.
import { EditorSelection, type EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { enclosingArgument, listItem } from "./scan";

const labelCommands = new Set([
  "ref", "eqref", "cref", "Cref", "autoref", "pageref", "nameref", "vref", "Vref", "cpageref", "Cpageref",
  "labelcref", "crefrange", "Crefrange", "hyperref", "subref", "label",
]);

const includeRe =
  /\\(input|include|subfile|includegraphics|includepdf|includesvg|lstinputlisting|bibliography|addbibresource|includestandalone|subimport|import|inputminted)\*?[ \t]*(?:\[[^\]\n]*\][ \t]*)?(?:\{([^{}\n]*)\}[ \t]*)?\{([^{}\n]*)\}/g;

/** The include target whose command or argument contains `pos` (scans ±2 lines). */
export function includeTargetAt(state: EditorState, pos: number): string | null {
  const doc = state.doc;
  const line = doc.lineAt(pos);
  const first = doc.line(Math.max(1, line.number - 2));
  const last = doc.line(Math.min(doc.lines, line.number + 2));
  const text = doc.sliceString(first.from, last.to);
  const at = pos - first.from;
  includeRe.lastIndex = 0;
  for (let m = includeRe.exec(text); m; m = includeRe.exec(text)) {
    if (at < m.index || at > m.index + m[0].length) continue;
    // Skip commented-out commands.
    const lineStart = text.lastIndexOf("\n", m.index - 1) + 1;
    if (/(^|[^\\])%/.test(text.slice(lineStart, m.index))) continue;
    const cmd = m[1]!;
    let arg = m[3]!.trim();
    if (m[2] !== undefined) {
      // \import{dir}{file}, \subimport{dir}{file}, \inputminted{lang}{file}
      if (cmd === "import" || cmd === "subimport") arg = m[2].trim() + arg;
      else if (cmd !== "inputminted") continue;
    }
    if (cmd === "bibliography" || cmd === "addbibresource") {
      // Pick the comma-separated item under the click.
      const argStart = m.index + m[0].lastIndexOf("{") + 1;
      const local = Math.max(argStart, Math.min(at, argStart + m[3]!.length));
      const item = listItem(text, local, argStart, argStart + m[3]!.length);
      if (item) arg = item;
    }
    return arg || null;
  }
  return null;
}

/** The label key under `pos` inside `\ref{…}` / `\eqref{…}` / `\cref{a,b}` … */
export function labelKeyAt(state: EditorState, pos: number): string | null {
  const line = state.doc.lineAt(pos);
  const arg = enclosingArgument(line.text, pos - line.from);
  if (!arg || !labelCommands.has(arg.command)) return null;
  return listItem(line.text, pos - line.from, arg.from, arg.to);
}

function isCitationAt(state: EditorState, pos: number): boolean {
  const line = state.doc.lineAt(pos);
  const arg = enclosingArgument(line.text, pos - line.from);
  return !!arg && /cite/i.test(arg.command);
}

export interface ClickHandlers {
  openInclude(target: string): void;
  jumpToLabel(label: string): void;
  forwardSync(pos: number): void;
}

/** Character index under the pointer, only when the pointer is over text. */
function charAt(view: EditorView, x: number, y: number): number | null {
  const pos = view.posAtCoords({ x, y });
  if (pos === null) return null;
  const line = view.state.doc.lineAt(pos);
  if (line.length === 0) return null;
  // posAtCoords returns the nearest boundary: take the character on the pointer's side.
  const c = view.coordsAtPos(pos, 1);
  let index = pos;
  if (c && x < c.left && pos > line.from) index = pos - 1;
  if (index >= line.to) index = line.to - 1;
  const box = view.coordsAtPos(index, 1);
  if (box && (y < box.top - 2 || y > box.bottom + 2)) return null;
  return index;
}

export function ctrlClick(handlers: () => ClickHandlers) {
  let swallow = false;
  return EditorView.domEventHandlers({
    mousedown(event, view) {
      swallow = false;
      const mod = event.ctrlKey || event.metaKey;
      if (!mod || event.shiftKey || event.altKey || event.button !== 0 || event.detail !== 1) return false;
      const index = charAt(view, event.clientX, event.clientY);
      if (index === null) return false;
      const h = handlers();
      const target = includeTargetAt(view.state, index);
      if (target) {
        h.openInclude(target);
      } else {
        const key = labelKeyAt(view.state, index);
        if (key) h.jumpToLabel(key);
        else if (isCitationAt(view.state, index)) return false;
        else {
          view.dispatch({ selection: EditorSelection.cursor(index), userEvent: "select.pointer" });
          h.forwardSync(index);
        }
      }
      swallow = true;
      event.preventDefault();
      return true;
    },
    mousemove(event) {
      if (swallow && event.buttons) { event.preventDefault(); return true; }
      return false;
    },
    mouseup() {
      const was = swallow;
      swallow = false;
      return was;
    },
  });
}

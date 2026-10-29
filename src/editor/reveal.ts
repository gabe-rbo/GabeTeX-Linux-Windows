// Reveal (port of EditorKit/RevealSupport.swift + RevealFlash.swift): caret to a position, vertical
// centering, and a soft accent-coloured rounded highlight that holds 250 ms then fades for 950 ms (shown
// 800 ms without animation under Reduce Motion). Pure decoration: never touches text, undo or dirty state.
// One flash at a time; any edit cancels it.
import { EditorSelection, StateEffect, StateField, type EditorState } from "@codemirror/state";
import { Decoration, EditorView, type DecorationSet } from "@codemirror/view";

const setFlash = StateEffect.define<{ from: number; to: number; id: number } | null>();
const flashMark = Decoration.mark({ class: "gx-flash" });

export const flashField = StateField.define<{ deco: DecorationSet; id: number }>({
  create: () => ({ deco: Decoration.none, id: 0 }),
  update(value, tr) {
    for (const e of tr.effects) {
      if (!e.is(setFlash)) continue;
      if (!e.value) return value.id && value.deco.size ? { deco: Decoration.none, id: 0 } : value;
      return { deco: Decoration.set([flashMark.range(e.value.from, e.value.to)]), id: e.value.id };
    }
    if (tr.docChanged && value.deco.size) return { deco: Decoration.none, id: 0 };
    return value;
  },
  provide: (f) => EditorView.decorations.from(f, (v) => v.deco),
});

/** `[from, to)` without leading/trailing whitespace; null when nothing is left. */
function trimmed(state: EditorState, from: number, to: number): [number, number] | null {
  const text = state.sliceDoc(from, to);
  const lead = text.length - text.trimStart().length;
  const body = text.trim();
  return body ? [from + lead, from + lead + body.length] : null;
}

let serial = 0;

const reduceMotion = () =>
  typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Caret to `line`/`column` (1-based; column in UTF-16 units), centred; flashes the line's content. */
export function revealLine(view: EditorView, line: number, column?: number, flash = true) {
  const doc = view.state.doc;
  const l = doc.line(Math.max(1, Math.min(line, doc.lines)));
  const offset = Math.min(l.from + Math.max(0, (column ?? 1) - 1), l.to);
  revealRange(view, offset, flash ? trimmed(view.state, l.from, l.to) : null);
}

/** Caret to `caret`, centred, with an optional flash of `[from, to)`. */
export function revealRange(view: EditorView, caret: number, flash: [number, number] | null) {
  const id = ++serial;
  const effects: StateEffect<unknown>[] = [
    EditorView.scrollIntoView(caret, { y: "center", x: "nearest" }),
    setFlash.of(flash && flash[1] > flash[0] ? { from: flash[0], to: flash[1], id } : null),
  ];
  view.dispatch({ selection: EditorSelection.cursor(caret), effects, userEvent: "select.reveal" });
  if (flash) {
    view.dom.classList.toggle("gx-reduce-motion", reduceMotion());
    setTimeout(() => {
      if (view.dom.isConnected && view.state.field(flashField, false)?.id === id) view.dispatch({ effects: setFlash.of(null) });
    }, reduceMotion() ? 800 : 1250);
  }
}

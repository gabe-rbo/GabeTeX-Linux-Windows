// Snippets with tab stops (port of TeXCore/Completion/Snippet.swift and EditorKit/SnippetSession.swift).
// Syntax: `${1:default}` (defaults may contain balanced braces and nested placeholders), `${1}`, `$1`, `$0`.
// A `$` after a backslash (`\$`) or not followed by a digit / `{digit` is literal. Later occurrences of an index
// are mirrors: they follow the first occurrence while it is edited (in the same undo step).
import { Annotation, EditorSelection, EditorState, StateEffect, StateField, type Transaction, type TransactionSpec } from "@codemirror/state";
import { Decoration, EditorView, type DecorationSet } from "@codemirror/view";

export interface Range { from: number; to: number }
export interface ParsedSnippet { text: string; stops: Range[]; mirrors: Range[][]; final: number | null }

const isDigit = (c: number) => c >= 48 && c <= 57;

class SnippetParser {
  out = "";
  occurrences = new Map<number, Range[]>();
  final: number | null = null;
  constructor(readonly u: string, readonly defaults: Map<number, string>) {}

  sequence(start: number, end: number) {
    const u = this.u;
    let i = start;
    while (i < end) {
      const c = u.charCodeAt(i);
      if (c === 36 && !(i > 0 && u.charCodeAt(i - 1) === 92)) {
        const next = this.placeholder(i, end);
        if (next !== null) { i = next; continue; }
      }
      this.out += u[i];
      i++;
    }
  }

  placeholder(i: number, end: number): number | null {
    const u = this.u;
    if (i + 1 >= end) return null;
    const d = u.charCodeAt(i + 1);
    let k: number, index = 0;
    if (isDigit(d)) {
      for (k = i + 1; k < end && isDigit(u.charCodeAt(k)); k++) index = index * 10 + u.charCodeAt(k) - 48;
      this.insert(index, null);
      return k;
    }
    if (d !== 123) return null;
    k = i + 2;
    if (k >= end || !isDigit(u.charCodeAt(k))) return null;
    for (; k < end && isDigit(u.charCodeAt(k)); k++) index = index * 10 + u.charCodeAt(k) - 48;
    if (k >= end) return null;
    if (u.charCodeAt(k) === 125) { this.insert(index, null); return k + 1; }
    if (u.charCodeAt(k) !== 58) return null;
    const bodyStart = k + 1;
    let depth = 1, m = bodyStart;
    for (; m < end; m++) {
      const c = u.charCodeAt(m);
      if (c === 92) { m++; continue; }
      if (c === 123) depth++;
      else if (c === 125 && --depth === 0) break;
    }
    if (m >= end || u.charCodeAt(m) !== 125) return null;
    this.insert(index, [bodyStart, m]);
    return m + 1;
  }

  insert(index: number, body: [number, number] | null) {
    const location = this.out.length;
    if (body) {
      this.sequence(body[0], body[1]);
      if (!this.defaults.has(index)) this.defaults.set(index, this.out.slice(location));
    } else {
      const known = this.defaults.get(index);
      if (known !== undefined) this.out += known;
    }
    if (index === 0) {
      if (this.final === null) this.final = location;
    } else {
      const list = this.occurrences.get(index) ?? [];
      list.push({ from: location, to: this.out.length });
      this.occurrences.set(index, list);
    }
  }
}

export function parseSnippet(source: string): ParsedSnippet {
  const first = new SnippetParser(source, new Map());
  first.sequence(0, source.length);
  const p = new SnippetParser(source, first.defaults);
  p.sequence(0, source.length);
  const stops: Range[] = [], mirrors: Range[][] = [];
  for (const index of [...p.occurrences.keys()].sort((a, b) => a - b)) {
    const ranges = p.occurrences.get(index)!;
    stops.push(ranges[0]!);
    mirrors.push(ranges.slice(1));
  }
  return { text: p.out, stops, mirrors, final: p.final };
}

/** Applies indentation: `\n` → `\n` + `indent`, `\t` → `unit`; remaps stops/mirrors/final. */
export function expandSnippet(s: ParsedSnippet, indent: string, unit: string): ParsedSnippet {
  const src = s.text;
  const map = new Array<number>(src.length + 1);
  let out = "";
  for (let i = 0; i < src.length; i++) {
    map[i] = out.length;
    const c = src[i]!;
    out += c === "\n" ? "\n" + indent : c === "\t" ? unit : c;
  }
  map[src.length] = out.length;
  const m = (o: number) => map[Math.max(0, Math.min(o, src.length))]!;
  const mr = (r: Range): Range => ({ from: m(r.from), to: Math.max(m(r.from), m(r.to)) });
  return {
    text: out,
    stops: s.stops.map(mr),
    mirrors: s.mirrors.map((list) => list.map(mr)),
    final: s.final === null ? out.length : m(s.final),
  };
}

// MARK: Session

interface Session { stops: Range[]; mirrors: Range[][]; index: number; final: number; span: Range }

export const startSnippetSession = StateEffect.define<Session>();
export const endSnippetSession = StateEffect.define<null>();
const setStopIndex = StateEffect.define<number>();
const mirrorSync = Annotation.define<boolean>();

const mapRange = (r: Range, tr: Transaction, grows: boolean): Range => {
  const from = tr.changes.mapPos(r.from, grows ? -1 : 1);
  const to = tr.changes.mapPos(r.to, grows ? 1 : -1);
  return { from, to: Math.max(from, to) };
};

export const snippetField = StateField.define<Session | null>({
  create: () => null,
  update(session, tr) {
    for (const e of tr.effects) {
      if (e.is(startSnippetSession)) return e.value;
      if (e.is(endSnippetSession)) return null;
    }
    if (!session) return null;
    if (tr.isUserEvent("undo") || tr.isUserEvent("redo")) return null;
    let next = session;
    if (tr.docChanged) {
      // An edit reaching outside the snippet ends the session.
      let outside = false;
      tr.changes.iterChangedRanges((fromA, toA) => {
        if (fromA < session.span.from || toA > session.span.to) {
          if (!(toA <= session.span.from || fromA >= session.span.to)) outside = true;
        }
      });
      if (outside) return null;
      next = {
        stops: session.stops.map((r, i) => mapRange(r, tr, i === session.index)),
        mirrors: session.mirrors.map((list, i) => list.map((r) => mapRange(r, tr, i === session.index))),
        index: session.index,
        final: tr.changes.mapPos(session.final, 1),
        span: mapRange(session.span, tr, true),
      };
    }
    for (const e of tr.effects) if (e.is(setStopIndex)) next = { ...next, index: e.value };
    if (tr.selection) {
      const sel = tr.state.selection.main;
      if (sel.from < next.span.from || sel.to > next.span.to) return null;
    }
    return next;
  },
  provide: (f) => EditorView.decorations.from(f, (s) => (s ? placeholderDecorations(s) : Decoration.none)),
});

const placeholderMark = Decoration.mark({ class: "gx-snippet-stop" });

function placeholderDecorations(s: Session): DecorationSet {
  const ranges = s.stops
    .filter((r, i) => i !== s.index && r.to > r.from)
    .map((r) => placeholderMark.range(r.from, r.to));
  return Decoration.set(ranges, true);
}

/** Transaction filter keeping mirrors equal to their stop. */
function mirrorTransactionFilter(tr: Transaction): TransactionSpec | readonly TransactionSpec[] {
  if (!tr.docChanged || tr.annotation(mirrorSync) || tr.isUserEvent("undo") || tr.isUserEvent("redo")) return tr;
  const session = tr.startState.field(snippetField, false);
  if (!session) return tr;
  const mirrors = session.mirrors[session.index];
  const stop = session.stops[session.index];
  if (!mirrors || !mirrors.length || !stop) return tr;
  const newStop = mapRange(stop, tr, true);
  const value = tr.newDoc.sliceString(newStop.from, newStop.to);
  const changes: { from: number; to: number; insert: string }[] = [];
  for (const m of mirrors) {
    const r = mapRange(m, tr, false);
    if (r.to > newStop.from && r.from < newStop.to) continue;
    if (tr.newDoc.sliceString(r.from, r.to) !== value) changes.push({ from: r.from, to: r.to, insert: value });
  }
  if (!changes.length) return tr;
  return [tr, { changes, sequential: true, annotations: mirrorSync.of(true) }];
}

/** The snippet session field plus mirror syncing. */
export const snippets = () => [snippetField, EditorState.transactionFilter.of(mirrorTransactionFilter)];

// MARK: Commands

export function insertSnippetAt(view: EditorView, source: string, from: number, to: number, indentUnit: string) {
  const state = view.state;
  const line = state.doc.lineAt(from);
  const lead = /^[ \t]*/.exec(line.text)![0];
  const indent = lead.slice(0, Math.max(0, from - line.from));
  const expanded = expandSnippet(parseSnippet(source), indent, indentUnit);
  const first = expanded.stops[0];
  const selection = first
    ? EditorSelection.range(from + first.from, from + first.to)
    : EditorSelection.cursor(from + (expanded.final ?? expanded.text.length));
  const shift = (r: Range): Range => ({ from: from + r.from, to: from + r.to });
  const effects = expanded.stops.length
    ? startSnippetSession.of({
      stops: expanded.stops.map(shift),
      mirrors: expanded.mirrors.map((l) => l.map(shift)),
      index: 0,
      final: from + (expanded.final ?? expanded.text.length),
      span: { from, to: from + expanded.text.length },
    })
    : endSnippetSession.of(null);
  view.dispatch({
    changes: { from, to, insert: expanded.text },
    selection,
    effects,
    scrollIntoView: true,
    userEvent: "input.complete",
  });
}

export const hasSnippetSession = (state: EditorState) => state.field(snippetField, false) != null;
export const snippetHasMirrors = (state: EditorState) =>
  state.field(snippetField, false)?.mirrors.some((l) => l.length > 0) ?? false;

/** Tab inside a snippet: next stop, or the final position (ending the session). */
export function advanceSnippet(view: EditorView): boolean {
  const s = view.state.field(snippetField, false);
  if (!s) return false;
  const next = s.index + 1;
  if (next < s.stops.length) {
    const r = s.stops[next]!;
    view.dispatch({ selection: EditorSelection.range(r.from, r.to), effects: setStopIndex.of(next), scrollIntoView: true });
  } else {
    view.dispatch({ selection: EditorSelection.cursor(s.final), effects: endSnippetSession.of(null), scrollIntoView: true });
  }
  return true;
}

/** ⇧Tab inside a snippet: previous stop. */
export function retreatSnippet(view: EditorView): boolean {
  const s = view.state.field(snippetField, false);
  if (!s) return false;
  const prev = Math.max(0, s.index - 1);
  const r = s.stops[prev]!;
  view.dispatch({ selection: EditorSelection.range(r.from, r.to), effects: setStopIndex.of(prev), scrollIntoView: true });
  return true;
}

export function cancelSnippet(view: EditorView): boolean {
  if (!hasSnippetSession(view.state)) return false;
  view.dispatch({ effects: endSnippetSession.of(null) });
  return true;
}

// Inline diagnostics (port of EditorKit/DiagnosticsRenderer.swift, DiagnosticPopover.swift and the marker
// column of EditorGutterView.swift): squiggles that follow edits, one marker per line (maximum severity) in
// the gutter, a hover popover after 450 ms, and a click-on-marker popover listing the line's messages.
import { RangeSet, RangeSetBuilder, StateEffect, StateField, type EditorState, type Text } from "@codemirror/state";
import { Decoration, EditorView, GutterMarker, gutter, hoverTooltip, type DecorationSet } from "@codemirror/view";
import type { Diagnostic, Severity } from "../ipc";
import { isHSpace, tokenExtent } from "./scan";
import { t } from "./i18n";

interface Anchor { from: number; to: number; diag: Diagnostic }
interface DiagState { anchors: Anchor[]; input: Diagnostic[]; deco: DecorationSet; markers: RangeSet<GutterMarker> }

export const setDiagnosticsEffect = StateEffect.define<Diagnostic[]>();

const rank: Record<Severity, number> = { info: 0, warning: 1, error: 2 };

/** The range a diagnostic underlines (DiagnosticRangeMapper): column+length → that span; column → the token
 *  there; no column → the line without surrounding whitespace. Null when the line does not exist. */
export function diagnosticRange(d: Diagnostic, doc: Text): [number, number] | null {
  if (!(d.line >= 1 && d.line <= doc.lines)) return null;
  const line = doc.line(d.line);
  const s = line.text;
  if (!d.column || d.column < 1) {
    let a = 0, b = s.length;
    while (a < b && isHSpace(s.charCodeAt(a))) a++;
    while (b > a && isHSpace(s.charCodeAt(b - 1))) b--;
    return [line.from + a, line.from + b];
  }
  const start = Math.min(d.column - 1, s.length);
  if (d.length && d.length > 0) {
    const end = Math.min(line.from + start + d.length, doc.length);
    return [line.from + start, Math.max(line.from + start, end)];
  }
  if (start >= s.length) return s.length ? [line.to - 1, line.to] : [line.from, line.from];
  const [a, b] = tokenExtent(s, start);
  return [line.from + a, line.from + b];
}

const sameReport = (a: Diagnostic, b: Diagnostic) =>
  a.line === b.line && a.column === b.column && a.length === b.length && a.message === b.message &&
  a.severity === b.severity && a.source === b.source;

/** Gutter kind: the backend reports bad boxes as "info"; the Mac gutter gives them their own symbol and rank. */
type MarkerKind = Severity | "badbox";
const BADBOX = /^(Overfull|Underfull) \\[hv]box/;
const markerKind = (d: Diagnostic): MarkerKind => (d.severity === "info" && BADBOX.test(d.message) ? "badbox" : d.severity);
const kindRank: Record<MarkerKind, number> = { info: 0, badbox: 1, warning: 2, error: 3 };

class SeverityMarker extends GutterMarker {
  constructor(readonly severity: MarkerKind) { super(); }
  override eq(other: GutterMarker) { return other instanceof SeverityMarker && other.severity === this.severity; }
  override toDOM() {
    const el = document.createElement("span");
    el.className = `gx-marker gx-marker-${this.severity}`;
    el.innerHTML = severityIcon(this.severity);
    return el;
  }
}
const markerFor: Record<MarkerKind, SeverityMarker> = {
  info: new SeverityMarker("info"), badbox: new SeverityMarker("badbox"), warning: new SeverityMarker("warning"), error: new SeverityMarker("error"),
};

const underline: Record<Severity, Decoration> = {
  info: Decoration.mark({ class: "gx-diag gx-diag-info" }),
  warning: Decoration.mark({ class: "gx-diag gx-diag-warning" }),
  error: Decoration.mark({ class: "gx-diag gx-diag-error" }),
};

function build(anchors: Anchor[], doc: Text): Pick<DiagState, "deco" | "markers"> {
  anchors.sort((a, b) => a.from - b.from || rank[a.diag.severity] - rank[b.diag.severity]);
  const deco = Decoration.set(
    anchors.filter((a) => a.to > a.from).map((a) => underline[a.diag.severity].range(a.from, a.to)), true);
  const perLine = new Map<number, MarkerKind>();
  for (const a of anchors) {
    const n = doc.lineAt(Math.min(a.from, doc.length)).number;
    const s = perLine.get(n);
    const k = markerKind(a.diag);
    if (!s || kindRank[k] > kindRank[s]) perLine.set(n, k);
  }
  const b = new RangeSetBuilder<GutterMarker>();
  for (const n of [...perLine.keys()].sort((x, y) => x - y)) {
    const pos = doc.line(n).from;
    b.add(pos, pos, markerFor[perLine.get(n)!]);
  }
  return { deco, markers: b.finish() };
}

export const diagnosticsField = StateField.define<DiagState>({
  create: () => ({ anchors: [], input: [], deco: Decoration.none, markers: RangeSet.empty }),
  update(value, tr) {
    let next = value;
    if (tr.docChanged && value.anchors.length) {
      const anchors = value.anchors.map((a) => {
        const from = tr.changes.mapPos(a.from, -1);
        return { from, to: Math.max(from, tr.changes.mapPos(a.to, 1)), diag: a.diag };
      });
      next = { anchors, input: value.input, deco: value.deco.map(tr.changes), markers: value.markers.map(tr.changes) };
    }
    for (const e of tr.effects) {
      if (!e.is(setDiagnosticsEffect)) continue;
      const doc = tr.state.doc;
      // Keep the edit-tracked anchor of a diagnostic its producer reports at the same place again.
      const old = next.anchors.slice();
      const anchors: Anchor[] = [];
      for (const d of e.value) {
        const i = old.findIndex((a) => sameReport(a.diag, d));
        if (i >= 0) {
          const a = old.splice(i, 1)[0]!;
          anchors.push({ from: a.from, to: a.to, diag: d });
          continue;
        }
        const r = diagnosticRange(d, doc);
        if (r) anchors.push({ from: r[0], to: r[1], diag: d });
      }
      next = { anchors, input: e.value, ...build(anchors, doc) };
    }
    return next;
  },
  provide: (f) => EditorView.decorations.from(f, (v) => v.deco),
});

/** Diagnostics whose range contains `pos`. */
export function diagnosticsAt(state: EditorState, pos: number, side: number): Anchor[] {
  return state.field(diagnosticsField).anchors.filter((a) =>
    a.to > a.from && (side < 0 ? pos > a.from && pos <= a.to : pos >= a.from && pos < a.to));
}

/** Diagnostics anchored on the line starting at `lineFrom`, most severe first. */
export function diagnosticsOnLine(state: EditorState, lineFrom: number): Diagnostic[] {
  const line = state.doc.lineAt(lineFrom);
  return state.field(diagnosticsField).anchors
    .filter((a) => a.from >= line.from && a.from <= line.to)
    .map((a) => a.diag)
    .sort((a, b) => rank[b.severity] - rank[a.severity]);
}

// MARK: Icons (SF Symbols look-alikes)

export function severityIcon(s: MarkerKind): string {
  switch (s) {
    case "badbox": // rectangle.compress.vertical
      return `<svg viewBox="0 0 16 16" width="1em" height="1em" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 8h11"/><path d="M8 1.5v3.8M6.3 3.6 8 5.3l1.7-1.7M8 14.5v-3.8M6.3 12.4 8 10.7l1.7 1.7"/></svg>`;
    case "error":
      return `<svg viewBox="0 0 16 16" width="1em" height="1em" aria-hidden="true"><circle cx="8" cy="8" r="7" fill="currentColor"/><path d="M5.5 5.5l5 5m0-5l-5 5" stroke="var(--gx-icon-fg)" stroke-width="1.6" stroke-linecap="round"/></svg>`;
    case "warning":
      return `<svg viewBox="0 0 16 16" width="1em" height="1em" aria-hidden="true"><path d="M8 1.6c.5 0 .9.25 1.15.7l5.6 10.1c.5.9-.15 1.95-1.15 1.95H2.4c-1 0-1.65-1.05-1.15-1.95l5.6-10.1C7.1 1.85 7.5 1.6 8 1.6z" fill="currentColor"/><path d="M8 5.4v4" stroke="var(--gx-icon-fg)" stroke-width="1.6" stroke-linecap="round"/><circle cx="8" cy="11.7" r=".95" fill="var(--gx-icon-fg)"/></svg>`;
    default:
      return `<svg viewBox="0 0 16 16" width="1em" height="1em" aria-hidden="true"><circle cx="8" cy="8" r="7" fill="currentColor"/><circle cx="8" cy="4.9" r="1" fill="var(--gx-icon-fg)"/><path d="M8 7.3v4.4" stroke="var(--gx-icon-fg)" stroke-width="1.6" stroke-linecap="round"/></svg>`;
  }
}

const severityLabel = (s: Severity) => t(s === "error" ? "Error" : s === "warning" ? "Warning" : "Information");
const toolName = (d: Diagnostic) => {
  switch (d.source) {
    case "latex": return "TeX";
    case "bibtex": return "BibTeX";
    case "chktex": return "ChkTeX";
    default: return t("Structure");
  }
};

/** The compact list (DiagnosticListView). */
export function diagnosticList(diags: Diagnostic[]): HTMLElement {
  const root = document.createElement("div");
  root.className = "gx-diag-list";
  for (const d of diags.slice(0, 12)) {
    const row = document.createElement("div");
    row.className = "gx-diag-row";
    const icon = document.createElement("span");
    icon.className = `gx-diag-icon gx-marker-${d.severity}`;
    icon.innerHTML = severityIcon(d.severity);
    icon.setAttribute("aria-label", severityLabel(d.severity));
    const body = document.createElement("div");
    body.className = "gx-diag-body";
    const msg = document.createElement("div");
    msg.className = "gx-diag-message";
    msg.textContent = d.message;
    body.append(msg);
    const detail = d.detail?.trim();
    if (detail) {
      const el = document.createElement("div");
      el.className = "gx-diag-detail";
      el.textContent = detail;
      body.append(el);
    }
    const src = document.createElement("div");
    src.className = "gx-diag-source";
    src.textContent = d.line
      ? t("%@ · %@ · line %lld", severityLabel(d.severity), toolName(d), d.line)
      : t("%@ · %@", severityLabel(d.severity), toolName(d));
    body.append(src);
    row.append(icon, body);
    root.append(row);
  }
  if (diags.length > 12) {
    const more = document.createElement("div");
    more.className = "gx-diag-more";
    more.textContent = t("%lld more…", diags.length - 12);
    root.append(more);
  }
  return root;
}

// MARK: Hover + gutter

const diagnosticHover = hoverTooltip((view, pos, side) => {
  const found = diagnosticsAt(view.state, pos, side);
  if (!found.length) return null;
  const first = found[0]!;
  return {
    pos: first.from,
    end: first.to,
    above: false,
    create: () => ({ dom: diagnosticList(found.map((a) => a.diag)) }),
  };
}, { hoverTime: 450 });

let openPopover: HTMLElement | null = null;
function closePopover() {
  openPopover?.remove();
  openPopover = null;
  document.removeEventListener("mousedown", outside, true);
  document.removeEventListener("keydown", escape, true);
}
const outside = (e: Event) => { if (openPopover && !openPopover.contains(e.target as Node)) closePopover(); };
const escape = (e: KeyboardEvent) => { if (e.key === "Escape") closePopover(); };

/** Shows `diags` in a transient popover to the right of `anchor` (gutter marker click). */
function showPopover(view: EditorView, diags: Diagnostic[], anchor: DOMRect) {
  closePopover();
  const pop = document.createElement("div");
  pop.className = "gx-popover";
  pop.append(diagnosticList(diags));
  view.dom.ownerDocument.body.append(pop);
  const r = pop.getBoundingClientRect();
  const vw = window.innerWidth, vh = window.innerHeight;
  const x = Math.min(anchor.right + 6, vw - r.width - 8);
  const y = Math.max(8, Math.min(anchor.top + anchor.height / 2 - 18, vh - r.height - 8));
  pop.style.left = `${Math.max(8, x)}px`;
  pop.style.top = `${y}px`;
  openPopover = pop;
  setTimeout(() => {
    document.addEventListener("mousedown", outside, true);
    document.addEventListener("keydown", escape, true);
  });
  view.scrollDOM.addEventListener("scroll", closePopover, { once: true });
}

const markerGutter = gutter({
  class: "gx-diag-gutter",
  markers: (view) => view.state.field(diagnosticsField).markers,
  initialSpacer: () => markerFor.error,
  domEventHandlers: {
    mousedown(view, block, event) {
      const diags = diagnosticsOnLine(view.state, block.from);
      if (!diags.length) return false;
      const target = (event.target as HTMLElement).closest(".cm-gutterElement") ?? (event.target as HTMLElement);
      showPopover(view, diags, target.getBoundingClientRect());
      event.preventDefault();
      return true;
    },
  },
});

export const diagnostics = () => [diagnosticsField, markerGutter, diagnosticHover];
export { closePopover as closeDiagnosticPopover };

// Completion (port of EditorKit/CompletionController.swift + CompletionListView.swift): debounced auto-trigger
// after `\com…` or inside `\ref{…}`-style arguments, manual trigger (Ctrl+Space), live re-filtering while
// typing, keyboard navigation, "Tab completes", and the Liquid Glass list (kind badge, monospaced label with
// the typed prefix emphasised, secondary detail). Items come from the backend (`opts.complete`).
import { Prec, StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
import { EditorView, ViewPlugin, keymap, showTooltip, type Tooltip, type TooltipView, type ViewUpdate } from "@codemirror/view";
import type { CompletionItem, CompletionResponse } from "../ipc";
import { commandPrefixStart, isCommandLetter, isInCommandArgument } from "./scan";
import { insertSnippetAt, parseSnippet, snippetField, snippetHasMirrors } from "./snippet";
import { plainTab, unitOf } from "./editing";
import { t } from "./i18n";

export interface CompletionConfig {
  /** Asks the backend for completions at `pos` of the view's document. */
  request(view: EditorView, pos: number): Promise<CompletionResponse | null>;
  /** False in Vim normal/visual mode. */
  isInsertLike(view: EditorView): boolean;
}

interface Open {
  from: number;
  to: number;
  prefix: string;
  items: readonly CompletionItem[];
  /** The full list from the backend (local filtering narrows `items` while a refresh is in flight). */
  all: readonly CompletionItem[];
  selected: number;
  /** The user picked the row with the arrows (Tab then accepts it as is). */
  chosen: boolean;
}

const ROW = 24, PAD = 6, MAX_ROWS = 9, MAX_RENDERED = 400;
const setOpen = StateEffect.define<Open | null>();
const select = StateEffect.define<number>();

function filterLocally(all: readonly CompletionItem[], prefix: string): readonly CompletionItem[] {
  if (!prefix) return all;
  const p = prefix.toLowerCase();
  const narrowed = all.filter((i) => i.label.toLowerCase().startsWith(p));
  return narrowed.length ? narrowed : all;
}

const completionField = StateField.define<Open | null>({
  create: () => null,
  update(open, tr) {
    for (const e of tr.effects) if (e.is(setOpen)) return e.value;
    if (!open) return null;
    if (tr.isUserEvent("input.complete") || tr.isUserEvent("undo") || tr.isUserEvent("redo")) return null;
    let next = open;
    if (tr.docChanged) {
      const from = tr.changes.mapPos(open.from, -1);
      const caret = tr.state.selection.main.head;
      const to = Math.max(caret, tr.changes.mapPos(open.to, 1));
      const prefix = tr.state.sliceDoc(from, Math.max(from, caret));
      const items = filterLocally(open.all, prefix);
      const keep = items.indexOf(open.items[open.selected]!);
      next = { ...open, from, to, prefix, items, selected: keep >= 0 ? keep : 0, chosen: keep >= 0 && open.chosen };
    }
    for (const e of tr.effects) if (e.is(select)) next = { ...next, selected: e.value, chosen: true };
    if (tr.selection || tr.docChanged) {
      const sel = tr.state.selection.main;
      const doc = tr.state.doc;
      if (!sel.empty || tr.state.selection.ranges.length > 1 || sel.head < next.from ||
        doc.lineAt(sel.head).number !== doc.lineAt(Math.min(next.from, doc.length)).number) return null;
    }
    return next.items.length ? next : null;
  },
  provide: (f) => showTooltip.from(f, (open): Tooltip | null =>
    open ? { pos: open.from, above: false, strictSide: false, arrow: false, create: createListView } : null),
});

export const isCompletionOpen = (state: EditorState) => state.field(completionField, false) != null;

// MARK: The list view

const kindInfo: Record<string, [string, string, string]> = {
  command: ["\\", "tx-k-command", "Command"],
  environment: ["E", "tx-k-environment", "Environment"],
  label: ["L", "tx-k-label", "Label"],
  citation: ["B", "tx-k-citation", "Citation"],
  file: ["F", "tx-k-file", "File"],
  package: ["P", "tx-k-package", "Package"],
  snippet: ["S", "tx-k-snippet", "Snippet"],
};

function createListView(view: EditorView): TooltipView {
  const dom = document.createElement("div");
  dom.className = "gx-completion";
  dom.setAttribute("role", "listbox");
  dom.setAttribute("aria-label", t("Completions"));
  const scroller = document.createElement("div");
  scroller.className = "gx-completion-scroll";
  dom.append(scroller);
  let shownItems: readonly CompletionItem[] | null = null;
  let shownPrefix = "";
  let shownSelected = -1;
  let rows: HTMLElement[] = [];

  const render = (open: Open) => {
    if (open.items !== shownItems || open.prefix !== shownPrefix) {
      shownItems = open.items;
      shownPrefix = open.prefix;
      shownSelected = -1;
      const frag = document.createDocumentFragment();
      rows = [];
      const p = open.prefix.toLowerCase();
      const count = Math.min(open.items.length, MAX_RENDERED);
      for (let i = 0; i < count; i++) {
        const item = open.items[i]!;
        const row = document.createElement("div");
        row.className = "gx-completion-row";
        row.setAttribute("role", "option");
        row.dataset.index = String(i);
        const [letter, cls, name] = kindInfo[item.kind] ?? ["•", "tx-k-file", item.kind];
        const badge = document.createElement("span");
        badge.className = `gx-badge ${cls}`;
        badge.textContent = letter;
        badge.setAttribute("aria-label", t(name));
        const label = document.createElement("span");
        label.className = "gx-completion-label";
        if (p && item.label.toLowerCase().startsWith(p)) {
          const b = document.createElement("b");
          b.textContent = item.label.slice(0, p.length);
          label.append(b, item.label.slice(p.length));
        } else label.textContent = item.label;
        row.append(badge, label);
        if (item.detail) {
          const detail = document.createElement("span");
          detail.className = "gx-completion-detail";
          detail.textContent = item.detail;
          row.append(detail);
        }
        if (item.package) {
          const pkg = document.createElement("span");
          pkg.className = "gx-completion-package";
          pkg.textContent = item.package;
          row.append(pkg);
        }
        rows.push(row);
        frag.append(row);
      }
      scroller.replaceChildren(frag);
      scroller.style.maxHeight = `${Math.min(count, MAX_ROWS) * ROW + PAD * 2}px`;
    }
    if (open.selected !== shownSelected) {
      rows[shownSelected]?.classList.remove("gx-selected");
      rows[shownSelected]?.setAttribute("aria-selected", "false");
      shownSelected = open.selected;
      const row = rows[shownSelected];
      if (row) {
        row.classList.add("gx-selected");
        row.setAttribute("aria-selected", "true");
        // Keep the selection visible without measuring the DOM: rows have a fixed height.
        const top = PAD + shownSelected * ROW, bottom = top + ROW;
        const viewH = Math.min(rows.length, MAX_ROWS) * ROW + PAD * 2;
        const st = scroller.scrollTop;
        if (top - PAD < st) scroller.scrollTop = top - PAD;
        else if (bottom + PAD > st + viewH) scroller.scrollTop = bottom + PAD - viewH;
      }
    }
  };

  dom.addEventListener("mousedown", (e) => {
    const row = (e.target as HTMLElement).closest<HTMLElement>(".gx-completion-row");
    e.preventDefault();
    if (row) accept(view, Number(row.dataset.index));
  });

  const open = view.state.field(completionField);
  if (open) render(open);
  return {
    dom,
    offset: { x: -(PAD + 6 + 18 + 8), y: 3 },
    update(u: ViewUpdate) {
      const o = u.state.field(completionField);
      if (o) render(o);
    },
  };
}

// MARK: Accepting

/** Whether accepting `item` would only re-type the text it replaces (`\sec` → `\sec`). */
function isNoOp(state: EditorState, item: CompletionItem, open: Open): boolean {
  const current = state.sliceDoc(open.from, open.to);
  const snippet = parseSnippet(item.insertText);
  return snippet.text === current && snippet.stops.length === 0;
}

/** Tab never just re-types what is there (see CompletionController.tabChoice): -1 means "plain Tab". */
function tabChoice(state: EditorState, open: Open): number {
  const first = open.items[0];
  if (!first) return -1;
  if (!isNoOp(state, first, open)) return 0;
  if (state.field(snippetField, false)) return -1;
  const typed = open.prefix.toLowerCase();
  return open.items.findIndex((i) => i.label.toLowerCase().startsWith(typed) && !isNoOp(state, i, open));
}

function accept(view: EditorView, index: number): boolean {
  const open = view.state.field(completionField);
  const item = open?.items[index];
  if (!open || !item) return false;
  view.dispatch({ effects: setOpen.of(null) });
  insertSnippetAt(view, item.insertText, open.from, Math.max(open.to, view.state.selection.main.head), unitOf(view.state));
  return true;
}

function acceptWithTab(view: EditorView): boolean {
  const open = view.state.field(completionField);
  if (!open) return false;
  if (open.chosen) return accept(view, open.selected);
  const choice = tabChoice(view.state, open);
  if (choice >= 0) return accept(view, choice);
  view.dispatch({ effects: setOpen.of(null) });
  return plainTab(view);
}

function move(view: EditorView, delta: number): boolean {
  const open = view.state.field(completionField);
  if (!open) return false;
  const count = Math.min(open.items.length, MAX_RENDERED);
  let index = open.selected + delta;
  index = Math.abs(delta) === 1 ? (index + count) % count : Math.max(0, Math.min(index, count - 1));
  view.dispatch({ effects: select.of(index) });
  return true;
}

const dismiss = (view: EditorView) => {
  if (!view.state.field(completionField)) return false;
  view.dispatch({ effects: setOpen.of(null) });
  return true;
};

// MARK: Local fallbacks for Tab (never wait on a slow backend)

/** The last backend answer, reusable while the typed text only extends its prefix on the same line. */
interface Cached {
  line: number;
  /** Line text before `from`: when it is unchanged the completion context is the same. */
  before: string;
  fromCol: number;
  prefix: string;
  /** The backend range ended at the caret (the usual case). */
  toAtCaret: boolean;
  context: CompletionResponse["context"];
  inMath: boolean;
  items: readonly CompletionItem[];
}

/** The most used commands, so Tab completes `\sec` instantly even before the backend has answered once. */
const CATALOG: ReadonlyArray<readonly [string, string, string?]> = [
  ["\\section", "\\section{${1:title}}"], ["\\subsection", "\\subsection{${1:title}}"],
  ["\\subsubsection", "\\subsubsection{${1:title}}"], ["\\paragraph", "\\paragraph{${1:title}}"],
  ["\\chapter", "\\chapter{${1:title}}"], ["\\label", "\\label{${1:key}}"], ["\\ref", "\\ref{${1:key}}"],
  ["\\eqref", "\\eqref{${1:key}}"], ["\\cite", "\\cite{${1:key}}"], ["\\begin", "\\begin{${1:environment}}"],
  ["\\end", "\\end{${1:environment}}"], ["\\textbf", "\\textbf{${1:text}}"], ["\\textit", "\\textit{${1:text}}"],
  ["\\emph", "\\emph{${1:text}}"], ["\\texttt", "\\texttt{${1:text}}"], ["\\footnote", "\\footnote{${1:text}}"],
  ["\\frac", "\\frac{${1:num}}{${2:den}}"], ["\\sqrt", "\\sqrt{${1:x}}"], ["\\sum", "\\sum_{${1:i=1}}^{${2:n}}"],
  ["\\int", "\\int_{${1:a}}^{${2:b}}"], ["\\alpha", "\\alpha"], ["\\beta", "\\beta"], ["\\gamma", "\\gamma"],
  ["\\delta", "\\delta"], ["\\lambda", "\\lambda"], ["\\mathbb", "\\mathbb{${1:R}}"], ["\\mathbf", "\\mathbf{${1:x}}"],
  ["\\includegraphics", "\\includegraphics{${1:file}}", "graphicx"], ["\\usepackage", "\\usepackage{${1:package}}"],
  ["\\documentclass", "\\documentclass{${1:article}}"], ["\\input", "\\input{${1:file}}"],
  ["\\include", "\\include{${1:file}}"], ["\\item", "\\item "], ["\\caption", "\\caption{${1:text}}"],
  ["\\centering", "\\centering"], ["\\newcommand", "\\newcommand{\\${1:name}}{${2:definition}}"],
  ["\\title", "\\title{${1:title}}"], ["\\author", "\\author{${1:name}}"], ["\\maketitle", "\\maketitle"],
  ["\\tableofcontents", "\\tableofcontents"], ["\\bibliography", "\\bibliography{${1:file}}"],
  ["\\url", "\\url{${1:url}}"], ["\\href", "\\href{${1:url}}{${2:text}}", "hyperref"],
];

/** Command completions from the built-in catalog (command prefixes only, `\` + at least one letter). */
function catalogMatches(prefix: string): CompletionItem[] {
  if (prefix.length < 2 || prefix[0] !== "\\") return [];
  return CATALOG.filter(([label]) => label.startsWith(prefix))
    .map(([label, insertText, pkg]) => ({ label, kind: "command" as const, insertText, ...(pkg ? { package: pkg } : {}) }));
}

/** How long Tab waits for the backend before using local data. */
const TAB_WAIT_MS = 120;

// MARK: Controller

class Controller {
  timer: ReturnType<typeof setTimeout> | null = null;
  refreshQueued = false;
  seq = 0;
  cached: Cached | null = null;
  constructor(readonly view: EditorView, readonly config: CompletionConfig) {}

  get triggerPending() { return this.timer !== null; }

  cancelPending() {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  }

  update(u: ViewUpdate) {
    const open = u.state.field(completionField);
    if (u.focusChanged && !u.view.hasFocus) {
      this.cancelPending();
      if (open) setTimeout(() => { if (!this.view.hasFocus) dismiss(this.view); });
      return;
    }
    if (!u.docChanged && !u.selectionSet) return;
    if (open) {
      if (u.docChanged) this.queueRefresh();
      return;
    }
    if (!u.docChanged) { this.cancelPending(); return; }
    const typed = u.transactions.some((tr) => tr.isUserEvent("input.type"));
    if (!typed || !this.config.isInsertLike(u.view)) { this.cancelPending(); return; }
    const sel = u.state.selection.main;
    if (!sel.empty) { this.cancelPending(); return; }
    const line = u.state.doc.lineAt(sel.head);
    const col = sel.head - line.from;
    if (commandPrefixStart(line.text, col) < 0 && !isInCommandArgument(line.text, col)) { this.cancelPending(); return; }
    this.cancelPending();
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.trigger(false);
    }, 80);
  }

  queueRefresh() {
    if (this.refreshQueued) return;
    this.refreshQueued = true;
    queueMicrotask(() => {
      this.refreshQueued = false;
      if (this.view.state.field(completionField)) void this.trigger(false, true);
    });
  }

  /** Fetches completions at the caret; null when the state moved on meanwhile or nothing is available. */
  async fetch(): Promise<{ state: EditorState; res: CompletionResponse } | null> {
    const view = this.view;
    const state = view.state;
    const sel = state.selection.main;
    if (!sel.empty || state.selection.ranges.length > 1) return null;
    const mine = ++this.seq;
    let res: CompletionResponse | null = null;
    try {
      res = await this.config.request(view, sel.head);
    } catch {
      res = null;
    }
    if (mine !== this.seq || view.state !== state) return null;
    if (!res || !res.items.length || res.from > sel.head || res.to > state.doc.length || res.from < 0) return null;
    this.remember(state, sel.head, res);
    return { state, res };
  }

  private remember(state: EditorState, head: number, res: CompletionResponse) {
    const line = state.doc.lineAt(head);
    if (res.from < line.from) { this.cached = null; return; }
    const fromCol = res.from - line.from;
    this.cached = {
      line: line.number, before: line.text.slice(0, fromCol), fromCol, prefix: state.sliceDoc(res.from, head),
      toAtCaret: res.to <= head, context: res.context, inMath: res.inMath, items: res.items,
    };
  }

  /** A response for the caret derived from the last backend answer, or null when it does not apply. */
  fromCache(state: EditorState): CompletionResponse | null {
    const c = this.cached;
    if (!c || !c.toAtCaret) return null;
    const head = state.selection.main.head;
    const line = state.doc.lineAt(head);
    if (line.number !== c.line || line.text.slice(0, c.fromCol) !== c.before) return null;
    const from = line.from + c.fromCol;
    const typed = state.sliceDoc(from, head);
    if (!typed.toLowerCase().startsWith(c.prefix.toLowerCase())) return null;
    const p = typed.toLowerCase();
    const items = c.items.filter((i) => i.label.toLowerCase().startsWith(p));
    if (!items.length) return null;
    return { from, to: head, prefix: typed, context: c.context, inMath: c.inMath, items };
  }

  /** The backend answer, but never later than TAB_WAIT_MS: cache or catalog answer meanwhile. */
  async fetchForTab(state: EditorState): Promise<CompletionResponse | null> {
    const local = this.fromCache(state);
    if (local) { void this.fetch(); return local; }
    const head = state.selection.main.head;
    const timeout = new Promise<null>((r) => setTimeout(() => r(null), TAB_WAIT_MS));
    const got = await Promise.race([this.fetch().then((g) => g?.res ?? null), timeout]);
    if (got) return got;
    if (this.view.state !== state) return null;
    const line = state.doc.lineAt(head);
    const col = head - line.from;
    const start = commandPrefixStart(line.text, col);
    if (start < 0) return null; // arguments (labels, citations, files) need the backend
    const prefix = line.text.slice(start, col);
    const items = catalogMatches(prefix);
    if (!items.length) return null;
    this.seq++; // the late backend answer must not open a popup under the user's hands
    return { from: line.from + start, to: head, prefix, context: "command", inMath: false, items };
  }

  async trigger(manual: boolean, refresh = false) {
    this.cancelPending();
    if (!this.config.isInsertLike(this.view)) return;
    const startState = this.view.state;
    const got = await this.fetch();
    if (!got) {
      // Only close when nothing changed meanwhile (a newer request is responsible otherwise).
      if (this.view.state === startState && (refresh || manual)) dismiss(this.view);
      return;
    }
    this.present(got.res, refresh, 0);
  }

  present(res: CompletionResponse, keepSelection: boolean, preferred: number) {
    const previous = this.view.state.field(completionField);
    const prevItem = keepSelection && previous ? previous.items[previous.selected] : undefined;
    let selected = preferred;
    let chosen = false;
    if (prevItem) {
      const i = res.items.findIndex((x) => x.label === prevItem.label && x.kind === prevItem.kind);
      if (i >= 0) { selected = i; chosen = previous!.chosen; }
    }
    const open: Open = {
      from: res.from, to: res.to, prefix: res.prefix, items: res.items, all: res.items,
      selected: Math.min(selected, res.items.length - 1), chosen,
    };
    this.view.dispatch({ effects: setOpen.of(open) });
  }

  /** "Tab completes" with the popup closed. Returns false when Tab should do its usual job right away. */
  tabComplete(): boolean {
    const view = this.view;
    const state = view.state;
    const sel = state.selection.main;
    if (!this.config.isInsertLike(view) || state.readOnly || !sel.empty || state.selection.ranges.length > 1 ||
      snippetHasMirrors(state)) return false;
    const line = state.doc.lineAt(sel.head);
    const col = sel.head - line.from;
    const cmd = commandPrefixStart(line.text, col) >= 0;
    if (cmd && col < line.length && isCommandLetter(line.text.charCodeAt(col))) return false;
    if (!cmd && !isInCommandArgument(line.text, col)) return false;
    const triggerPending = this.triggerPending;
    this.cancelPending();
    void this.fetchForTab(state).then((res) => {
      if (!res) {
        if (view.state === state) plainTab(view);
        return;
      }
      if (view.state !== state) return;
      if (res.prefix.startsWith("\\") && res.prefix.length < 2) { plainTab(view); return; }
      const open: Open = { from: res.from, to: res.to, prefix: res.prefix, items: res.items, all: res.items, selected: 0, chosen: false };
      const choice = tabChoice(view.state, open);
      if (choice < 0) { plainTab(view); return; }
      const additions = res.items.filter((i) => !isNoOp(view.state, i, open)).length;
      if (additions === 1 || triggerPending) {
        insertSnippetAt(view, res.items[choice]!.insertText, res.from, res.to, unitOf(view.state));
      } else {
        this.present(res, false, choice);
      }
    });
    return true;
  }
}

export interface CompletionSupport {
  extension: Extension;
  /** Tab with the popup closed: "Tab completes" when a completable prefix precedes the caret. */
  tabComplete(view: EditorView): boolean;
  /** Ctrl+Space: always opens completion at the caret. */
  startCompletion(view: EditorView): boolean;
}

export function completion(config: CompletionConfig): CompletionSupport {
  const plugin = ViewPlugin.define((view) => new Controller(view, config));
  const tabComplete = (view: EditorView) => view.plugin(plugin)?.tabComplete() ?? false;
  const startCompletion = (view: EditorView) => {
    const c = view.plugin(plugin);
    if (!c) return false;
    void c.trigger(true);
    return true;
  };
  const extension = [
    completionField,
    plugin,
    Prec.highest(keymap.of([
      { key: "ArrowDown", run: (v) => move(v, 1) },
      { key: "ArrowUp", run: (v) => move(v, -1) },
      { key: "PageDown", run: (v) => move(v, MAX_ROWS - 1) },
      { key: "PageUp", run: (v) => move(v, -(MAX_ROWS - 1)) },
      { key: "Enter", run: (v) => { const o = v.state.field(completionField); return o ? accept(v, o.selected) : false; } },
      { key: "Tab", run: acceptWithTab, shift: (v) => { const o = v.state.field(completionField); return o ? accept(v, o.selected) : false; } },
      { key: "Escape", run: dismiss },
      { key: "Ctrl-Space", run: startCompletion },
    ])),
  ];
  return { extension, tabComplete, startCompletion };
}

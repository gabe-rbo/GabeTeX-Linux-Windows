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

// MARK: Controller

class Controller {
  timer: ReturnType<typeof setTimeout> | null = null;
  refreshQueued = false;
  seq = 0;
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
    return { state, res };
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
    void this.fetch().then((got) => {
      if (!got) {
        if (view.state === state) plainTab(view);
        return;
      }
      const { res } = got;
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

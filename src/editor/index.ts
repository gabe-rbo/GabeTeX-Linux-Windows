// The LaTeX editor (CodeMirror 6), mirroring Sources/EditorKit. One EditorView; one EditorState per open file
// kept in a cache, so switching files keeps undo history, selection, scroll, diagnostics and snippet state.
// Heavy parts are lazy: Vim mode (@replit/codemirror-vim) loads on first use.
import "./editor.css";
import { Compartment, EditorSelection, EditorState, Facet, Prec, Text, Transaction, type Extension, type StateEffect, type TransactionSpec } from "@codemirror/state";
import {
  EditorView, drawSelection, highlightActiveLine, highlightActiveLineGutter, highlightSpecialChars, keymap,
  lineNumbers, tooltips, type Command,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { indentUnit } from "@codemirror/language";
import { openSearchPanel, search, searchKeymap } from "@codemirror/search";
import type { CompletionResponse, Diagnostic } from "../ipc";
import { latex } from "./latex";
import { autoClosers, autoPairInput, backTab, plainTab, smartBackspace, smartEnter, toggleComment, unitOf } from "./editing";
import { cancelSnippet, insertSnippetAt, snippets } from "./snippet";
import { completion } from "./completion";
import { closeDiagnosticPopover, diagnostics, setDiagnosticsEffect } from "./diagnostics";
import { flashField, revealLine } from "./reveal";
import { ctrlClick } from "./mouse";
import { lineEnding } from "./scan";
import { cmPhrasesPT, isPortuguese } from "./i18n";

export interface EditorSettings {
  fontSize: number;
  vim: boolean;
  lineNumbers: boolean;
  wrap: boolean;
  tabSize: number;
  spellcheck?: boolean;
}

export interface EditorOptions {
  onChange(path: string, text: string): void;
  onCursor(path: string, line: number, column: number): void;
  /** Ctrl-click on an `\input`/`\include`/`\includegraphics`/`\bibliography` … argument (raw target text). */
  onOpenInclude(target: string, fromPath: string): void;
  /** Ctrl-click on a `\ref`-family key (or a `\label`). */
  onJumpToLabel(label: string): void;
  /** Show in PDF: Ctrl-click on ordinary text, Ctrl+Shift+J (Mac ⇧⌘J) or Ctrl+Alt+J at the caret. */
  onForwardSync(path: string, line: number, column: number): void;
  complete(path: string, text: string, line: number, column: number): Promise<CompletionResponse | null>;
  settings: EditorSettings;
}

export interface EditorHandle {
  open(path: string, text: string): void;
  close(path: string): void;
  getText(path: string): string | undefined;
  isDirty(path: string): boolean;
  markSaved(path: string, text?: string): void;
  setDiagnostics(path: string, diags: Diagnostic[]): void;
  reveal(path: string, line: number, column?: number, flash?: boolean): void;
  applySettings(s: Partial<EditorSettings>): void;
  focus(): void;
  insertSnippet(snippet: string): void;
  destroy(): void;
  // Extras beyond the shared contract (optional for the shell).
  /** Path of the file shown, or null. */
  currentPath(): string | null;
  /** Ctrl+/ */
  toggleComment(): void;
  /** Ctrl+F (find) — the replace field is part of the same panel. */
  openSearch(): void;
  /** Show in PDF at the caret (calls `onForwardSync`). */
  forwardSync(): void;
  /** Opens completion at the caret (Ctrl+Space). */
  complete(): void;
}

const DEFAULTS: EditorSettings = { fontSize: 13, vim: false, lineNumbers: true, wrap: true, tabSize: 2, spellcheck: false };

/** The file a state belongs to. */
const pathFacet = Facet.define<string, string>({ combine: (v) => v[0] ?? "" });

interface Entry {
  path: string;
  state: EditorState;
  eol: "\n" | "\r\n" | "\r";
  saved: Text;
  scroll: StateEffect<unknown> | null;
  settingsVersion: number;
  dirtyCache: { doc: Text; saved: Text; dirty: boolean } | null;
}

type VimModule = typeof import("@replit/codemirror-vim");

const normalize = (text: string) => (text.indexOf("\r") < 0 ? text : text.replace(/\r\n?/g, "\n"));
const toText = (text: string) => Text.of(text.split("\n"));

export function createEditor(parent: HTMLElement, opts: EditorOptions): EditorHandle {
  let settings: EditorSettings = { ...DEFAULTS, ...opts.settings };
  let settingsVersion = 0;
  const cache = new Map<string, Entry>();
  const pendingDiagnostics = new Map<string, Diagnostic[]>();
  const pendingReveal = new Map<string, { line: number; column?: number; flash: boolean }>();
  let current: Entry | null = null;
  let vimModule: VimModule | null = null;
  let vimLoading: Promise<void> | null = null;
  let destroyed = false;

  // MARK: Settings → extensions

  const vimC = new Compartment(), lineNumbersC = new Compartment(), wrapC = new Compartment();
  const tabC = new Compartment(), spellC = new Compartment();

  const vimExt = (): Extension => (settings.vim && vimModule ? Prec.highest(vimModule.vim({ status: true })) : []);
  const isInsertLike = (view: EditorView) => {
    if (!settings.vim || !vimModule) return true;
    const cm = vimModule.getCM(view) as unknown as { state?: { vim?: { insertMode?: boolean } } } | null;
    return cm?.state?.vim?.insertMode ?? true;
  };
  const selectLines = (view: EditorView, fromLine: number, toLine: number) => {
    const doc = view.state.doc;
    const a = doc.line(Math.min(fromLine, toLine)), b = doc.line(Math.max(fromLine, toLine));
    view.dispatch({ selection: EditorSelection.range(a.from, b.number < doc.lines ? b.to + 1 : b.to), userEvent: "select.pointer" });
  };
  let dragAnchorLine = 0;
  const lineNumbersExt = (): Extension =>
    settings.lineNumbers
      ? lineNumbers({
        domEventHandlers: {
          mousedown(view, block, event) {
            const e = event as MouseEvent;
            if (e.button !== 0) return false;
            const n = view.state.doc.lineAt(block.from).number;
            dragAnchorLine = e.shiftKey ? view.state.doc.lineAt(view.state.selection.main.anchor).number : n;
            selectLines(view, dragAnchorLine, n);
            view.focus();
            const move = (m: MouseEvent) => {
              const pos = view.posAtCoords({ x: view.contentDOM.getBoundingClientRect().left + 4, y: m.clientY }, false);
              selectLines(view, dragAnchorLine, view.state.doc.lineAt(pos).number);
            };
            const up = () => {
              window.removeEventListener("mousemove", move);
              window.removeEventListener("mouseup", up);
            };
            window.addEventListener("mousemove", move);
            window.addEventListener("mouseup", up);
            e.preventDefault();
            return true;
          },
        },
      })
      : [];
  const tabExt = (): Extension => {
    const size = Math.max(1, Math.min(16, Math.round(settings.tabSize) || 2));
    return [EditorState.tabSize.of(size), indentUnit.of(" ".repeat(size))];
  };
  const spellExt = (): Extension =>
    EditorView.contentAttributes.of({
      spellcheck: settings.spellcheck ? "true" : "false", autocorrect: "off", autocapitalize: "off",
    });
  const settingEffects = () => [
    vimC.reconfigure(vimExt()),
    lineNumbersC.reconfigure(lineNumbersExt()),
    wrapC.reconfigure(settings.wrap ? EditorView.lineWrapping : []),
    tabC.reconfigure(tabExt()),
    spellC.reconfigure(spellExt()),
  ];

  // MARK: Commands

  const forwardSyncAt = (view: EditorView, pos: number) => {
    const path = view.state.facet(pathFacet);
    if (!path) return;
    const line = view.state.doc.lineAt(pos);
    opts.onForwardSync(path, line.number, pos - line.from + 1);
  };
  const forwardSyncCommand: Command = (view) => {
    forwardSyncAt(view, view.state.selection.main.head);
    return true;
  };

  const support = completion({
    request: (view, pos) => {
      const path = view.state.facet(pathFacet);
      if (!path) return Promise.resolve(null);
      const line = view.state.doc.lineAt(pos);
      return opts.complete(path, view.state.doc.toString(), line.number, pos - line.from + 1);
    },
    isInsertLike,
  });

  const editorKeymap = keymap.of([
    { key: "Enter", run: smartEnter },
    { key: "Backspace", run: smartBackspace },
    { key: "Tab", run: (v) => support.tabComplete(v) || plainTab(v), shift: backTab },
    { key: "Escape", run: cancelSnippet },
    { key: "Mod-/", run: toggleComment },
    { key: "Shift-Mod-j", run: forwardSyncCommand },
    { key: "Mod-Alt-j", run: forwardSyncCommand },
  ]);

  // MARK: Change notifications (coalesced to one per frame)

  let pendingChange = false, pendingCursor = false, frame = 0, silent = false;
  const flush = () => {
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
    if (!current) { pendingChange = pendingCursor = false; return; }
    const path = current.path;
    if (pendingChange) {
      pendingChange = false;
      opts.onChange(path, textOf(current));
    }
    if (pendingCursor) {
      pendingCursor = false;
      const head = view.state.selection.main.head;
      const line = view.state.doc.lineAt(head);
      opts.onCursor(path, line.number, head - line.from + 1);
    }
  };
  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(() => { frame = 0; flush(); });
  };

  const base: Extension = [
    support.extension,
    vimC.of(vimExt()),
    history(),
    drawSelection(),
    highlightSpecialChars(),
    highlightActiveLine(),
    highlightActiveLineGutter(),
    EditorState.allowMultipleSelections.of(true),
    diagnostics(),
    lineNumbersC.of(lineNumbersExt()),
    latex(),
    snippets(),
    autoClosers,
    autoPairInput,
    flashField,
    wrapC.of(settings.wrap ? EditorView.lineWrapping : []),
    tabC.of(tabExt()),
    spellC.of(spellExt()),
    search({ top: true }),
    tooltips({ parent: document.body }),
    Prec.high(editorKeymap),
    keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap]),
    ctrlClick(() => ({
      openInclude: (target) => current && opts.onOpenInclude(target, current.path),
      jumpToLabel: (label) => opts.onJumpToLabel(label),
      forwardSync: (pos) => forwardSyncAt(view, pos),
    })),
    EditorView.updateListener.of((u) => {
      if (silent || !current) return;
      if (u.docChanged) { pendingChange = true; current.dirtyCache = null; }
      if (u.docChanged || u.selectionSet) pendingCursor = true;
      if (pendingChange || pendingCursor) schedule();
    }),
    EditorView.editorAttributes.of({ class: "gx-editor" }),
    isPortuguese ? EditorState.phrases.of(cmPhrasesPT) : [],
  ];

  const makeState = (path: string, text: string) =>
    EditorState.create({ doc: text, extensions: [pathFacet.of(path), base] });
  const emptyState = () =>
    EditorState.create({ doc: "", extensions: [base, EditorState.readOnly.of(true), EditorView.editable.of(false)] });

  const view = new EditorView({ state: emptyState(), parent });
  view.dom.style.setProperty("--gx-font-size", `${settings.fontSize}px`);

  // MARK: Cache

  const stateOf = (e: Entry) => (e === current ? view.state : e.state);
  function textOf(e: Entry): string {
    const s = stateOf(e).doc.toString();
    return e.eol === "\n" ? s : s.replace(/\n/g, e.eol);
  }

  /** Applies a transaction to a cached (not shown) state, or to the view when it is shown. */
  function update(e: Entry, spec: TransactionSpec) {
    if (e === current) view.dispatch(spec);
    else e.state = e.state.update(spec).state;
  }

  function show(e: Entry) {
    if (e === current) return;
    flush();
    if (current) {
      current.state = view.state;
      current.scroll = view.scrollSnapshot();
    }
    closeDiagnosticPopover();
    if (e.settingsVersion !== settingsVersion) {
      e.state = e.state.update({ effects: settingEffects() }).state;
      e.settingsVersion = settingsVersion;
    }
    current = e;
    view.setState(e.state);
    if (e.scroll) view.dispatch({ effects: e.scroll });
    pendingCursor = true;
    schedule();
  }

  function applyPending(e: Entry) {
    const d = pendingDiagnostics.get(e.path);
    if (d) {
      pendingDiagnostics.delete(e.path);
      update(e, { effects: setDiagnosticsEffect.of(d) });
    }
    const r = pendingReveal.get(e.path);
    if (r && e === current) {
      pendingReveal.delete(e.path);
      revealLine(view, r.line, r.column, r.flash);
    }
  }

  async function loadVim() {
    if (vimModule || !settings.vim) return;
    vimLoading ??= import("@replit/codemirror-vim").then((m) => {
      vimModule = m;
    });
    await vimLoading;
    if (destroyed || !settings.vim) return;
    settingsVersion++;
    view.dispatch({ effects: vimC.reconfigure(vimExt()) });
    if (current) current.settingsVersion = settingsVersion;
  }
  if (settings.vim) void loadVim();

  // MARK: Handle

  const handle: EditorHandle = {
    open(path, text) {
      const eol = lineEnding(text);
      const norm = normalize(text);
      let e = cache.get(path);
      if (!e) {
        e = {
          path, state: makeState(path, norm), eol, saved: toText(norm), scroll: null, settingsVersion,
          dirtyCache: null,
        };
        cache.set(path, e);
      } else {
        e.eol = eol;
        const doc = stateOf(e).doc;
        if (doc.length !== norm.length || doc.toString() !== norm) {
          // Text changed behind the editor (reload): replace only the differing middle part.
          const old = doc.toString();
          let a = 0;
          const max = Math.min(old.length, norm.length);
          while (a < max && old.charCodeAt(a) === norm.charCodeAt(a)) a++;
          let b = 0;
          while (b < max - a && old.charCodeAt(old.length - 1 - b) === norm.charCodeAt(norm.length - 1 - b)) b++;
          silent = true;
          try {
            update(e, {
              changes: { from: a, to: old.length - b, insert: norm.slice(a, norm.length - b) },
              annotations: Transaction.addToHistory.of(false),
            });
          } finally {
            silent = false;
          }
          e.dirtyCache = null;
        }
      }
      show(e);
      applyPending(e);
    },

    close(path) {
      const e = cache.get(path);
      pendingDiagnostics.delete(path);
      pendingReveal.delete(path);
      if (!e) return;
      if (e === current) {
        flush();
        current = null;
        closeDiagnosticPopover();
        view.setState(emptyState());
      }
      cache.delete(path);
    },

    getText(path) {
      const e = cache.get(path);
      return e ? textOf(e) : undefined;
    },

    isDirty(path) {
      const e = cache.get(path);
      if (!e) return false;
      const doc = stateOf(e).doc;
      const c = e.dirtyCache;
      if (c && c.doc === doc && c.saved === e.saved) return c.dirty;
      const dirty = !doc.eq(e.saved);
      e.dirtyCache = { doc, saved: e.saved, dirty };
      return dirty;
    },

    markSaved(path, text) {
      const e = cache.get(path);
      if (!e) return;
      if (text === undefined) e.saved = stateOf(e).doc;
      else {
        if (/[\r\n]/.test(text)) e.eol = lineEnding(text);
        const norm = normalize(text);
        const doc = stateOf(e).doc;
        e.saved = doc.length === norm.length && doc.toString() === norm ? doc : toText(norm);
      }
      e.dirtyCache = null;
    },

    setDiagnostics(path, diags) {
      const e = cache.get(path);
      if (e) update(e, { effects: setDiagnosticsEffect.of(diags) });
      else pendingDiagnostics.set(path, diags);
    },

    reveal(path, line, column, flash = true) {
      const e = cache.get(path);
      if (!e) {
        pendingReveal.set(path, { line, column, flash });
        return;
      }
      show(e);
      revealLine(view, line, column, flash);
    },

    applySettings(s) {
      settings = { ...settings, ...s };
      settingsVersion++;
      if (s.fontSize !== undefined) view.dom.style.setProperty("--gx-font-size", `${settings.fontSize}px`);
      if (settings.vim && !vimModule) void loadVim();
      view.dispatch({ effects: settingEffects() });
      if (current) current.settingsVersion = settingsVersion;
    },

    focus() {
      view.focus();
    },

    insertSnippet(snippet) {
      if (!current || view.state.readOnly) return;
      const sel = view.state.selection.main;
      insertSnippetAt(view, snippet, sel.from, sel.to, unitOf(view.state));
      view.focus();
    },

    destroy() {
      destroyed = true;
      flush();
      closeDiagnosticPopover();
      view.destroy();
      cache.clear();
      current = null;
    },

    currentPath: () => current?.path ?? null,
    toggleComment: () => { if (current) toggleComment(view); },
    openSearch: () => { if (current) openSearchPanel(view); },
    forwardSync: () => { if (current) forwardSyncCommand(view); },
    complete: () => { if (current) support.startCompletion(view); },
  };
  return handle;
}

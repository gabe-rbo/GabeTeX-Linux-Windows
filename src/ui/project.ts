// The project window (port of ProjectWindow / DetailView / WorkArea / EditorPreviewSplit): glass sidebar, unified
// toolbar, tab pills, banners, editor | PDF split, Issues panel and status bar. The editor and the PDF preview are
// loaded lazily (separate chunks) so the Welcome window starts instantly.
import type { EditorHandle, EditorSettings } from "../editor";
import type { PreviewHandle } from "../preview";
import * as ipc from "../ipc";
import { t, formatList } from "../i18n";
import { settings } from "../app/settings";
import { Workspace } from "../app/workspace";
import { h, basename, reducedMotion } from "./dom";
import { icon } from "./icons";
import { alertDialog, promptDialog } from "./overlay";
import { createSidebar } from "./sidebar";
import { createToolbar } from "./toolbar";
import { createTabStrip } from "./tabs";
import { createBanners } from "./banners";
import { createStatusBar } from "./statusbar";
import { createIssuesPanel } from "./issues";
import { openQuickOpen } from "./palette";

export function editorSettings(): EditorSettings {
  return {
    fontSize: settings.get("fontSize"),
    vim: settings.get("vimMode"),
    lineNumbers: settings.get("showLineNumbers"),
    wrap: settings.get("wrapLines"),
    tabSize: settings.get("indentWidth"),
    spellcheck: settings.get("spellChecking"),
  };
}

export class ProjectView {
  readonly ws: Workspace;
  readonly el: HTMLElement;
  private editorHost = h("div", { class: "editor-host" });
  private previewHost = h("div", { class: "preview-host" });
  private editorEmpty: HTMLElement;
  private previewPlaceholder: HTMLElement;
  private split: HTMLElement;
  private divider: HTMLElement;
  private issuesWrap: HTMLElement;
  private issuesResizer: HTMLElement;
  private sidebarEl: HTMLElement;
  private sidebarResizer: HTMLElement;
  private toolbarSidebarSlot: HTMLElement;
  private cleanups: Array<() => void> = [];
  private ready: Promise<void>;

  constructor(info: ipc.ProjectInfo) {
    const ws = (this.ws = new Workspace(info));
    ws.ask = (o) => alertDialog(o);

    const sidebar = createSidebar(ws, { onOpen: (p) => void ws.open(p).then(() => ws.editor?.focus()) });
    const toolbar = createToolbar(ws, {
      toggleSidebar: () => this.toggleSidebar(),
      togglePreview: () => this.togglePreview(),
      togglePdfOnly: () => this.togglePdfOnly(),
      toggleIssues: () => this.toggleIssues(),
    });
    const tabs = createTabStrip(ws, { onClose: (p) => void this.closeTab(p), onCloseOthers: (p) => void this.closeOthers(p) });
    const banners = createBanners(ws, { openSettings: () => void import("./settingsView").then((m) => m.openSettings("tex")) });
    const status = createStatusBar(ws, { toggleIssues: () => this.toggleIssues() });
    const issues = createIssuesPanel(ws, { hide: () => settings.set("showIssues", false) });
    this.cleanups.push(sidebar.destroy, toolbar.destroy, tabs.destroy, banners.destroy, status.destroy, issues.destroy);

    this.editorEmpty = h("div", { class: "empty-state editor-empty" }, icon("docText", 34),
      h("div", { class: "empty-title", text: t("No File Open") }),
      h("div", { class: "empty-detail", text: t("Choose a file in the sidebar, or press ⌘P to open one quickly.") }));
    this.previewPlaceholder = h("div", { class: "preview-placeholder" });
    this.divider = h("div", { class: "split-divider", attrs: { role: "separator", "aria-orientation": "vertical", "aria-label": t("Editor and PDF divider"), tabindex: 0 } });
    this.split = h("div", { class: "split" },
      h("div", { class: "pane editor-pane" }, this.editorHost, this.editorEmpty),
      this.divider,
      h("div", { class: "pane preview-pane" }, this.previewHost, this.previewPlaceholder));
    this.issuesResizer = h("div", { class: "issues-resizer", attrs: { role: "separator", "aria-orientation": "horizontal" } });
    this.issuesWrap = h("div", { class: "issues-wrap" }, this.issuesResizer, issues.el);

    this.sidebarResizer = h("div", { class: "sidebar-resizer", attrs: { role: "separator", "aria-orientation": "vertical" } });
    const sidebarHead = h("div", { class: "sidebar-head" }, toolbar.sidebarButton);
    sidebar.el.prepend(sidebarHead);
    this.sidebarEl = h("div", { class: "sidebar-wrap" }, sidebar.el, this.sidebarResizer);
    this.toolbarSidebarSlot = h("div", { class: "tb-sidebar-slot" });
    toolbar.el.prepend(this.toolbarSidebarSlot);

    this.el = h("div", { class: "project-root" },
      this.sidebarEl,
      h("div", { class: "detail" }, toolbar.el, tabs.el, banners.el,
        h("div", { class: "work" }, this.split, this.issuesWrap), status.el));

    this.installResizers();
    this.cleanups.push(
      ws.events.on("tabs", () => this.renderEmpty()),
      ws.events.on(["pdf", "compile"], () => this.renderPreviewPlaceholder()),
      ws.events.on("layout", () => this.applyLayout()),
      settings.subscribe(() => this.applyLayout(), ["showPreview", "showIssues", "sidebarVisible", "sidebarWidth", "splitRatio", "issuesHeight"]),
      settings.subscribe(() => {
        ws.editor?.applySettings(editorSettings());
        this.el.style.setProperty("--editor-line-height", String(settings.get("lineHeight")));
      }, ["fontSize", "vimMode", "showLineNumbers", "wrapLines", "indentWidth", "spellChecking", "lineHeight"]),
      settings.subscribe(() => ws.liveCompileSettingChanged(), ["liveCompile"]),
      settings.subscribe(() => ws.events.emit("engine"), ["defaultEngine"]),
    );
    this.el.style.setProperty("--editor-line-height", String(settings.get("lineHeight")));
    this.applyLayout();
    this.renderEmpty();
    this.renderPreviewPlaceholder();
    this.ready = this.mount(info);
  }

  /** Loads the editor and preview chunks, restores tabs and starts the workspace. */
  private async mount(info: ipc.ProjectInfo): Promise<void> {
    const ws = this.ws;
    const [{ createEditor }, { createPreview }] = await Promise.all([import("../editor"), import("../preview")]);
    const editor: EditorHandle = createEditor(this.editorHost, {
      onChange: (path) => ws.didChange(path),
      onCursor: (_path, line, column) => ws.didMoveCursor(line, column),
      onOpenInclude: (target, from) => {
        const path = ws.resolveInclude(target, from);
        if (path) void ws.open(path).then(() => ws.editor?.focus());
        else ws.flash(t("Could not find “%@”.", target));
      },
      onJumpToLabel: (label) => {
        const hit = ws.index?.labels.find((l) => l.name === label);
        if (hit) void ws.open(hit.path, { line: hit.line, flash: true }).then(() => ws.editor?.focus());
        else ws.flash(t("No \\label{%@} was found.", label));
      },
      onForwardSync: (path, line, column) => void ws.forwardSync(path, line, column),
      complete: (path, text, line, column) => ipc.complete(ws.root, path, text, line, column).catch(() => null),
      settings: editorSettings(),
    });
    const preview: PreviewHandle = createPreview(this.previewHost, {
      onInverseSync: (page, x, y) => void ws.inverseSync(page, x, y),
      onPageChange: (page, count) => {
        const changed = count !== ws.pageCount;
        ws.page = page;
        ws.pageCount = count;
        if (changed) ws.events.emit("pdf");
      },
    });
    ws.editor = editor;
    ws.preview = preview;
    this.cleanups.push(() => { editor.destroy(); preview.destroy(); });

    await ws.start();
    // Restore the tabs of the last session, else open the main document.
    const memory = settings.project(ws.root);
    const exists = new Set(ws.flatFiles);
    const tabs = (memory.tabs ?? []).filter((p) => exists.has(p));
    for (const p of tabs) await ws.open(p);
    const first = (memory.current && exists.has(memory.current) ? memory.current : undefined) ?? info.rootFile ?? ws.mainFile;
    if (first) await ws.open(first);
    editor.focus();
    ws.startCompile("live", { force: true });
  }

  whenReady(): Promise<void> {
    return this.ready;
  }

  // ---- layout ------------------------------------------------------------------------------------------------

  private applyLayout(): void {
    const showPreview = settings.get("showPreview") || this.ws.pdfOnly;
    this.el.classList.toggle("no-preview", !showPreview);
    this.el.classList.toggle("pdf-only", this.ws.pdfOnly);
    this.el.classList.toggle("no-issues", !settings.get("showIssues"));
    const sidebar = settings.get("sidebarVisible");
    this.el.classList.toggle("no-sidebar", !sidebar);
    this.el.style.setProperty("--sidebar-width", `${settings.get("sidebarWidth")}px`);
    this.el.style.setProperty("--split", String(settings.get("splitRatio")));
    this.el.style.setProperty("--issues-height", `${settings.get("issuesHeight")}px`);
    // The sidebar button lives in the sidebar head, or in the toolbar while the sidebar is hidden.
    const btn = this.el.querySelector(".tb-sidebar");
    if (btn) {
      const target = sidebar ? this.sidebarEl.querySelector(".sidebar-head") : this.toolbarSidebarSlot;
      if (target && btn.parentElement !== target) target.appendChild(btn);
      const label = sidebar ? t("Hide Sidebar") : t("Show Sidebar");
      btn.setAttribute("title", label);
      btn.setAttribute("aria-label", label);
    }
    this.ws.events.emitSoon("layout");
  }

  private installResizers(): void {
    const drag = (handle: HTMLElement, onMove: (e: PointerEvent) => void, cursor: string) => {
      handle.addEventListener("pointerdown", (e) => {
        e.preventDefault();
        handle.setPointerCapture(e.pointerId);
        document.body.style.cursor = cursor;
        this.el.classList.add("resizing");
        let frame = 0;
        let last: PointerEvent = e;
        const move = (ev: PointerEvent) => {
          last = ev;
          if (frame) return;
          frame = requestAnimationFrame(() => { frame = 0; onMove(last); });
        };
        const up = () => {
          handle.removeEventListener("pointermove", move);
          handle.removeEventListener("pointerup", up);
          handle.removeEventListener("pointercancel", up);
          document.body.style.cursor = "";
          this.el.classList.remove("resizing");
        };
        handle.addEventListener("pointermove", move);
        handle.addEventListener("pointerup", up);
        handle.addEventListener("pointercancel", up);
      });
    };
    drag(this.divider, (e) => {
      const r = this.split.getBoundingClientRect();
      const min = 320, minPreview = 260;
      const x = Math.min(Math.max(e.clientX - r.left, min), r.width - minPreview);
      settings.set("splitRatio", x / r.width);
    }, "col-resize");
    this.divider.addEventListener("dblclick", () => settings.set("splitRatio", 0.55));
    this.divider.addEventListener("keydown", (e) => {
      if (e.key === "ArrowLeft") settings.set("splitRatio", settings.get("splitRatio") - 0.02);
      else if (e.key === "ArrowRight") settings.set("splitRatio", settings.get("splitRatio") + 0.02);
    });
    drag(this.sidebarResizer, (e) => settings.set("sidebarWidth", e.clientX - this.el.getBoundingClientRect().left), "col-resize");
    drag(this.issuesResizer, (e) => {
      const r = this.issuesWrap.parentElement!.getBoundingClientRect();
      settings.set("issuesHeight", Math.min(r.height - 180, r.bottom - e.clientY));
    }, "row-resize");
  }

  private renderEmpty(): void {
    this.editorEmpty.hidden = this.ws.tabs.length > 0;
    this.editorHost.classList.toggle("hidden", this.ws.tabs.length === 0);
  }

  private renderPreviewPlaceholder(): void {
    const ws = this.ws;
    const has = !!ws.pdfPath;
    this.previewPlaceholder.hidden = has;
    if (has) return;
    this.previewPlaceholder.textContent = "";
    if (ws.isCompiling) {
      this.previewPlaceholder.append(h("span", { class: "spinner large" }), h("div", { class: "empty-detail", text: t("Compiling…") }));
      return;
    }
    const failure = ws.compileState.kind === "toolError" ? ws.compileState.message : ws.compileState.kind === "failed" ? t("The last build failed. See the Issues panel for details.") : null;
    this.previewPlaceholder.append(icon("docRich", 40),
      h("div", { class: "empty-title", text: t("No PDF yet") }),
      h("div", { class: "empty-detail", text: failure ?? t("Press ⌘B to compile") }));
  }

  // ---- toggles -----------------------------------------------------------------------------------------------

  toggleSidebar(): void {
    settings.set("sidebarVisible", !settings.get("sidebarVisible"));
  }

  togglePreview(): void {
    if (this.ws.pdfOnly) { this.togglePdfOnly(); return; }
    settings.set("showPreview", !settings.get("showPreview"));
  }

  togglePdfOnly(): void {
    this.ws.pdfOnly = !this.ws.pdfOnly;
    this.applyLayout();
    this.ws.events.emit("layout");
    if (!this.ws.pdfOnly) this.ws.editor?.focus();
  }

  toggleIssues(): void {
    settings.set("showIssues", !settings.get("showIssues"));
  }

  // ---- closing -----------------------------------------------------------------------------------------------

  /** Closes a tab, asking about unsaved changes first. */
  async closeTab(path: string): Promise<void> {
    if (this.ws.isDirty(path)) {
      const answer = await alertDialog({
        title: t("Do you want to save the changes you made to “%@”?", basename(path)),
        message: t("Your changes will be lost if you don’t save them."),
        buttons: [
          { title: t("Save"), role: "default", value: "save" },
          { title: t("Don’t Save"), role: "destructive", value: "discard" },
          { title: t("Cancel"), role: "cancel", value: "cancel" },
        ],
      });
      if (answer === "cancel") return;
      if (answer === "save" && !(await this.ws.save(path, { compile: false }))) return;
    }
    this.ws.closeTabNow(path);
  }

  async closeOthers(keep: string): Promise<void> {
    const others = this.ws.tabs.filter((p) => p !== keep);
    if (!(await this.confirmDiscard(others))) return;
    for (const p of others) this.ws.closeTabNow(p);
  }

  /** Asks to save the unsaved files among `paths` (all by default). False = cancelled. */
  async confirmDiscard(paths: string[] = this.ws.tabs): Promise<boolean> {
    const dirty = paths.filter((p) => this.ws.isDirty(p));
    if (!dirty.length) return true;
    const names = formatList(dirty.map((p) => `“${basename(p)}”`));
    const answer = await alertDialog({
      title: dirty.length === 1 ? t("Do you want to save the changes you made to “%@”?", basename(dirty[0]!)) : t("Do you want to save the changes you made to %@?", names),
      message: t("Your changes will be lost if you don’t save them."),
      buttons: [
        { title: dirty.length === 1 ? t("Save") : t("Save All"), role: "default", value: "save" },
        { title: t("Don’t Save"), role: "destructive", value: "discard" },
        { title: t("Cancel"), role: "cancel", value: "cancel" },
      ],
    });
    if (answer === "cancel") return false;
    if (answer === "save") {
      for (const p of dirty) if (!(await this.ws.save(p, { compile: false }))) return false;
    } else {
      for (const p of dirty) this.ws.editor?.markSaved(p, undefined);
    }
    return true;
  }

  async newFile(): Promise<void> {
    const name = await promptDialog({
      title: t("New File"), message: t("Enter a name for the new file. “.tex” is added if you leave out an extension."),
      label: t("Name"), confirm: t("Create"),
    });
    if (name) await this.ws.createFile(name);
  }

  async goToLine(): Promise<void> {
    const path = this.ws.current;
    if (!path) return;
    const text = await promptDialog({ title: t("Go to Line"), message: t("Enter a line number."), label: t("Line"), confirm: t("Go") });
    if (text === null) return;
    const line = Number.parseInt(text.trim(), 10);
    if (!Number.isFinite(line) || line <= 0) { this.ws.flash(t("Enter a positive line number.")); return; }
    this.ws.editor?.reveal(path, line, undefined, true);
    this.ws.editor?.focus();
  }

  quickOpen(): void {
    openQuickOpen(this.ws, (p) => void this.ws.open(p).then(() => this.ws.editor?.focus()));
  }

  /** Smoothly reveals the view (skipped with Reduce Motion). */
  appear(): void {
    if (reducedMotion()) return;
    this.el.classList.add("appearing");
    requestAnimationFrame(() => requestAnimationFrame(() => this.el.classList.remove("appearing")));
  }

  destroy(): void {
    for (const c of this.cleanups.splice(0)) c();
    this.ws.dispose();
    this.el.remove();
  }
}

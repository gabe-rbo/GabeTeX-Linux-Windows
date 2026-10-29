// The application shell (port of AppRouter + Commands): shows the Welcome view or one project, routes keyboard
// shortcuts to the command catalog, and implements `CommandHost` for menus-free command dispatch.
import * as ipc from "../ipc";
import type { Engine } from "../ipc";
import { t } from "../i18n";
import { bindLiveCompile, command, commands, type CommandHost, type EditorAction } from "./commands";
import { comboFromEvent, shortcuts } from "./shortcuts";
import { settings } from "./settings";
import { createWelcome } from "../ui/welcome";
import type { ProjectView } from "../ui/project";
import { openCommandPalette, closePalette, isPaletteOpen } from "../ui/palette";
import { alertDialog, isModalOpen, closeMenu } from "../ui/overlay";
import { openLabelEditor } from "../ui/labelEditor";
import { showStatistics } from "../ui/statistics";
import { dirname, basename } from "../ui/dom";

/** The default key CodeMirror uses for each editor action (replayed into the editor from the palette). */
const EDITOR_KEYS: Record<EditorAction, { key: string; code: string; shift?: boolean }> = {
  find: { key: "f", code: "KeyF" },
  replace: { key: "f", code: "KeyF" },
  toggleComment: { key: "/", code: "Slash" },
  indent: { key: "]", code: "BracketRight" },
  outdent: { key: "[", code: "BracketLeft" },
  complete: { key: " ", code: "Space" },
};

export class App implements CommandHost {
  private root: HTMLElement;
  private welcomeView: { destroy(): void; refresh(): void } | null = null;
  private project: ProjectView | null = null;

  constructor(root: HTMLElement) {
    this.root = root;
    bindLiveCompile(() => settings.get("liveCompile"), () => settings.set("liveCompile", !settings.get("liveCompile")));
    commands();
    document.addEventListener("keydown", (e) => this.onKey(e), true);
    document.addEventListener("contextmenu", (e) => {
      // No browser context menu outside text fields and the editor (the app has its own menus).
      const el = e.target as HTMLElement;
      if (!el.closest("input, textarea, .cm-editor, .log-view, .issue-detail")) e.preventDefault();
    });
    void this.listenForDrops();
  }

  async start(): Promise<void> {
    const params = new URLSearchParams(location.search);
    const project = params.get("project");
    if (project) await this.openProject(project);
    else this.showWelcome();
  }

  // ---- routing -----------------------------------------------------------------------------------------------

  private showWelcome(): void {
    this.project?.destroy();
    this.project = null;
    document.title = "GabeTeX";
    if (!this.welcomeView) {
      this.welcomeView = createWelcome(this.root, {
        newProject: () => this.newProject(),
        openFolder: () => this.openFolder(),
        openFile: () => this.openFile(),
        openProject: (p) => void this.openProject(p),
      });
    } else this.welcomeView.refresh();
  }

  /** Opens a project folder (optionally a file in it), replacing the current one after asking about unsaved work. */
  async openProject(path: string, file?: string): Promise<void> {
    if (this.project) {
      if (this.project.ws.root === path) {
        if (file) await this.project.ws.open(file);
        return;
      }
      if (!(await this.project.confirmDiscard())) return;
    }
    let info: ipc.ProjectInfo;
    try {
      info = await ipc.openProject(path);
    } catch (e) {
      await alertDialog({ title: t("Could not open “%@”", basename(path)), message: String(e) });
      return;
    }
    if (file) info = { ...info, rootFile: info.rootFile ?? file };
    this.project?.destroy();
    this.welcomeView?.destroy();
    this.welcomeView = null;
    const { ProjectView } = await import("../ui/project");
    const view = new ProjectView(info);
    this.project = view;
    this.root.appendChild(view.el);
    view.appear();
    await view.whenReady().catch((e) => console.error(e));
    if (file && this.project === view) await view.ws.open(file);
  }

  private async closeProject(): Promise<void> {
    if (this.project && !(await this.project.confirmDiscard())) return;
    this.showWelcome();
  }

  private async listenForDrops(): Promise<void> {
    try {
      const { getCurrentWebview } = await import("@tauri-apps/api/webview");
      await getCurrentWebview().onDragDropEvent((ev) => {
        if (ev.payload.type !== "drop") return;
        const first = ev.payload.paths[0];
        if (!first) return;
        if (/\.(tex|ltx|bib|sty|cls)$/i.test(first)) void this.openProject(dirname(first), first);
        else void this.openProject(first);
      });
    } catch {
      /* not running inside Tauri (vite dev in a browser) */
    }
  }

  // ---- keyboard ----------------------------------------------------------------------------------------------

  private onKey(e: KeyboardEvent): void {
    if (e.defaultPrevented || e.isComposing) return;
    const combo = comboFromEvent(e);
    if (!combo) return;
    if (isModalOpen()) return;
    const ids = shortcuts.commands(combo);
    if (!ids.length) return;
    const cmd = command(ids[0]!);
    if (!cmd) return;
    // Editor actions keep CodeMirror's own default key; only a custom shortcut is routed through the palette path.
    if (cmd.editorAction && combo === cmd.defaultShortcut && (e.target as HTMLElement).closest?.(".cm-editor")) return;
    // In Vim mode plain Ctrl+letter belongs to Vim inside the editor (Ctrl+D, Ctrl+O, Ctrl+W…); Ctrl+S, Ctrl+P and
    // shortcuts with Shift or Alt still reach the app.
    if (settings.get("vimMode") && /^Ctrl\+[A-Z\[\]]$/.test(combo) && combo !== "Ctrl+S" && combo !== "Ctrl+P" && (e.target as HTMLElement).closest?.(".cm-editor")) return;
    if (isPaletteOpen() && cmd.id !== "view.commandPalette" && cmd.id !== "view.quickOpen") return;
    // Leave plain text-field editing alone (Ctrl+A/C/V… are not commands; Ctrl+S etc. still work).
    if (!cmd.isEnabled(this)) { e.preventDefault(); return; }
    e.preventDefault();
    e.stopPropagation();
    closeMenu();
    cmd.run(this);
  }

  // ---- CommandHost: state ------------------------------------------------------------------------------------

  private get ws() { return this.project?.ws; }
  hasProject(): boolean { return !!this.project; }
  hasFile(): boolean { return !!this.ws?.current; }
  tabCount(): number { return this.ws?.tabs.length ?? 0; }
  canCompile(): boolean { return !!this.ws?.mainFile; }
  isCompiling(): boolean { return !!this.ws?.isCompiling; }
  engineOverride(): Engine | undefined { return this.ws?.engineOverride; }
  hasMainOverride(): boolean { return !!this.ws?.mainOverride; }
  hasPDF(): boolean { return !!this.ws?.pdfPath; }
  showPreview(): boolean { return settings.get("showPreview"); }
  isPdfOnly(): boolean { return !!this.ws?.pdfOnly; }
  showIssues(): boolean { return settings.get("showIssues"); }

  // ---- CommandHost: actions ----------------------------------------------------------------------------------

  newProject(): void {
    void import("../ui/newProject").then((m) => m.openNewProject((info) => void this.openProject(info.root, info.rootFile)));
  }

  newFile(): void { void this.project?.newFile(); }

  async openFolder(): Promise<void> {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, multiple: false, title: t("Choose a folder that contains your LaTeX project.") }).catch(() => null);
    if (typeof picked === "string") await this.openProject(picked);
  }

  async openFile(): Promise<void> {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: false, multiple: false, filters: [{ name: "LaTeX", extensions: ["tex", "ltx", "bib", "sty", "cls"] }] }).catch(() => null);
    if (typeof picked === "string") await this.openProject(dirname(picked), picked);
  }

  closeTab(): void {
    const p = this.ws?.current;
    if (p) void this.project?.closeTab(p);
  }

  save(): void { void this.ws?.saveCurrent(); }
  saveAll(): void { void this.ws?.saveAll({ compile: true }); }
  revealCurrent(): void { const p = this.ws?.current; if (p) this.ws?.reveal(p); }
  newProjectLabel(): void { const r = this.ws?.root; if (r) openLabelEditor({ assignTo: r }); }

  editorAction(action: EditorAction): void {
    const content = document.querySelector<HTMLElement>(".editor-host .cm-content");
    if (!content) return;
    content.focus();
    const k = EDITOR_KEYS[action];
    content.dispatchEvent(new KeyboardEvent("keydown", { key: k.key, code: k.code, ctrlKey: true, shiftKey: !!k.shift, bubbles: true, cancelable: true }));
  }

  goToLine(): void { void this.project?.goToLine(); }
  insertSnippet(snippet: string): void { this.ws?.editor?.focus(); this.ws?.editor?.insertSnippet(snippet); }
  togglePreview(): void { this.project?.togglePreview(); }
  togglePdfOnly(): void { this.project?.togglePdfOnly(); }
  toggleIssues(): void { this.project?.toggleIssues(); }
  quickOpen(): void { this.project?.quickOpen(); }

  commandPalette(): void {
    if (isPaletteOpen()) { closePalette(); return; }
    openCommandPalette(this);
  }

  showSidebar(mode: "files" | "outline"): void {
    if (!this.ws) return;
    if (!settings.get("sidebarVisible")) settings.set("sidebarVisible", true);
    this.ws.sidebarMode = mode;
    this.project?.el.querySelector<HTMLElement>(mode === "files" ? ".segmented .seg:first-child" : ".segmented .seg:last-child")?.click();
  }

  documentStatistics(): void {
    if (this.ws) showStatistics(this.ws, this.project?.el.querySelector<HTMLElement>(".statusbar .stats") ?? undefined);
  }

  cycleTab(delta: number): void { this.ws?.cycleTab(delta); }
  selectTab(index: number): void { this.ws?.selectTabAt(index); }
  compile(): void { this.ws?.compile(); }
  stop(): void { this.ws?.stopCompile(); }
  setEngine(engine: Engine | undefined): void { this.ws?.setEngineOverride(engine); }
  setMainDocument(): void { const p = this.ws?.current; if (p) this.ws?.setMainFile(p); }
  automaticMainDocument(): void { this.ws?.setMainFile(undefined); }

  showInPDF(): void {
    const ws = this.ws;
    if (ws?.current) void ws.forwardSync(ws.current, ws.cursor.line, ws.cursor.column);
  }

  revealPDF(): void { const p = this.ws?.pdfPath; if (p) this.ws?.reveal(p); }
  nextIssue(forward: boolean): void { this.ws?.goToNextIssue(forward); }
  welcome(): void { void this.closeProject(); }
  openSettings(pane?: string): void { void import("../ui/settingsView").then((m) => m.openSettings(pane)); }
}

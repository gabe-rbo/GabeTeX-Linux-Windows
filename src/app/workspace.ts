// One open project (port of ProjectWorkspace and its extensions): files and tabs, buffers and saving, the main
// document, compile orchestration (explicit builds and live compile while typing), diagnostics, the project index,
// external file changes, missing packages and SyncTeX glue. Talks to the backend only through ../ipc.
import * as ipc from "../ipc";
import type { CompileResult, Diagnostic, Encoding, Engine, FileNode, ProjectIndex, ProjectInfo, TeXDistribution } from "../ipc";
import type { EditorHandle } from "../editor";
import type { PreviewHandle } from "../preview";
import { t, formatList } from "../i18n";
import { Emitter } from "./emitter";
import { settings } from "./settings";
import { basename, extname, relativePath, joinPath, dirname } from "../ui/dom";

export type Topic =
  | "files" | "tabs" | "dirty" | "main" | "compile" | "diagnostics" | "index" | "pdf" | "banners" | "status"
  | "cursor" | "layout" | "engine" | "tex" | "log";

export type CompileState =
  | { kind: "idle" }
  | { kind: "compiling"; purpose: Purpose }
  | { kind: "succeeded" }
  | { kind: "failed" }
  | { kind: "toolError"; message: string };

type Purpose = "build" | "live";

export type PackagePhase =
  | { kind: "idle" }
  | { kind: "offer"; files: string[] }
  | { kind: "installing"; files: string[] };

export type ExternalChange = "modified" | "deleted";

/** Files the editor opens as text. Everything else opens in the default app. */
const TEXT_EXTENSIONS = new Set(["tex", "ltx", "sty", "cls", "bib", "bst", "bbx", "cbx", "dtx", "ins", "txt", "md", "def", "cfg", "clo", "fd", "latexmkrc", "tikz", "pgf", "csv", "dat", "lua", "py", "json", "yaml", "yml", "xml", "bbl", "log", "tsv", ""]);
export const isTextFile = (path: string): boolean => TEXT_EXTENSIONS.has(extname(path)) || basename(path).startsWith(".");

export class Workspace {
  readonly events = new Emitter<Topic>();
  readonly root: string;
  readonly name: string;
  files: FileNode[];
  /** Every file in the tree (depth first), absolute paths. */
  flatFiles: string[] = [];

  tabs: string[] = [];
  current: string | undefined;
  private encodings = new Map<string, Encoding>();
  /** Text last read from / written to disk, per open file (tells our own writes from external ones). */
  private diskText = new Map<string, string>();
  private dirty = new Set<string>();

  editor: EditorHandle | undefined;
  preview: PreviewHandle | undefined;

  private detectedRoot: string | undefined;
  index: ProjectIndex | null = null;
  tex: TeXDistribution | null | undefined = undefined;

  compileState: CompileState = { kind: "idle" };
  lastDurationMs: number | undefined;
  hasCompiled = false;
  compileDiagnostics: Diagnostic[] = [];
  diagnostics: Diagnostic[] = [];
  logPath: string | undefined;
  toolOutput = "";
  pdfPath: string | undefined;
  pageCount = 0;
  page = 0;
  packages: PackagePhase = { kind: "idle" };
  dismissedMissing = new Set<string>();
  externalChanges = new Map<string, ExternalChange>();
  statusMessage: string | undefined;
  cursor = { line: 1, column: 1 };
  selectedIssue = -1;

  pdfOnly = false;
  /** The preview lives in its own window (src/app/pdfLink.ts); `preview` then forwards to it. */
  pdfDetached = false;
  sidebarMode: "files" | "outline";

  private generation = 0;
  private inflight: string | undefined;
  private rerunLive = false;
  private timers = new Map<string, number>();
  private unlisten: (() => void) | undefined;
  private stopped = false;
  private statusTimer = 0;

  constructor(info: ProjectInfo) {
    this.root = info.root;
    this.name = info.name;
    this.files = info.files;
    this.detectedRoot = info.rootFile;
    this.flatFiles = flatten(info.files);
    this.sidebarMode = settings.project(this.root).sidebar ?? "files";
  }

  // ---- lifecycle ---------------------------------------------------------------------------------------------

  async start(): Promise<void> {
    void ipc.addRecentProject(this.root, this.name).catch(() => {});
    try {
      await ipc.watchProject(this.root);
      this.unlisten = await ipc.onFsChanged((e) => this.fsChanged(e.paths));
    } catch (e) {
      console.warn("File watching unavailable", e);
    }
    ipc.detectTex().then((d) => { this.tex = d; this.events.emit("tex"); }).catch(() => { this.tex = null; this.events.emit("tex"); });
    await this.refreshIndex();
  }

  dispose(): void {
    this.stopped = true;
    for (const id of this.timers.values()) clearTimeout(id);
    this.unlisten?.();
    void ipc.unwatchProject(this.root).catch(() => {});
    if (this.isCompiling) void ipc.cancelCompile(this.root).catch(() => {});
    settings.updateProject(this.root, (m) => { m.tabs = this.tabs.slice(); m.current = this.current; m.sidebar = this.sidebarMode; });
    this.events.clear();
  }

  // ---- helpers -----------------------------------------------------------------------------------------------

  private debounce(key: string, ms: number, fn: () => void): void {
    clearTimeout(this.timers.get(key));
    this.timers.set(key, window.setTimeout(() => { this.timers.delete(key); if (!this.stopped) fn(); }, ms));
  }

  /** A short message in the status bar (Mac `flash`). */
  flash(message: string): void {
    this.statusMessage = message;
    this.events.emit("status");
    clearTimeout(this.statusTimer);
    this.statusTimer = window.setTimeout(() => { this.statusMessage = undefined; this.events.emit("status"); }, 3500);
  }

  relative(path: string): string {
    return relativePath(path, this.root);
  }

  // ---- main document and engine ------------------------------------------------------------------------------

  get mainOverride(): string | undefined {
    return settings.project(this.root).mainFile;
  }

  get mainFile(): string | undefined {
    return this.mainOverride ?? this.index?.rootFile ?? this.detectedRoot;
  }

  isMainFile(path: string): boolean {
    return this.mainFile === path;
  }

  setMainFile(path: string | undefined): void {
    settings.updateProject(this.root, (m) => { if (path) m.mainFile = path; else delete m.mainFile; });
    this.events.emit("main");
    this.startCompile("live", { force: true });
  }

  get engineOverride(): Engine | undefined {
    return settings.project(this.root).engine;
  }

  setEngineOverride(engine: Engine | undefined): void {
    settings.updateProject(this.root, (m) => { if (engine) m.engine = engine; else delete m.engine; });
    this.events.emit("engine");
    this.scheduleLiveCompile();
  }

  /** Override › `% !TEX program = …` in the main file › the default engine setting. */
  get effectiveEngine(): Engine {
    if (this.engineOverride) return this.engineOverride;
    const main = this.mainFile;
    const text = main ? this.editor?.getText(main) : undefined;
    const magic = text !== undefined ? magicProgram(text) : this.mainMagic;
    return magic ?? settings.get("defaultEngine");
  }
  /** The main file's magic program when it is not open in the editor (read at index time). */
  private mainMagic: Engine | undefined;

  // ---- files and tabs ----------------------------------------------------------------------------------------

  async refreshFiles(): Promise<void> {
    try {
      this.files = await ipc.listFiles(this.root);
      this.flatFiles = flatten(this.files);
      this.events.emit("files");
    } catch (e) {
      console.warn(e);
    }
  }

  isDirty(path: string): boolean {
    return this.dirty.has(path);
  }

  get dirtyPaths(): string[] {
    return [...this.dirty];
  }

  encodingOf(path: string): Encoding | undefined {
    return this.encodings.get(path);
  }

  /** Opens a file in a tab (or selects its tab) and shows it in the editor. */
  async open(path: string, opts: { line?: number; column?: number; flash?: boolean } = {}): Promise<boolean> {
    let isNew = false;
    if (!this.tabs.includes(path)) {
      if (!isTextFile(path)) {
        void ipc.openInDefaultApp(path).catch(() => ipc.revealInFileManager(path).catch(() => {}));
        return false;
      }
      let content: ipc.FileContent;
      try {
        content = await ipc.readFile(path);
      } catch (e) {
        this.flash(t("Could not open “%@”", basename(path)) + " " + String(e));
        return false;
      }
      if (!this.tabs.includes(path)) {
        isNew = true;
        this.encodings.set(path, content.encoding);
        this.diskText.set(path, content.text);
        const at = this.current ? this.tabs.indexOf(this.current) + 1 : this.tabs.length;
        this.tabs.splice(at, 0, path);
      }
    }
    if (this.current !== path || isNew) {
      this.current = path;
      this.editor?.open(path, this.editor.getText(path) ?? this.diskText.get(path) ?? "");
      if (isNew) this.pushDiagnostics(path);
      this.events.emit("tabs");
    }
    if (opts.line) this.editor?.reveal(path, opts.line, opts.column, opts.flash ?? false);
    return true;
  }

  select(path: string): void {
    void this.open(path);
  }

  selectTabAt(index: number): void {
    const p = this.tabs[index];
    if (p) this.select(p);
  }

  cycleTab(delta: number): void {
    if (this.tabs.length < 2 || !this.current) return;
    const i = this.tabs.indexOf(this.current);
    this.selectTabAt((i + delta + this.tabs.length) % this.tabs.length);
  }

  moveTab(path: string, before: string | undefined): void {
    const from = this.tabs.indexOf(path);
    if (from < 0 || path === before) return;
    this.tabs.splice(from, 1);
    const to = before ? this.tabs.indexOf(before) : -1;
    this.tabs.splice(to < 0 ? this.tabs.length : to, 0, path);
    this.events.emit("tabs");
  }

  /** Closes a tab without asking (the view asks about unsaved changes first). */
  closeTabNow(path: string): void {
    const i = this.tabs.indexOf(path);
    if (i < 0) return;
    this.tabs.splice(i, 1);
    this.editor?.close(path);
    this.dirty.delete(path);
    this.diskText.delete(path);
    this.encodings.delete(path);
    if (this.externalChanges.delete(path)) this.events.emit("banners");
    if (this.current === path) {
      this.current = undefined;
      const next = this.tabs[Math.min(i, this.tabs.length - 1)];
      if (next) this.select(next);
    }
    this.events.emit("tabs");
    this.events.emit("dirty");
  }

  /** Creates an empty file (adds “.tex” when there is no extension) and opens it. */
  async createFile(name: string, inDir?: string): Promise<void> {
    let clean = name.trim().replace(/^[/\\]+/, "");
    if (!clean) return;
    if (!/\.[^./\\]+$/.test(clean)) clean += ".tex";
    const path = joinPath(inDir ?? this.root, clean);
    if (this.flatFiles.includes(path)) {
      this.flash(t("“%@” already exists", clean));
      return;
    }
    try {
      await ipc.writeFile(path, "", "utf-8");
    } catch (e) {
      this.flash(t("Could not create “%@”", clean) + " " + String(e));
      return;
    }
    await this.refreshFiles();
    await this.open(path);
    this.editor?.focus();
  }

  // ---- file operations (sidebar; Mac ProjectWorkspace+Files) ---------------------------------------------------

  /** An alert for a failed file operation (Mac `present`). Backend messages are English; known ones are localized. */
  private async fileError(e: unknown, title: string, name: string): Promise<void> {
    const msg = String(e);
    let head = title;
    let detail = msg;
    if (/already exists$/.test(msg)) { head = t("“%@” already exists", name); detail = t("Choose a different name."); }
    else if (/^Names cannot be empty/.test(msg)) { head = t("Invalid name"); detail = t("Names cannot be empty or contain “/”."); }
    await this.ask({ title: head, message: detail, buttons: [{ title: t("OK"), role: "default", value: "ok" }] });
  }

  private static under(p: string, base: string): boolean {
    return p === base || p.startsWith(base + "/") || p.startsWith(base + "\\");
  }

  /** Folder used as the target of New File / New Folder for a selected path. */
  directoryOf(path: string, isDir: boolean): string {
    return isDir ? path : dirname(path);
  }

  async createFolder(name: string, inDir?: string): Promise<void> {
    try {
      await ipc.createFolder(inDir ?? this.root, name);
    } catch (e) {
      await this.fileError(e, t("Could not create “%@”", name.trim()), name.trim());
      return;
    }
    await this.refreshFiles();
  }

  /** Renames a file or folder; open tabs follow it. Unsaved edits of affected files are saved first. */
  async renameItem(path: string, name: string): Promise<boolean> {
    const old = basename(path);
    if (name.trim() === old) return true;
    for (const p of this.tabs.filter((x) => Workspace.under(x, path))) {
      if (this.dirty.has(p) && !(await this.save(p, { compile: false }))) return false;
    }
    let to: string;
    try {
      to = await ipc.renamePath(path, name);
    } catch (e) {
      await this.fileError(e, t("Could not rename “%@”", old), name.trim());
      return false;
    }
    if (to === path) return true;
    this.remapPaths(path, to);
    await this.refreshFiles();
    void this.refreshIndex();
    this.scheduleLiveCompile();
    return true;
  }

  /** Updates tabs, buffers and the main-document choice after `from` moved to `to` on disk. */
  private remapPaths(from: string, to: string): void {
    const map = (p: string): string | undefined => (Workspace.under(p, from) ? to + p.slice(from.length) : undefined);
    const wasCurrent = this.current;
    for (let i = 0; i < this.tabs.length; i++) {
      const old = this.tabs[i]!;
      const neu = map(old);
      if (!neu) continue;
      this.tabs[i] = neu;
      const disk = this.diskText.get(old);
      this.diskText.delete(old);
      if (disk !== undefined) this.diskText.set(neu, disk);
      const enc = this.encodings.get(old);
      this.encodings.delete(old);
      if (enc) this.encodings.set(neu, enc);
      this.dirty.delete(old);
      if (this.externalChanges.delete(old)) this.events.emit("banners");
      this.editor?.close(old);
      if (old === wasCurrent) {
        this.current = neu;
        this.editor?.open(neu, disk ?? "");
        this.pushDiagnostics(neu);
      }
    }
    const main = settings.project(this.root).mainFile;
    const newMain = main ? map(main) : undefined;
    if (newMain) settings.updateProject(this.root, (m) => { m.mainFile = newMain; });
    const detected = this.detectedRoot && map(this.detectedRoot);
    if (detected) this.detectedRoot = detected;
    this.events.emit("tabs");
    this.events.emit("dirty");
    this.events.emit("main");
  }

  /** Copies next to the original (“name copy.ext”) and opens the copy when it is a file. */
  async duplicateItem(path: string, isDir: boolean): Promise<void> {
    let copy: string;
    try {
      copy = await ipc.duplicatePath(path);
    } catch (e) {
      await this.fileError(e, t("Could not duplicate “%@”", basename(path)), basename(path));
      return;
    }
    await this.refreshFiles();
    if (!isDir) await this.open(copy);
  }

  /** Move to Trash. Asks first when files have unsaved edits (or when `confirm` is set); false = not trashed. */
  async trashItem(path: string, opts: { confirm?: boolean } = {}): Promise<boolean> {
    const name = basename(path);
    const dirty = this.tabs.filter((p) => Workspace.under(p, path) && this.dirty.has(p));
    const cancel = { title: t("Cancel"), role: "cancel" as const, value: "cancel" };
    if (dirty.length) {
      const answer = await this.ask({
        title: t("Move “%@” to the Trash?", name),
        message: t("Unsaved changes in %@ will be lost if you don’t save them.", formatList(dirty.map((p) => `“${basename(p)}”`))),
        buttons: [
          { title: t("Save and Move to Trash"), role: "default", value: "save" },
          { title: t("Discard Changes and Move to Trash"), role: "destructive", value: "discard" },
          cancel,
        ],
      });
      if (answer === "cancel") return false;
      if (answer === "save") for (const p of dirty) if (!(await this.save(p, { compile: false }))) return false;
    } else if (opts.confirm) {
      const answer = await this.ask({
        title: t("Move “%@” to the Trash?", name),
        buttons: [{ title: t("Move to Trash"), role: "destructive", value: "trash" }, cancel],
      });
      if (answer !== "trash") return false;
    }
    try {
      await ipc.moveToTrash(path);
    } catch (e) {
      await this.fileError(e, t("Could not move “%@” to the Trash", name), name);
      return false;
    }
    for (const p of this.tabs.filter((x) => Workspace.under(x, path))) this.closeTabNow(p);
    const main = settings.project(this.root).mainFile;
    if (main && Workspace.under(main, path)) this.setMainFile(undefined);
    await this.refreshFiles();
    void this.refreshIndex();
    this.scheduleLiveCompile();
    return true;
  }

  copyPath(path: string): void {
    void navigator.clipboard.writeText(path).then(() => this.flash(t("Copy Path"))).catch(() => {});
  }

  /** Mac `cleanAuxiliaryFiles`: drops the build workspace (never project files), then rebuilds from scratch. */
  async cleanAuxiliaryFiles(): Promise<void> {
    this.stopCompile();
    try {
      await ipc.cleanAuxiliaryFiles(this.root);
    } catch (e) {
      this.flash(String(e));
      return;
    }
    this.compileDiagnostics = [];
    this.toolOutput = "";
    this.rebuildDiagnostics();
    this.flash(t("Auxiliary files removed"));
    this.startCompile("live", { force: true });
    void this.refreshFiles();
  }

  // ---- editing and saving ------------------------------------------------------------------------------------

  /** Called by the editor on every change. */
  didChange(path: string): void {
    const wasDirty = this.dirty.has(path);
    const isDirty = this.editor?.isDirty(path) ?? true;
    if (isDirty) this.dirty.add(path); else this.dirty.delete(path);
    if (wasDirty !== isDirty) this.events.emit("dirty");
    this.scheduleLiveCompile();
    this.debounce("index", 700, () => void this.refreshIndex());
  }

  didMoveCursor(line: number, column: number): void {
    this.cursor = { line, column };
    this.events.emitSoon("cursor");
    if (settings.get("followCursorInPDF") && this.current) {
      const path = this.current;
      this.debounce("follow", 450, () => void this.forwardSync(path, line, column, true));
    }
  }

  /** Saves one file atomically. Returns false when it failed (an alert was shown). */
  async save(path: string, opts: { compile?: boolean } = {}): Promise<boolean> {
    const text = this.editor?.getText(path);
    if (text === undefined) return true;
    const encoding = this.encodings.get(path) ?? "utf-8";
    try {
      await ipc.writeFile(path, text, encoding);
    } catch (e) {
      const message = String(e);
      if (encoding === "latin-1" && /encod|represent|charact/i.test(message)) {
        const answer = await this.ask({
          title: t("Could not save “%@”", basename(path)),
          message: t("The text contains characters that cannot be stored in the file’s %@ encoding. You can save it as UTF-8 instead.", "ISO Latin 1"),
          buttons: [{ title: t("Save as UTF-8"), role: "default", value: "utf8" }, { title: t("Cancel"), role: "cancel", value: "cancel" }],
        });
        if (answer !== "utf8") return false;
        this.encodings.set(path, "utf-8");
        this.events.emit("status");
        return this.save(path, opts);
      }
      await this.ask({ title: t("Could not save “%@”", basename(path)), message, buttons: [{ title: t("OK"), role: "default", value: "ok" }] });
      return false;
    }
    this.diskText.set(path, text);
    this.editor?.markSaved(path, text);
    this.dirty.delete(path);
    if (this.externalChanges.delete(path)) this.events.emit("banners");
    this.events.emit("dirty");
    if (opts.compile !== false && settings.get("compileOnSave")) this.startCompile("build", { saveFirst: false });
    return true;
  }

  async saveCurrent(): Promise<void> {
    if (this.current) await this.save(this.current);
  }

  /** Saves every unsaved buffer. False when one failed. */
  async saveAll(opts: { compile?: boolean } = {}): Promise<boolean> {
    for (const path of [...this.dirty]) {
      if (!(await this.save(path, { compile: false }))) return false;
    }
    if (opts.compile && settings.get("compileOnSave")) this.startCompile("build", { saveFirst: false });
    return true;
  }

  /** Discards unsaved changes of a file (reloads it from disk). */
  async revert(path: string): Promise<void> {
    await this.reloadFromDisk(path);
  }

  /** Alert hook, set by the view. */
  ask: (opts: { title: string; message?: string; buttons: Array<{ title: string; role?: "cancel" | "destructive" | "default"; value: string }> }) => Promise<string> = async () => "cancel";

  // ---- external changes --------------------------------------------------------------------------------------

  private fsChanged(paths: string[]): void {
    let treeChanged = false;
    const rootNorm = this.root.replace(/\\/g, "/");
    for (const p of paths) {
      const n = p.replace(/\\/g, "/");
      if (!n.startsWith(rootNorm)) continue;
      if (/\.(aux|log|synctex(\.gz)?|fls|fdb_latexmk|out|toc|bbl|blg|bcf|run\.xml|pdf)$/i.test(n)) continue;
      treeChanged = true;
      if (this.tabs.includes(p)) this.debounce("ext:" + p, 120, () => void this.checkExternal(p));
    }
    if (treeChanged) {
      this.debounce("tree", 250, () => void this.refreshFiles());
      this.debounce("index", 400, () => void this.refreshIndex());
    }
  }

  private async checkExternal(path: string): Promise<void> {
    let content: ipc.FileContent | null = null;
    try {
      content = await ipc.readFile(path);
    } catch {
      content = null;
    }
    if (!this.tabs.includes(path)) return;
    if (!content) {
      this.externalChanges.set(path, "deleted");
      this.events.emit("banners");
      return;
    }
    if (content.text === this.diskText.get(path)) return; // our own save, or no real change
    const current = this.editor?.getText(path);
    if (current === content.text) {
      this.diskText.set(path, content.text);
      this.editor?.markSaved(path, content.text);
      return;
    }
    if (!this.dirty.has(path)) {
      this.applyDiskText(path, content);
      this.flash(t("“%@” was reloaded", basename(path)));
      return;
    }
    this.externalChanges.set(path, "modified");
    this.events.emit("banners");
  }

  private applyDiskText(path: string, content: ipc.FileContent): void {
    this.encodings.set(path, content.encoding);
    this.diskText.set(path, content.text);
    this.editor?.close(path);
    this.editor?.open(path, content.text);
    if (this.current && this.current !== path) this.editor?.open(this.current, this.editor.getText(this.current) ?? "");
    this.dirty.delete(path);
    this.pushDiagnostics(path);
    this.events.emit("dirty");
    this.scheduleLiveCompile();
  }

  async reloadFromDisk(path: string): Promise<void> {
    try {
      const content = await ipc.readFile(path);
      this.applyDiskText(path, content);
    } catch (e) {
      this.flash(t("Could not reload “%@”", basename(path)) + " " + String(e));
    }
    this.externalChanges.delete(path);
    this.events.emit("banners");
  }

  keepMine(path: string): void {
    this.externalChanges.delete(path);
    this.diskText.set(path, "\u0000keep-mine"); // the next save is ours; a further external change asks again
    this.dirty.add(path);
    this.events.emit("dirty");
    this.events.emit("banners");
  }

  // ---- index -------------------------------------------------------------------------------------------------

  overrides(): Record<string, string> {
    const out: Record<string, string> = {};
    for (const p of this.dirty) {
      const text = this.editor?.getText(p);
      if (text !== undefined) out[p] = text;
    }
    return out;
  }

  async refreshIndex(): Promise<void> {
    try {
      const index = await ipc.projectIndex(this.root, this.overrides());
      if (this.stopped) return;
      const mainBefore = this.mainFile;
      this.index = index;
      this.rebuildDiagnostics();
      this.events.emit("index");
      if (this.mainFile !== mainBefore) this.events.emit("main");
      const main = this.mainFile;
      if (main && this.editor?.getText(main) === undefined) {
        ipc.readFile(main).then((c) => { this.mainMagic = magicProgram(c.text); this.events.emit("engine"); }).catch(() => {});
      } else {
        this.events.emit("engine");
      }
    } catch (e) {
      console.warn("Index failed", e);
    }
  }

  // ---- compile -----------------------------------------------------------------------------------------------

  get isCompiling(): boolean {
    return this.compileState.kind === "compiling";
  }

  /** Ctrl+B / toolbar: save everything, then build. */
  compile(): void {
    this.startCompile("build", { explicit: true });
  }

  /** Debounced compile while typing (no-op when live compile is off). */
  scheduleLiveCompile(): void {
    if (!settings.get("liveCompile")) return;
    this.debounce("live", settings.get("liveDelay") * 1000, () => this.startCompile("live"));
  }

  /**
   * Starts a compile. A live compile never interrupts a running compile of the same root and engine: it finishes and
   * exactly one more live compile follows. Changing the root or engine, `force`, an explicit build and Stop cancel.
   */
  startCompile(purpose: Purpose, opts: { force?: boolean; explicit?: boolean; saveFirst?: boolean } = {}): void {
    if (this.stopped) return;
    clearTimeout(this.timers.get("live"));
    if (purpose === "live" && !opts.force && !settings.get("liveCompile")) return;
    const root = this.mainFile;
    if (!root) {
      if (opts.explicit) {
        void this.ask({
          title: t("Nothing to compile"),
          message: t("Open a .tex file that contains \\documentclass, or choose one with Set as Main Document."),
          buttons: [{ title: t("OK"), role: "default", value: "ok" }],
        });
      }
      return;
    }
    if (purpose === "live" && this.compileState.kind === "compiling" && this.compileState.purpose === "build") {
      this.rerunLive = true;
      return;
    }
    const engine = this.effectiveEngine;
    const signature = `${root}|${engine}`;
    if (purpose === "live" && !opts.force && this.isCompiling && this.inflight === signature) {
      this.rerunLive = true;
      return;
    }
    void this.runCompile(purpose, root, engine, signature, opts.saveFirst ?? purpose === "build");
  }

  private async runCompile(purpose: Purpose, root: string, engine: Engine, signature: string, saveFirst: boolean): Promise<void> {
    if (saveFirst && !(await this.saveAll({ compile: false }))) return;
    const generation = ++this.generation;
    this.inflight = signature;
    this.rerunLive = false;
    this.compileState = { kind: "compiling", purpose };
    this.events.emit("compile");
    let result: CompileResult | null = null;
    let toolError: string | null = null;
    try {
      result = await ipc.compile(this.root, root, engine, this.overrides());
    } catch (e) {
      toolError = String(e);
    }
    if (generation !== this.generation || this.stopped) return;
    this.finishCompile(purpose, result, toolError);
  }

  private finishCompile(purpose: Purpose, result: CompileResult | null, toolError: string | null): void {
    this.inflight = undefined;
    this.hasCompiled = true;
    if (!result) {
      this.compileState = { kind: "toolError", message: toolError ?? t("Cannot compile") };
      this.toolOutput = toolError ?? "";
      this.compileDiagnostics = [];
      if (toolError && /No TeX distribution/i.test(toolError)) { this.tex = null; this.events.emit("tex"); }
    } else {
      this.lastDurationMs = result.durationMs;
      this.logPath = result.logPath;
      this.compileDiagnostics = result.diagnostics;
      const hasErrors = result.diagnostics.some((d) => d.severity === "error");
      this.compileState = result.ok || (!hasErrors && result.pdfPath) ? { kind: "succeeded" } : { kind: "failed" };
      if (result.pdfPath) {
        this.pdfPath = result.pdfPath;
        void this.preview?.load(result.pdfPath).catch((e) => console.warn("PDF load failed", e));
        this.events.emit("pdf");
      }
      const missing = result.missingPackages.filter((f) => !this.dismissedMissing.has(f));
      if (missing.length && this.packages.kind === "idle") {
        this.packages = { kind: "offer", files: missing };
        this.events.emit("banners");
      }
      if (purpose === "build" && settings.get("syncAfterBuild") && this.pdfPath && this.current) {
        void this.forwardSync(this.current, this.cursor.line, this.cursor.column, true);
      }
    }
    this.rebuildDiagnostics();
    this.events.emit("compile");
    this.events.emit("log");
    if (this.rerunLive) {
      this.rerunLive = false;
      this.startCompile("live");
    }
  }

  stopCompile(): void {
    if (!this.isCompiling) return;
    void ipc.cancelCompile(this.root).catch(() => {});
    this.generation++;
    this.inflight = undefined;
    this.rerunLive = false;
    this.compileState = { kind: "idle" };
    this.events.emit("compile");
  }

  liveCompileSettingChanged(): void {
    if (settings.get("liveCompile")) this.startCompile("live");
    else clearTimeout(this.timers.get("live"));
  }

  // ---- diagnostics -------------------------------------------------------------------------------------------

  private rebuildDiagnostics(): void {
    const seen = new Set<string>();
    const out: Diagnostic[] = [];
    for (const d of [...this.compileDiagnostics, ...(this.index?.warnings ?? [])]) {
      const k = `${d.severity}|${d.path}|${d.line}|${d.message}`;
      if (seen.has(k)) continue;
      seen.add(k);
      out.push(d);
    }
    this.diagnostics = out;
    for (const p of this.tabs) this.pushDiagnostics(p);
    this.events.emit("diagnostics");
  }

  private pushDiagnostics(path: string): void {
    this.editor?.setDiagnostics(path, this.diagnostics.filter((d) => d.path === path));
  }

  get errorCount(): number {
    return this.diagnostics.filter((d) => d.severity === "error").length;
  }

  get warningCount(): number {
    return this.diagnostics.filter((d) => d.severity === "warning").length;
  }

  get otherCount(): number {
    return this.diagnostics.filter((d) => d.severity === "info").length;
  }

  goToIssue(d: Diagnostic): void {
    if (!d.path) return;
    void this.open(d.path, { line: Math.max(1, d.line), column: d.column, flash: true }).then(() => this.editor?.focus());
  }

  /** Next / previous error or warning (wraps). */
  goToNextIssue(forward: boolean): void {
    const list = this.diagnostics.filter((d) => d.severity !== "info");
    if (!list.length) { this.flash(t("No Issues")); return; }
    this.selectedIssue = (this.selectedIssue + (forward ? 1 : -1) + list.length) % list.length;
    const d = list[this.selectedIssue]!;
    this.goToIssue(d);
    this.events.emit("diagnostics");
  }

  // ---- packages ----------------------------------------------------------------------------------------------

  dismissPackages(): void {
    if (this.packages.kind === "offer") for (const f of this.packages.files) this.dismissedMissing.add(f);
    this.packages = { kind: "idle" };
    this.events.emit("banners");
  }

  async installPackages(): Promise<void> {
    if (this.packages.kind !== "offer") return;
    const files = this.packages.files;
    this.packages = { kind: "installing", files };
    this.events.emit("banners");
    let result: ipc.InstallResult;
    try {
      result = await ipc.installPackages(files);
    } catch (e) {
      result = { ok: false, output: String(e) };
    }
    this.packages = { kind: "idle" };
    this.events.emit("banners");
    if (result.ok) {
      this.flash(t("Installed %@", formatList(files)));
      this.startCompile("live", { force: true });
    } else {
      for (const f of files) this.dismissedMissing.add(f);
      await this.ask({ title: t("Installation failed"), message: result.output.trim().slice(-1500), buttons: [{ title: t("OK"), role: "default", value: "ok" }] });
    }
  }

  // ---- SyncTeX -----------------------------------------------------------------------------------------------

  /** Editor → PDF: highlights where `line:column` of `path` is. */
  async forwardSync(path: string, line: number, column: number, silent = false): Promise<void> {
    if (!this.pdfPath) {
      if (!silent) this.flash(t("Compile the document first."));
      return;
    }
    if (settings.get("showPreview") === false && !this.pdfOnly && !this.pdfDetached) {
      settings.set("showPreview", true);
    }
    try {
      const boxes = await ipc.synctexForward(this.pdfPath, path, line, column);
      if (boxes.length) this.preview?.highlight(boxes);
      else if (!silent) this.flash(t("This position was not found in the PDF."));
    } catch (e) {
      if (!silent) this.flash(String(e));
    }
  }

  /** PDF → editor. */
  async inverseSync(page: number, x: number, y: number): Promise<void> {
    if (!this.pdfPath) return;
    try {
      const hit = await ipc.synctexInverse(this.pdfPath, page, x, y);
      if (!hit) { this.flash(t("This position was not found in the source.")); return; }
      if (this.pdfOnly) { this.pdfOnly = false; this.events.emit("layout"); }
      await this.open(hit.path, { line: hit.line, column: hit.column, flash: true });
      this.editor?.focus();
    } catch (e) {
      this.flash(String(e));
    }
  }

  // ---- misc --------------------------------------------------------------------------------------------------

  reveal(path: string): void {
    void ipc.revealInFileManager(path).catch(() => this.flash(t("Could not reveal the file")));
  }

  /** Include target of `\input{x}` relative to the including file's folder or the project root. */
  resolveInclude(target: string, fromPath: string): string | undefined {
    const candidates: string[] = [];
    const t0 = target.replace(/\\/g, "/");
    for (const base of [dirname(fromPath), this.mainFile ? dirname(this.mainFile) : this.root, this.root]) {
      candidates.push(joinPath(base, t0), joinPath(base, t0 + ".tex"));
    }
    const norm = (p: string) => p.replace(/\\/g, "/").split("/").reduce<string[]>((acc, seg) => {
      if (seg === "..") acc.pop(); else if (seg !== "." && seg !== "") acc.push(seg);
      return acc;
    }, []).join("/");
    const set = new Map(this.flatFiles.map((f) => [norm(f), f]));
    for (const c of candidates) {
      const hit = set.get(norm(c));
      if (hit) return hit;
    }
    return undefined;
  }
}

function flatten(nodes: FileNode[], out: string[] = []): string[] {
  for (const n of nodes) {
    if (n.isDir) flatten(n.children ?? [], out);
    else out.push(n.path);
  }
  return out;
}

/** `% !TEX program = xelatex` (also `TS-program`) in the first 20 lines. */
export function magicProgram(text: string): Engine | undefined {
  const lines = text.split("\n", 20);
  for (const line of lines) {
    const m = /^\s*%\s*!\s*TEX\s+(?:TS-)?program\s*=\s*(\w+)/i.exec(line);
    if (m) {
      const v = m[1]!.toLowerCase();
      if (v === "pdflatex" || v === "xelatex" || v === "lualatex") return v;
    }
  }
  return undefined;
}

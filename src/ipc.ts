// Typed wrappers for every Tauri command in CrossPlatform/PLAN.md. Keep in sync with src-tauri/src/commands.rs.
// Paths are absolute strings. Lines/columns are 1-based; columns are UTF-16 units.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Files with a BOM keep it: "utf-8-bom", "utf-16le", "utf-16be" are returned by readFile and accepted by writeFile. */
export type Encoding = "utf-8" | "latin-1" | "utf-8-bom" | "utf-16le" | "utf-16be";
export type Engine = "pdflatex" | "xelatex" | "lualatex";
export type Severity = "error" | "warning" | "info";
export type DiagnosticSource = "latex" | "bibtex" | "chktex" | "reference" | "structure";

export interface FileNode { name: string; path: string; isDir: boolean; children?: FileNode[] }
export interface ProjectInfo { root: string; name: string; files: FileNode[]; rootFile?: string }
export interface FileContent { text: string; encoding: Encoding }

export interface Diagnostic {
  severity: Severity; message: string; path: string; line: number; column?: number; source: DiagnosticSource;
  /** Multi-line TeX context (`l.12 ...`). */ detail?: string;
  /** Length in UTF-16 units of the span starting at `column`. */ length?: number;
  /** File TeX could not find (e.g. `tikz-cd.sty`); offer a package install. */ missingFile?: string;
}
export interface OutlineItem { level: number; title: string; path: string; line: number; label?: string }
export interface Label { name: string; path: string; line: number }
export interface BibEntry { key: string; type?: string; path: string; line: number; title?: string; author?: string }
export interface ProjectStats { words: number; [key: string]: number }
export interface ProjectIndex {
  outline: OutlineItem[]; labels: Label[]; citations: string[]; bibKeys: BibEntry[]; includes: string[];
  rootFile?: string; warnings: Diagnostic[]; stats: ProjectStats;
}

export type CompletionKind = "command" | "environment" | "label" | "citation" | "file" | "package" | "snippet" | string;
export interface CompletionItem { label: string; kind: CompletionKind; detail?: string; insertText: string; package?: string }

export interface TeXDistribution { name: string; binDir: string; latexmk: boolean; tlmgr: boolean; synctex: boolean }
export interface CompileResult {
  ok: boolean; pdfPath?: string; diagnostics: Diagnostic[]; missingPackages: string[]; durationMs: number; logPath?: string;
}
/** `complete`: items plus the document range (UTF-16 offsets) they replace. */
export interface CompletionResponse {
  from: number; to: number; prefix: string;
  context: "command" | "environment" | "endEnvironment" | "label" | "citation" | "package" | "file";
  inMath: boolean; items: CompletionItem[];
}
export interface SyncBox { page: number; x: number; y: number; width: number; height: number }
export interface InverseResult { path: string; line: number; column?: number }
export interface InstallResult { ok: boolean; output: string }
export interface RecentProject { path: string; name: string; lastOpened: number; label?: string }
/** Free-form settings JSON stored in the OS config dir; the UI agent defines the keys. */
export type Settings = Record<string, unknown>;

/** Unsaved editor buffers: absolute path -> text. */
export type Overrides = Record<string, string>;

export interface FsChangedEvent { paths: string[] }

export const openProject = (path: string) => invoke<ProjectInfo>("open_project", { path });
export const listFiles = (root: string) => invoke<FileNode[]>("list_files", { root });
export const readFile = (path: string) => invoke<FileContent>("read_file", { path });
export const writeFile = (path: string, text: string, encoding: Encoding) =>
  invoke<void>("write_file", { path, text, encoding });
export const createProject = (parentDir: string, name: string, template: string) =>
  invoke<ProjectInfo>("create_project", { parentDir, name, template });
export const projectIndex = (root: string, overrides: Overrides = {}) =>
  invoke<ProjectIndex>("project_index", { root, overrides });
export const completions = (root: string, path: string, text: string, line: number, column: number) =>
  invoke<CompletionItem[]>("completions", { root, path, text, line, column });
export const complete = (root: string, path: string, text: string, line: number, column: number) =>
  invoke<CompletionResponse | null>("complete", { root, path, text, line, column });
export const detectTex = () => invoke<TeXDistribution | null>("detect_tex", {});
export const compile = (root: string, rootFile: string, engine?: Engine, overrides: Overrides = {}) =>
  invoke<CompileResult>("compile", { root, rootFile, engine, overrides });
export const cancelCompile = (root: string) => invoke<void>("cancel_compile", { root });
export const synctexForward = (pdfPath: string, texPath: string, line: number, column: number) =>
  invoke<SyncBox[]>("synctex_forward", { pdfPath, texPath, line, column });
export const synctexInverse = (pdfPath: string, page: number, x: number, y: number) =>
  invoke<InverseResult | null>("synctex_inverse", { pdfPath, page, x, y });
export const installPackages = (packages: string[]) => invoke<InstallResult>("install_packages", { packages });
export const watchProject = (root: string) => invoke<void>("watch_project", { root });
export const unwatchProject = (root: string) => invoke<void>("unwatch_project", { root });
export const revealInFileManager = (path: string) => invoke<void>("reveal_in_file_manager", { path });
/** Opens an http(s)/mailto link in the system browser (backend enforces the scheme allowlist). */
export const openUrl = (url: string) => invoke<void>("open_url", { url });
export const loadSettings = () => invoke<Settings>("load_settings", {});
export const saveSettings = (settings: Settings) => invoke<Settings>("save_settings", { settings });
export const recentProjects = () => invoke<RecentProject[]>("recent_projects", {});
export const addRecentProject = (path: string, name?: string) =>
  invoke<RecentProject[]>("add_recent_project", { path, name });
export const setProjectLabel = (path: string, label?: string) =>
  invoke<RecentProject[]>("set_project_label", { path, label });

/** Subscribe to `fs-changed` (emitted by the Rust watcher after `watchProject`). */
export const onFsChanged = (cb: (e: FsChangedEvent) => void): Promise<UnlistenFn> =>
  listen<FsChangedEvent>("fs-changed", (ev) => cb(ev.payload));

/** The raw bytes of a PDF (binary IPC response, no JSON array). */
export const readPdf = (path: string) => invoke<ArrayBuffer>("read_pdf", { path });

// ---- file operations (sidebar) ----------------------------------------------------------------------------------

/** Creates an empty file in `dir` (adds “.tex” without an extension); resolves with its path. */
export const createFile = (dir: string, name: string) => invoke<string>("create_file", { dir, name });
export const createFolder = (dir: string, name: string) => invoke<string>("create_folder", { dir, name });
/** Renames in place; resolves with the new path. */
export const renamePath = (path: string, name: string) => invoke<string>("rename_path", { path, name });
/** “name copy.ext”, “name copy 2.ext”, …; resolves with the copy's path. */
export const duplicatePath = (path: string) => invoke<string>("duplicate_path", { path });
/** System trash (freedesktop spec on Linux, the Recycle Bin on Windows). */
export const moveToTrash = (path: string) => invoke<void>("move_to_trash", { path });
export const openInDefaultApp = (path: string) => invoke<void>("open_in_default_app", { path });
/** Drops the shadow workspace and deletes TeX leftovers (.aux, .log, …) in the project; resolves with how many. */
export const cleanAuxiliaryFiles = (root: string) => invoke<number>("clean_auxiliary_files", { root });
export const removeRecentProject = (path: string) => invoke<RecentProject[]>("remove_recent_project", { path });

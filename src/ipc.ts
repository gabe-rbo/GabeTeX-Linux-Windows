// Typed wrappers for every Tauri command in CrossPlatform/PLAN.md. Keep in sync with src-tauri/src/commands.rs.
// Paths are absolute strings. Lines/columns are 1-based; columns are UTF-16 units.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Encoding = "utf-8" | "latin-1";
export type Engine = "pdflatex" | "xelatex" | "lualatex";
export type Severity = "error" | "warning" | "info";
export type DiagnosticSource = "latex" | "bibtex" | "chktex" | "reference" | "structure";

export interface FileNode { name: string; path: string; isDir: boolean; children?: FileNode[] }
export interface ProjectInfo { root: string; name: string; files: FileNode[]; rootFile?: string }
export interface FileContent { text: string; encoding: Encoding }

export interface Diagnostic {
  severity: Severity; message: string; path: string; line: number; column?: number; source: DiagnosticSource;
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
export interface CompletionItem { label: string; kind: CompletionKind; detail?: string; insertText: string }

export interface TeXDistribution { name: string; binDir: string; latexmk: boolean; tlmgr: boolean; synctex: boolean }
export interface CompileResult {
  ok: boolean; pdfPath?: string; diagnostics: Diagnostic[]; missingPackages: string[]; durationMs: number; logPath?: string;
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
export const detectTex = () => invoke<TeXDistribution | null>("detect_tex", {});
export const compile = (root: string, rootFile: string, engine?: Engine, overrides: Overrides = {}) =>
  invoke<CompileResult>("compile", { root, rootFile, engine, overrides });
export const synctexForward = (pdfPath: string, texPath: string, line: number, column: number) =>
  invoke<SyncBox[]>("synctex_forward", { pdfPath, texPath, line, column });
export const synctexInverse = (pdfPath: string, page: number, x: number, y: number) =>
  invoke<InverseResult | null>("synctex_inverse", { pdfPath, page, x, y });
export const installPackages = (packages: string[]) => invoke<InstallResult>("install_packages", { packages });
export const watchProject = (root: string) => invoke<void>("watch_project", { root });
export const unwatchProject = (root: string) => invoke<void>("unwatch_project", { root });
export const revealInFileManager = (path: string) => invoke<void>("reveal_in_file_manager", { path });
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

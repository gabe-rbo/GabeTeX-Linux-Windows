// Window management (port of AppRouter + the scenes of GabeTeXApp.swift): the Welcome window, one window per
// project (several may be open), the detached PDF window of a project and the presentation windows. Every window
// runs the same frontend; its label says what it shows. Window labels are derived from the project path, so a
// project window is found again (reused) instead of opened twice.
//
// | label                 | shows                                                   |
// |-----------------------|---------------------------------------------------------|
// | `welcome`             | Welcome (small, fixed size)                              |
// | `project-<hash>`      | a project (`?project=<path>&file=<path>`)                |
// | `pdf-<hash>`          | the detached PDF of that project                         |
// | `present-<hash>`      | the presentation (slides; controls the presentation)     |
// | `presenter-<hash>`    | the presenter view on a second display                   |
// | `main`                | the browser mock / old single-window mode (in-window routing, no extra windows) |
//
// Tauri's window API is imported lazily (it is only needed once a window is opened or titled).
import "../ui/windows.css";
import { t } from "../i18n";
import { basename } from "../ui/dom";

export type WindowRole = "legacy" | "welcome" | "project" | "pdf" | "present" | "presenter";

interface TauriInternals { metadata?: { currentWindow?: { label?: string } } }

/** The label of this window ("main" outside Tauri). */
export function windowLabel(): string {
  const internals = (window as unknown as { __TAURI_INTERNALS__?: TauriInternals }).__TAURI_INTERNALS__;
  return internals?.metadata?.currentWindow?.label ?? "main";
}

export function windowRole(label = windowLabel()): WindowRole {
  if (label === "welcome") return "welcome";
  if (label.startsWith("project-")) return "project";
  if (label.startsWith("pdf-")) return "pdf";
  if (label.startsWith("presenter-")) return "presenter";
  if (label.startsWith("present-")) return "present";
  return "legacy";
}

/** Whether this window may open other windows (false in the browser mock and the single-window mode). */
export const isMultiWindow = (): boolean => windowRole() !== "legacy";

/** A short stable id for a path (FNV-1a, two seeds → 16 hex digits; labels allow only [A-Za-z0-9-/:_]). */
export function pathKey(path: string): string {
  const norm = path.replace(/\\/g, "/").replace(/\/+$/, "");
  const fnv = (seed: number) => {
    let h = seed >>> 0;
    for (let i = 0; i < norm.length; i++) {
      h ^= norm.charCodeAt(i);
      h = Math.imul(h, 0x01000193) >>> 0;
    }
    return h.toString(16).padStart(8, "0");
  };
  return fnv(0x811c9dc5) + fnv(0x2f6b1a37);
}

export const projectLabel = (root: string): string => `project-${pathKey(root)}`;
export const pdfLabel = (root: string): string => `pdf-${pathKey(root)}`;
export const presentLabel = (root: string): string => `present-${pathKey(root)}`;
export const presenterLabel = (root: string): string => `presenter-${pathKey(root)}`;

const windowApi = () => import("@tauri-apps/api/window");
const webviewWindowApi = () => import("@tauri-apps/api/webviewWindow");

/** Sets the document and the native window title (Mac: “Project — file” in the title bar). */
export function setWindowTitle(title: string): void {
  if (document.title === title) return;
  document.title = title;
  if (windowRole() === "legacy" && !("__TAURI_INTERNALS__" in window)) return;
  void windowApi().then((m) => m.getCurrentWindow().setTitle(title)).catch(() => {});
}

/** Finds an open window by label. */
export async function findWindow(label: string): Promise<import("@tauri-apps/api/webviewWindow").WebviewWindow | null> {
  try {
    const { WebviewWindow } = await webviewWindowApi();
    return await WebviewWindow.getByLabel(label);
  } catch {
    return null;
  }
}

/** Brings a window forward (restoring it from the dock/taskbar). */
export async function focusWindow(label: string): Promise<boolean> {
  const w = await findWindow(label);
  if (!w) return false;
  await w.unminimize().catch(() => {});
  await w.show().catch(() => {});
  await w.setFocus().catch(() => {});
  return true;
}

export interface CreateOptions {
  url: string;
  title: string;
  width?: number;
  height?: number;
  minWidth?: number;
  minHeight?: number;
  x?: number;
  y?: number;
  resizable?: boolean;
  maximizable?: boolean;
  center?: boolean;
  decorations?: boolean;
  focus?: boolean;
  fullscreen?: boolean;
  visible?: boolean;
}

/** Creates a webview window and waits until it exists. Rejects when it could not be created. */
export async function createWindow(label: string, opts: CreateOptions): Promise<import("@tauri-apps/api/webviewWindow").WebviewWindow> {
  const { WebviewWindow } = await webviewWindowApi();
  const w = new WebviewWindow(label, { ...opts, dragDropEnabled: true });
  await new Promise<void>((resolve, reject) => {
    void w.once("tauri://created", () => resolve());
    void w.once("tauri://error", (e) => reject(new Error(String((e as { payload?: unknown }).payload ?? "Could not create the window"))));
  });
  return w;
}

/** Destroys a window without asking it (no close-requested event). */
export async function destroyWindow(label: string): Promise<void> {
  const w = await findWindow(label);
  await w?.destroy().catch(() => {});
}

/** The labels of every open window. */
export async function windowLabels(): Promise<string[]> {
  try {
    const { getAllWindows } = await windowApi();
    return (await getAllWindows()).map((w) => w.label);
  } catch {
    return [];
  }
}

/** URL of the frontend with query parameters (relative: Tauri resolves it against the app or dev server). */
export function appUrl(params: Record<string, string | number | undefined>): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== "") q.set(k, String(v));
  const s = q.toString();
  return s ? `index.html?${s}` : "index.html";
}

// ---- project and welcome windows --------------------------------------------------------------------------------

/** Event sent to an open project window to open a file in it. */
export const OPEN_FILE_EVENT = "gabetex://open-file";

/** Opens (or brings forward) the window of a project folder; `file` is opened in it. */
export async function openProjectWindow(root: string, name: string, file?: string): Promise<void> {
  const label = projectLabel(root);
  const existing = await findWindow(label);
  if (existing) {
    if (file) {
      const { emitTo } = await import("@tauri-apps/api/event");
      await emitTo(label, OPEN_FILE_EVENT, { file });
    }
    await focusWindow(label);
    return;
  }
  await createWindow(label, {
    url: appUrl({ project: root, file }),
    title: name || basename(root),
    width: 1320, height: 840, minWidth: 900, minHeight: 600,
    focus: true,
  });
}

/** Shows the Welcome window (Window ▸ Welcome to GabeTeX), creating it when it is closed. */
export async function showWelcomeWindow(): Promise<void> {
  if (await focusWindow("welcome")) return;
  await createWindow("welcome", {
    url: appUrl({}),
    title: t("Welcome to GabeTeX"),
    width: WELCOME_SIZE.width, height: WELCOME_SIZE.height,
    resizable: false, maximizable: false, center: true, focus: true,
  });
}

/** The Mac Welcome window is 300 + 380 pt wide and 440 pt tall; ours matches the card of the web version. */
export const WELCOME_SIZE = { width: 700, height: 450 };

/** Closes the Welcome window (after a project window opened). */
export async function closeWelcomeWindow(): Promise<void> {
  if (windowRole() === "welcome") {
    const { getCurrentWindow } = await windowApi();
    await getCurrentWindow().destroy().catch(() => {});
  } else {
    await destroyWindow("welcome");
  }
}

/** Number of open project windows. */
export async function projectWindowCount(): Promise<number> {
  return (await windowLabels()).filter((l) => l.startsWith("project-")).length;
}

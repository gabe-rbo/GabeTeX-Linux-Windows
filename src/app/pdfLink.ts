// The detachable PDF preview, project side (port of ProjectWorkspace+PDFWindow.swift). The preview can leave the
// project window and live in a window of its own (second display, free placement). The PDF window runs its own
// pdf.js preview (src/ui/pdfWindow.ts); this link keeps it in sync through window-targeted events:
//
//   project → PDF   `pdf:load {path}` (every compile), `pdf:highlight {boxes}` (forward SyncTeX), `pdf:clear`
//   PDF → project   `pdf:ready`, `pdf:inverse {page,x,y}` (inverse SyncTeX), `pdf:page {page,count}`,
//                   `pdf:command {id}` (shortcuts typed in the PDF window), `pdf:attach`, `pdf:closed {frame}`
//
// While detached, `ws.preview` is a proxy that forwards to the PDF window, so the workspace code is unchanged.
// The detached state is remembered per project and restored when the project opens again; closing the PDF window
// docks the preview again; closing the project window closes its PDF window and keeps the state.
import type { UnlistenFn } from "@tauri-apps/api/event";
import type { PreviewHandle } from "../preview";
import type { SyncBox } from "../ipc";
import type { Workspace } from "./workspace";
import { t } from "../i18n";
import { settings } from "./settings";
import { appUrl, createWindow, destroyWindow, findWindow, focusWindow, pdfLabel, windowLabel } from "./windows";

export interface PdfFrame { x: number; y: number; width: number; height: number }

/** Events between a project window and its PDF window. */
export const PDF_EVENT = {
  load: "pdf:load", highlight: "pdf:highlight", clear: "pdf:clear",
  ready: "pdf:ready", inverse: "pdf:inverse", page: "pdf:page", command: "pdf:command", attach: "pdf:attach", closed: "pdf:closed",
} as const;

type Memory = { pdfDetached?: boolean; pdfFrame?: PdfFrame };
const memory = (root: string): Memory => settings.project(root) as Memory;

export class PdfWindowLink {
  detached = false;
  private readonly label: string;
  private unlisten: UnlistenFn[] = [];
  private closingProject = false;
  private readonly remote: PreviewHandle;

  constructor(
    private readonly ws: Workspace,
    private readonly local: PreviewHandle,
    private readonly hooks: { changed(): void; runCommand(id: string): void },
  ) {
    this.label = pdfLabel(ws.root);
    const send = (event: string, payload: unknown = {}) => void this.emit(event, payload);
    this.remote = {
      load: async (path) => send(PDF_EVENT.load, { path }),
      highlight: (boxes: SyncBox[]) => {
        send(PDF_EVENT.highlight, { boxes });
        void this.reveal();
      },
      zoomIn: () => {}, zoomOut: () => {}, zoomToFit: () => {}, setZoom: () => {},
      clear: () => send(PDF_EVENT.clear),
      destroy: () => {},
    };
  }

  /** Listens to the PDF window and restores a remembered detached preview. */
  async start(): Promise<void> {
    const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const me = getCurrentWebviewWindow();
    const on = async <T>(name: string, fn: (p: T) => void) => this.unlisten.push(await me.listen<T>(name, (e) => fn(e.payload)));
    // The PDF window may be ready before `createWindow` resolved here: answer whenever it asks.
    await on(PDF_EVENT.ready, () => {
      if (this.ws.pdfPath) void this.emit(PDF_EVENT.load, { path: this.ws.pdfPath });
    });
    await on<{ page: number; x: number; y: number }>(PDF_EVENT.inverse, (p) => {
      void focusWindow(windowLabel());
      void this.ws.inverseSync(p.page, p.x, p.y);
    });
    await on<{ page: number; count: number }>(PDF_EVENT.page, (p) => {
      const changed = p.count !== this.ws.pageCount;
      this.ws.page = p.page;
      this.ws.pageCount = p.count;
      if (changed) this.ws.events.emit("pdf");
    });
    await on<{ id: string }>(PDF_EVENT.command, (p) => this.hooks.runCommand(p.id));
    await on(PDF_EVENT.attach, () => void this.attach());
    await on<{ frame?: PdfFrame }>(PDF_EVENT.closed, (p) => {
      if (p.frame) settings.updateProject(this.ws.root, (m) => { (m as Memory).pdfFrame = p.frame; });
      // The user closed the PDF window: the preview docks again (unless the whole project is closing).
      if (this.detached && !this.closingProject) this.setDocked(true);
    });
    if (memory(this.ws.root).pdfDetached) await this.detach(true);
  }

  toggle(): Promise<boolean> {
    if (this.detached) { void this.attach(); return Promise.resolve(true); }
    return this.detach();
  }

  /** Moves the preview into its own window. False when the window could not be created (use PDF Only then). */
  async detach(restoring = false): Promise<boolean> {
    if (this.detached) { await focusWindow(this.label); return true; }
    const frame = memory(this.ws.root).pdfFrame;
    try {
      if (!(await findWindow(this.label))) {
        await createWindow(this.label, {
          url: appUrl({ pdf: this.ws.root, owner: windowLabel(), name: this.ws.name }),
          title: t("%@ — PDF", this.ws.name),
          width: frame?.width ?? 760, height: frame?.height ?? 940, minWidth: 320, minHeight: 360,
          x: frame?.x, y: frame?.y, focus: !restoring,
        });
      } else {
        await focusWindow(this.label);
      }
    } catch (e) {
      console.warn("PDF window unavailable", e);
      return false;
    }
    this.detached = true;
    this.ws.pdfDetached = true;
    if (this.ws.pdfOnly) this.ws.pdfOnly = false;
    this.ws.preview = this.remote;
    this.local.clear();
    settings.updateProject(this.ws.root, (m) => { (m as Memory).pdfDetached = true; });
    this.hooks.changed();
    // A restored PDF window must not take the focus from the project window.
    if (restoring) window.setTimeout(() => void focusWindow(windowLabel()), 250);
    return true;
  }

  /** Docks the preview in the project window again (“Attach PDF to Project Window”). */
  async attach(): Promise<void> {
    if (!this.detached) return;
    await this.storeFrame();
    this.setDocked(true);
    await destroyWindow(this.label);
    void focusWindow(windowLabel());
  }

  /** Shows the PDF window for a forward search (brings it back when it got lost). */
  async reveal(): Promise<void> {
    if (!this.detached) return;
    const w = await findWindow(this.label);
    if (!w) { this.setDocked(true); return; }
    if (await w.isMinimized().catch(() => false)) await w.unminimize().catch(() => {});
  }

  /** ⌥⌘P while detached: bring the PDF window forward. */
  focus(): void {
    void focusWindow(this.label).then((ok) => { if (!ok) this.setDocked(true); });
  }

  /** The project window is closing: its PDF window goes with it; the detached state stays for the next time. */
  async closeWithProject(): Promise<void> {
    this.closingProject = true;
    if (this.detached) {
      await this.storeFrame();
      await destroyWindow(this.label);
    }
  }

  dispose(): void {
    for (const u of this.unlisten.splice(0)) u();
  }

  private setDocked(remember: boolean): void {
    this.detached = false;
    this.ws.pdfDetached = false;
    this.ws.preview = this.local;
    if (remember) settings.updateProject(this.ws.root, (m) => { (m as Memory).pdfDetached = false; });
    if (!settings.get("showPreview")) settings.set("showPreview", true);
    if (this.ws.pdfPath) void this.local.load(this.ws.pdfPath).catch(() => {});
    this.hooks.changed();
  }

  private async storeFrame(): Promise<void> {
    const w = await findWindow(this.label);
    if (!w) return;
    try {
      const [pos, size, scale] = await Promise.all([w.outerPosition(), w.innerSize(), w.scaleFactor()]);
      const frame: PdfFrame = { x: Math.round(pos.x / scale), y: Math.round(pos.y / scale), width: Math.round(size.width / scale), height: Math.round(size.height / scale) };
      settings.updateProject(this.ws.root, (m) => { (m as Memory).pdfFrame = frame; });
    } catch {
      /* keep the previous frame */
    }
  }

  private async emit(event: string, payload: unknown): Promise<void> {
    try {
      const { emitTo } = await import("@tauri-apps/api/event");
      await emitTo(this.label, event, payload);
    } catch {
      /* the window is gone */
    }
  }
}

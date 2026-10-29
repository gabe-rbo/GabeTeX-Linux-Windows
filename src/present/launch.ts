// Presentation mode, project side (port of ProjectWorkspace+Presentation.swift): knows from the root's preamble
// whether the document is a Beamer talk (the toolbar shows Present) and where its notes are, starts a presentation
// of the compiled PDF on the right displays, keeps it following live compiles, and ends it with the project window.
import type { UnlistenFn } from "@tauri-apps/api/event";
import type { Workspace } from "../app/workspace";
import * as ipc from "../ipc";
import { t } from "../i18n";
import { alertDialog } from "../ui/overlay";
import {
  appUrl, createWindow, destroyWindow, focusWindow, isMultiWindow, presentLabel, presenterLabel, windowLabel,
} from "../app/windows";
import { isBuiltInDisplayName, isPresentationSource, planScreens, scanPresentationSource, type PresentationSourceInfo } from "./logic";
import { PRESENT_EVENT } from "./events";
import type { LocalPresentation } from "./presentation";

export class PresentationLink {
  presenting = false;
  source: PresentationSourceInfo = { documentClass: null, notesSide: null };
  private local: LocalPresentation | null = null;
  private unlisten: UnlistenFn[] = [];
  private offs: Array<() => void> = [];
  private scanTimer = 0;
  private scanGen = 0;
  private readonly primary: string;
  private readonly mirror: string;

  constructor(private readonly ws: Workspace, private readonly changed: () => void) {
    this.primary = presentLabel(ws.root);
    this.mirror = presenterLabel(ws.root);
    this.offs.push(
      ws.events.on(["index", "main"], () => this.scheduleScan()),
      ws.events.on("compile", () => this.compiled()),
    );
    this.scheduleScan();
  }

  async start(): Promise<void> {
    if (!isMultiWindow()) return;
    const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const me = getCurrentWebviewWindow();
    this.unlisten.push(
      await me.listen(PRESENT_EVENT.ended, () => this.didEnd()),
      await me.listen<{ message: string }>(PRESENT_EVENT.failed, (e) => void alertDialog({
        title: t("The presentation could not start"),
        message: e.payload.message || t("The PDF file could not be read."),
      })),
    );
  }

  /** The root document uses a slide class: the toolbar shows Present. */
  get isPresentationDocument(): boolean { return isPresentationSource(this.source); }
  get canPresent(): boolean { return !!this.ws.pdfPath; }

  toggle(): void {
    if (this.presenting) this.stop();
    else void this.present(false);
  }

  /** Starts presenting (from the first slide, or the page the preview shows); brings a running one forward. */
  async present(fromCurrentPage: boolean): Promise<void> {
    if (this.presenting) {
      if (this.local) return;
      void import("@tauri-apps/api/event").then(({ emitTo }) => emitTo(this.primary, PRESENT_EVENT.focus, {})).catch(() => {});
      return;
    }
    const pdf = this.ws.pdfPath;
    if (!pdf) { this.ws.flash(t("Compile the document to present it")); return; }
    await this.scan();
    const page = fromCurrentPage ? Math.max(0, this.ws.page - 1) : 0;
    this.presenting = true;
    this.changed();
    if (isMultiWindow() && (await this.openWindows(pdf, page))) return;
    const { startLocalPresentation } = await import("./presentation");
    this.local = startLocalPresentation({
      pdf, page, title: this.ws.name, notesHint: this.source.notesSide,
      onEnd: () => { this.local = null; this.didEnd(); },
      onFailed: (message) => void alertDialog({ title: t("The presentation could not start"), message: message || t("The PDF file could not be read.") }),
    });
  }

  stop(): void {
    if (!this.presenting) return;
    if (this.local) { this.local.end(); return; }
    void import("@tauri-apps/api/event").then(({ emitTo }) => emitTo(this.primary, PRESENT_EVENT.end, {})).catch(() => this.didEnd());
  }

  /** The project window is closing: its presentation ends with it. */
  async closeWithProject(): Promise<void> {
    if (!this.presenting) return;
    this.presenting = false;
    this.local?.end();
    await destroyWindow(this.mirror);
    await destroyWindow(this.primary);
  }

  dispose(): void {
    clearTimeout(this.scanTimer);
    for (const off of this.offs.splice(0)) off();
    for (const u of this.unlisten.splice(0)) u();
  }

  // ---- windows -----------------------------------------------------------------------------------------------------

  private async openWindows(pdf: string, page: number): Promise<boolean> {
    try {
      const { availableMonitors, currentMonitor } = await import("@tauri-apps/api/window");
      const monitors = await availableMonitors();
      const here = await currentMonitor();
      const id = (m: { position: { x: number; y: number } }) => `${m.position.x},${m.position.y}`;
      const plan = planScreens(monitors.map((m) => ({ id: id(m), isBuiltIn: isBuiltInDisplayName(m.name) })), here ? id(here) : null, false);
      const audience = monitors.find((m) => plan && id(m) === plan.audience) ?? here ?? monitors[0];
      const presenter = plan?.presenter ? monitors.find((m) => id(m) === plan.presenter) : undefined;
      const place = (m: typeof audience) => {
        if (!m) return {};
        const pos = m.position.toLogical(m.scaleFactor);
        const size = m.size.toLogical(m.scaleFactor);
        return { x: pos.x, y: pos.y, width: size.width, height: size.height, mx: m.position.x, my: m.position.y };
      };
      const a = place(audience);
      await createWindow(this.primary, {
        url: appUrl({
          present: 1, owner: windowLabel(), peer: presenter ? this.mirror : undefined, pdf, page, title: this.ws.name,
          notes: this.source.notesSide ?? undefined, aname: audience?.name ?? undefined, mx: a.mx, my: a.my,
        }),
        title: this.ws.name, x: a.x, y: a.y, width: a.width, height: a.height, decorations: false, focus: !presenter,
      });
      if (presenter) {
        const b = place(presenter);
        await createWindow(this.mirror, {
          url: appUrl({ presenter: 1, primary: this.primary, mx: b.mx, my: b.my }),
          title: t("Presenter View"), x: b.x, y: b.y, width: b.width, height: b.height, decorations: false, focus: true,
        }).catch(async (e) => {
          await destroyWindow(this.primary);
          throw e;
        });
      }
      return true;
    } catch (e) {
      console.warn("Presentation windows unavailable", e);
      await destroyWindow(this.primary);
      return false;
    }
  }

  private didEnd(): void {
    if (!this.presenting) return;
    this.presenting = false;
    this.changed();
    void focusWindow(windowLabel());
  }

  /** Every finished compile with a PDF reloads the running presentation (keeping the slide). */
  private compiled(): void {
    const k = this.ws.compileState.kind;
    if (!this.presenting || (k !== "succeeded" && k !== "failed") || !this.ws.pdfPath) return;
    const pdf = this.ws.pdfPath;
    if (this.local) { this.local.reload(pdf); return; }
    void import("@tauri-apps/api/event").then(({ emitTo }) => emitTo(this.primary, PRESENT_EVENT.reload, { path: pdf })).catch(() => {});
  }

  // ---- source ------------------------------------------------------------------------------------------------------

  private scheduleScan(): void {
    clearTimeout(this.scanTimer);
    this.scanTimer = window.setTimeout(() => void this.scan(), 300);
  }

  /** Reads the root's preamble (the open buffer if there is one, else the file). */
  private async scan(): Promise<void> {
    clearTimeout(this.scanTimer);
    const gen = ++this.scanGen;
    const main = this.ws.mainFile;
    let info: PresentationSourceInfo = { documentClass: null, notesSide: null };
    if (main) {
      let text = this.ws.editor?.getText(main);
      if (text === undefined) text = await ipc.readFile(main).then((c) => c.text).catch(() => "");
      info = scanPresentationSource(text);
    }
    if (gen !== this.scanGen) return;
    const before = this.isPresentationDocument;
    this.source = info;
    if (before !== this.isPresentationDocument) this.changed();
  }
}

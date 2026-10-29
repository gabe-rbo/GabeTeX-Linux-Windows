// Presentation mode (port of PreviewKit/Presentation/PresentationController.swift). A presentation runs in its own
// full-screen windows, started by the project window (src/present/launch.ts):
//
// - `present-<hash>` (the controller): shows the slides on the audience display; owns the navigation state, the
//   timer and the PDF reloads, and broadcasts the state to the presenter window.
// - `presenter-<hash>` (two displays only): the presenter view; sends every key / control to the controller.
//
// With one display there is a single window: the slides, and P toggles the presenter view in place. Swap Displays
// exchanges what the two windows show. Esc, the End button or closing a window ends the presentation; the project
// window is told (`present:ended`) and comes back. Without separate windows (browser mock), `startLocalPresentation`
// runs the same controller in a full-screen layer of the current window.
import { t } from "../i18n";
import { openSlideDocument, type SlideDocument } from "../preview/slides";
import { Navigator, commandForKey, timerReset, timerToggle, type NotesMode, type NotesSide, type Outcome } from "./logic";
import { PRESENT_EVENT } from "./events";
import { Stage, type PresentationAction, type PresentationState, type StageMode } from "./stage";

export { PRESENT_EVENT } from "./events";

const DIGIT_TIMEOUT_MS = 5000;
const NOTES_KEY = "gabetex.presentation.notesMode";

function rememberedNotesMode(): NotesMode {
  try {
    const v = localStorage.getItem(NOTES_KEY);
    if (v === "automatic" || v === "right" || v === "left" || v === "off") return v;
  } catch { /* storage unavailable */ }
  return "automatic";
}

export interface ControllerOptions {
  pdf: string;
  page: number;
  title: string;
  notesHint: NotesSide | null;
  layout: "dual" | "single";
  audienceName: string;
  /** Shows the state in this window. */
  stage: Stage;
  /** The mode this window shows for a state. */
  modeFor(state: PresentationState): StageMode;
  /** Sends the state to the presenter window (dual layout). */
  broadcast?(state: PresentationState): void;
  /** Called once after the presentation ended (close the windows here). */
  finish(): void;
  /** The PDF could not be read at the start. */
  failed(message: string): void;
}

/** The presentation state machine shared by both windows (lives in the controller window). */
export class PresentationController {
  private nav = new Navigator(0);
  private doc: SlideDocument | null = null;
  private state: PresentationState;
  private ended = false;
  private digitTimer = 0;
  private reloadGen = 0;

  constructor(private readonly o: ControllerOptions) {
    this.state = {
      pdf: o.pdf, version: 0, index: 0, pageCount: 0, blank: "none", typed: "",
      timer: { accumulated: 0, runningSince: Date.now() }, notesMode: rememberedNotesMode(), notesHint: o.notesHint,
      layout: o.layout, showsPresenter: false, swapped: false, audienceName: o.audienceName, title: o.title,
    };
  }

  /** Loads the PDF; false when it cannot be read (the presentation then ends). */
  async start(): Promise<boolean> {
    try {
      this.doc = await openSlideDocument(this.o.pdf);
    } catch (e) {
      this.o.failed(String(e));
      this.end();
      return false;
    }
    if (this.ended) { this.doc.destroy(); return false; }
    this.nav = new Navigator(this.doc.pageCount, this.o.page);
    this.o.stage.setDocument(this.doc);
    this.state.timer = { accumulated: 0, runningSince: Date.now() };
    this.publish();
    return true;
  }

  get current(): PresentationState { return this.state; }

  /** A live compile produced a new PDF: reload, keeping the slide (clamped). */
  async reload(path: string): Promise<void> {
    const gen = ++this.reloadGen;
    let doc: SlideDocument;
    try {
      doc = await openSlideDocument(path);
    } catch {
      return; // keep the previous document on screen
    }
    if (gen !== this.reloadGen || this.ended) { doc.destroy(); return; }
    const old = this.doc;
    this.doc = doc;
    this.nav.updatePageCount(doc.pageCount);
    this.state.pdf = path;
    this.state.version++;
    this.o.stage.setDocument(doc);
    this.publish();
    old?.destroy();
  }

  handle(a: PresentationAction): void {
    if (this.ended) return;
    switch (a.type) {
      case "cmd": this.apply(this.nav.perform(a.cmd)); break;
      case "timer":
        this.state.timer = a.op === "toggle" ? timerToggle(this.state.timer, Date.now()) : timerReset(this.state.timer, Date.now());
        this.publish();
        break;
      case "notes":
        this.state.notesMode = a.mode;
        try { localStorage.setItem(NOTES_KEY, a.mode); } catch { /* storage unavailable */ }
        this.publish();
        break;
      case "swap":
        if (this.state.layout === "dual") { this.state.swapped = !this.state.swapped; this.publish(); }
        break;
    }
  }

  /** A key from either window; true when it belongs to the presentation. */
  handleKey(e: KeyboardEvent): boolean {
    const cmd = commandForKey({ key: e.key, code: e.code, shift: e.shiftKey, alt: e.altKey, ctrl: e.ctrlKey, meta: e.metaKey });
    if (!cmd) return false;
    this.handle({ type: "cmd", cmd });
    return true;
  }

  end(): void {
    if (this.ended) return;
    this.ended = true;
    clearTimeout(this.digitTimer);
    this.doc?.destroy();
    this.doc = null;
    this.o.finish();
  }

  private apply(outcome: Outcome): void {
    switch (outcome.kind) {
      case "end": this.end(); return;
      case "togglePresenterView":
        if (this.state.layout !== "single") return;
        this.state.showsPresenter = !this.state.showsPresenter;
        break;
      case "digitsChanged":
        clearTimeout(this.digitTimer);
        if (this.nav.typedDigits) this.digitTimer = window.setTimeout(() => this.handle({ type: "cmd", cmd: { kind: "clearDigits" } }), DIGIT_TIMEOUT_MS);
        break;
      case "none": return;
      default: break;
    }
    this.publish();
  }

  private publish(): void {
    const s = this.state;
    s.index = this.nav.currentIndex;
    s.pageCount = this.nav.pageCount;
    s.blank = this.nav.blank;
    s.typed = this.nav.typedDigits;
    const snapshot: PresentationState = { ...s, timer: { ...s.timer } };
    this.o.stage.setMode(this.o.modeFor(snapshot));
    this.o.stage.update(snapshot);
    this.o.broadcast?.(snapshot);
  }
}

/** Mode of the controller window: the slides, or the presenter view (one display + P, or swapped displays). */
const primaryMode = (s: PresentationState): StageMode =>
  s.layout === "dual" ? (s.swapped ? "presenter" : "audience") : s.showsPresenter ? "presenter" : "audience";
/** Mode of the presenter window. */
const mirrorMode = (s: PresentationState): StageMode => (s.swapped ? "audience" : "presenter");

function keyListener(handle: (e: KeyboardEvent) => boolean): (e: KeyboardEvent) => void {
  return (e) => {
    if (e.isComposing) return;
    if (handle(e)) { e.preventDefault(); e.stopPropagation(); }
  };
}

async function fullscreenOn(mx: string | null, my: string | null): Promise<void> {
  const { getCurrentWindow, PhysicalPosition } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();
  try {
    if (mx !== null && my !== null) await win.setFullscreenOnMonitor(new PhysicalPosition(Number(mx) + 1, Number(my) + 1));
    else await win.setFullscreen(true);
  } catch {
    await win.setFullscreen(true).catch(() => {});
  }
}

// ---- the controller window --------------------------------------------------------------------------------------

export async function bootPresentationWindow(root: HTMLElement): Promise<void> {
  const p = new URLSearchParams(location.search);
  const owner = p.get("owner") ?? "";
  const peer = p.get("peer") ?? "";
  const title = p.get("title") ?? "";
  document.title = title ? `${title} — ${t("Present")}` : t("Present");
  document.documentElement.classList.add("presentation-window");
  const { emitTo } = await import("@tauri-apps/api/event");
  const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  const me = getCurrentWebviewWindow();
  void fullscreenOn(p.get("mx"), p.get("my"));

  let controller: PresentationController | null = null;
  const stage = new Stage((a) => controller?.handle(a));
  root.append(stage.el);
  const hint = p.get("notes");
  controller = new PresentationController({
    pdf: p.get("pdf") ?? "",
    page: Number(p.get("page") ?? 0) || 0,
    title,
    notesHint: hint === "left" || hint === "right" || hint === "top" || hint === "bottom" ? hint : null,
    layout: peer ? "dual" : "single",
    audienceName: p.get("aname") ?? "",
    stage,
    modeFor: primaryMode,
    broadcast: peer ? (s) => void emitTo(peer, PRESENT_EVENT.state, s).catch(() => {}) : undefined,
    failed: (message) => void emitTo(owner, PRESENT_EVENT.failed, { message }).catch(() => {}),
    finish: () => void (async () => {
      if (peer) await (await WebviewWindow.getByLabel(peer))?.destroy().catch(() => {});
      await emitTo(owner, PRESENT_EVENT.ended, {}).catch(() => {});
      await me.destroy().catch(() => {});
    })(),
  });
  const c = controller;
  document.addEventListener("keydown", keyListener((e) => c.handleKey(e)), true);
  document.addEventListener("contextmenu", (e) => e.preventDefault());
  await me.listen<PresentationAction>(PRESENT_EVENT.action, (e) => c.handle(e.payload));
  await me.listen(PRESENT_EVENT.hello, () => { if (peer) void emitTo(peer, PRESENT_EVENT.state, c.current).catch(() => {}); });
  await me.listen<{ path: string }>(PRESENT_EVENT.reload, (e) => void c.reload(e.payload.path));
  await me.listen(PRESENT_EVENT.end, () => c.end());
  await me.listen(PRESENT_EVENT.focus, () => {
    const presenterWindow = peer && !c.current.swapped ? peer : null;
    if (presenterWindow) void WebviewWindow.getByLabel(presenterWindow).then((w) => w?.setFocus()).catch(() => {});
    else void me.setFocus().catch(() => {});
  });
  await me.onCloseRequested((e) => { e.preventDefault(); c.end(); });
  await c.start();
}

// ---- the presenter window (two displays) -------------------------------------------------------------------------

export async function bootPresenterWindow(root: HTMLElement): Promise<void> {
  const p = new URLSearchParams(location.search);
  const primary = p.get("primary") ?? "";
  document.title = t("Presenter View");
  document.documentElement.classList.add("presentation-window");
  const { emitTo } = await import("@tauri-apps/api/event");
  const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  const me = getCurrentWebviewWindow();
  void fullscreenOn(p.get("mx"), p.get("my"));

  const send = (a: PresentationAction) => void emitTo(primary, PRESENT_EVENT.action, a).catch(() => {});
  const stage = new Stage(send);
  stage.setMode("presenter");
  root.append(stage.el);
  let pdf = "";
  let version = -1;
  let docGen = 0;
  await me.listen<PresentationState>(PRESENT_EVENT.state, (e) => {
    const s = e.payload;
    stage.setMode(mirrorMode(s));
    if (s.pdf !== pdf || s.version !== version) {
      pdf = s.pdf;
      version = s.version;
      const gen = ++docGen;
      void openSlideDocument(s.pdf).then((doc) => {
        if (gen !== docGen) { doc.destroy(); return; }
        stage.setDocument(doc);
      }).catch(() => {});
    }
    stage.update(s);
  });
  document.addEventListener("keydown", keyListener((e) => {
    const cmd = commandForKey({ key: e.key, code: e.code, shift: e.shiftKey, alt: e.altKey, ctrl: e.ctrlKey, meta: e.metaKey });
    if (!cmd) return false;
    send({ type: "cmd", cmd });
    return true;
  }), true);
  document.addEventListener("contextmenu", (e) => e.preventDefault());
  await me.onCloseRequested((e) => { e.preventDefault(); send({ type: "cmd", cmd: { kind: "end" } }); });
  void emitTo(primary, PRESENT_EVENT.hello, {}).catch(() => {});
  void me.setFocus().catch(() => {});
}

// ---- fallback: in the current window ---------------------------------------------------------------------------

export interface LocalPresentation { reload(path: string): void; end(): void }

/** Runs a single-display presentation in a full-screen layer of this window (no separate windows available). */
export function startLocalPresentation(opts: { pdf: string; page: number; title: string; notesHint: NotesSide | null; onEnd(): void; onFailed(message: string): void }): LocalPresentation {
  const layer = document.createElement("div");
  layer.className = "presentation-layer";
  document.body.append(layer);
  let controller: PresentationController | null = null;
  const stage = new Stage((a) => controller?.handle(a));
  layer.append(stage.el);
  const onKey = keyListener((e) => controller?.handleKey(e) ?? false);
  document.addEventListener("keydown", onKey, true);
  void layer.requestFullscreen?.().catch(() => {});
  const c = (controller = new PresentationController({
    pdf: opts.pdf, page: opts.page, title: opts.title, notesHint: opts.notesHint, layout: "single", audienceName: "",
    stage, modeFor: primaryMode, failed: opts.onFailed,
    finish: () => {
      document.removeEventListener("keydown", onKey, true);
      if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
      stage.destroy();
      layer.remove();
      opts.onEnd();
    },
  }));
  void c.start();
  return { reload: (path) => void c.reload(path), end: () => c.end() };
}

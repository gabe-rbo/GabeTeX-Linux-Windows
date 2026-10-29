// PDF preview (port of Sources/PreviewKit: GabePDFView, PDFPreviewControls, PDFPreviewPlaceholder, the highlight
// overlay). Plain DOM + pdf.js, loaded lazily on the first `load()`.
//
// - Continuous vertical layout like PDFKit (`.singlePageContinuous`, page-break margins 8/10 pt that grow with
//   the zoom), white pages with a soft shadow on a calm backdrop in Light and Dark mode.
// - Only the visible pages (+1 above/below) are rendered, at devicePixelRatio, nearest to the centre first;
//   renders of pages that scroll away are cancelled and far canvases are freed. While zooming, existing canvases are
//   stretched by CSS and re-rendered once the zoom settles (no blank pages).
// - `load()` of a rebuilt PDF keeps zoom (or fit width) and the reading position (page + point in the page), renders
//   the pages that will be visible offscreen and swaps everything in one synchronous step: no flash, no jump.
// - Ctrl-click → `onInverseSync(page, x, y)` in PDF user space (points, origin bottom-left, rotation and crop box
//   honoured by pdf.js's viewport). `highlight()` centres the first box and flashes all of them (1.5 s + fade).
// - Text layer (selection / copy) only for visible pages. Ctrl+wheel / pinch zoom anchored at the pointer.
import "./preview.css";
import { readPdf, type SyncBox } from "../ipc";
import { t } from "../i18n";
import { PreviewControls, createPlaceholder } from "./controls";
import {
  MARGIN_TOP, MAX_SCALE, MIN_SCALE, anchorAt, clampScale, fitWidthScale, layoutPages, pageAtY, resolveAnchor, stepZoom,
  type Anchor, type PageBox,
} from "./layout";
import {
  loadPdfJs, type PDFDocumentProxy, type PDFPageProxy, type PdfLib, type RenderTask, type TextContent, type TextLayer,
} from "./pdfjs";

export { MIN_SCALE, MAX_SCALE } from "./layout";

export interface PreviewOptions {
  /** Ctrl-click on the PDF: 1-based page, PDF points with the origin at the bottom-left of the page. */
  onInverseSync(page: number, x: number, y: number): void;
  /** The page at the centre of the view (1-based) or the page count changed. */
  onPageChange?(page: number, pageCount: number): void;
}

export interface PreviewHandle {
  /** Shows `pdfPath`. A reload keeps page, scroll offset and zoom; the swap happens offscreen (no flash). */
  load(pdfPath: string): Promise<void>;
  /** Scrolls the first box to the centre and flashes all boxes briefly (Mac RevealFlash). */
  highlight(boxes: SyncBox[]): void;
  zoomIn(): void;
  zoomOut(): void;
  /** Fits the page width and keeps following the width of the pane. */
  zoomToFit(): void;
  setZoom(scale: number): void;
  /** Drops the document and shows the "No PDF yet" placeholder. */
  clear(): void;
  destroy(): void;
}

export function createPreview(parent: HTMLElement, opts: PreviewOptions): PreviewHandle {
  const preview = new Preview(parent, opts);
  return {
    load: (path) => preview.load(path),
    highlight: (boxes) => preview.highlight(boxes),
    zoomIn: () => preview.zoomStep(1),
    zoomOut: () => preview.zoomStep(-1),
    zoomToFit: () => preview.zoomToFit(),
    setZoom: (scale) => preview.setZoom(scale),
    clear: () => preview.clear(),
    destroy: () => preview.destroy(),
  };
}

// ---------------------------------------------------------------------------------------------------------------

interface PageView extends PageBox {
  index: number;
  proxy: PDFPageProxy;
  rotation: number;
  el: HTMLDivElement;
  canvas: HTMLCanvasElement | null;
  /** Render key (scale × pixel ratio) of `canvas`, 0 = none. */
  key: number;
  task: RenderTask | null;
  taskKey: number;
  textEl: HTMLDivElement | null;
  text: TextLayer | null;
  textScale: number;
  textBusy: boolean;
  content: Promise<TextContent> | null;
  /** Its document was replaced or cleared: late renders are dropped. */
  dead: boolean;
}

interface Flash { page: PageView; x1: number; y1: number; x2: number; y2: number; el: HTMLDivElement }

/** Larger canvases are rendered at a lower pixel ratio (memory and GPU texture limits). */
const MAX_CANVAS_PIXELS = 16_777_216;
/** Pages rendered above / below the visible ones. */
const BUFFER = 1;
/** Canvases further than this many pages from the visible range are freed. */
const KEEP = 3;
const CONCURRENCY = 2;
/** Re-render this long after the last zoom change (pinch / wheel), not during it. */
const SETTLE_MS = 140;
/** A reload waits at most this long for the visible pages before swapping anyway. */
const SWAP_TIMEOUT_MS = 1500;
const HIGHLIGHT_HOLD_MS = 1500;
const HIGHLIGHT_FADE_MS = 500;

const now = (): number => performance.now();
const dpr = (): number => window.devicePixelRatio || 1;
const reduceMotion = (): boolean => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

function pixelRatio(cw: number, ch: number): number {
  const r = dpr();
  const px = cw * ch * r * r;
  return px > MAX_CANVAS_PIXELS ? Math.sqrt(MAX_CANVAS_PIXELS / (cw * ch)) : r;
}

const renderKey = (scale: number, p: PageBox): number => Math.round(scale * pixelRatio(p.cw, p.ch) * 10000) / 10000;

const isCancel = (e: unknown): boolean =>
  !!e && typeof e === "object" && "name" in e && ((e as { name: string }).name === "RenderingCancelledException" || (e as { name: string }).name === "AbortException");

type GestureLike = Event & { scale: number; clientX: number; clientY: number };

class Preview {
  private readonly root = document.createElement("div");
  private readonly scroller = document.createElement("div");
  private readonly docEl = document.createElement("div");
  private readonly placeholder = createPlaceholder();
  private readonly controls: PreviewControls;
  private readonly resizeObserver: ResizeObserver;
  private readonly cleanups: Array<() => void> = [];

  private lib: PdfLib | null = null;
  private doc: PDFDocumentProxy | null = null;
  private pages: PageView[] = [];
  private scale = 1;
  private fits = true;
  private gen = 0;
  private loading = 0;
  private destroyed = false;

  private clientW = 0;
  private clientH = 0;
  private frame = 0;
  private settleTimer = 0;
  private lastZoomAt = 0;
  private queue: PageView[] = [];
  private inflight = 0;
  private pendingZoom: { scale: number; vx: number; vy: number } | null = null;
  private gestureBase = 1;
  private reportedPage = -1;
  private reportedCount = -1;
  private centerIndex = 0;

  private flashes: Flash[] = [];
  private flashTimers: number[] = [];
  private pendingHighlight: SyncBox[] | null = null;

  constructor(parent: HTMLElement, private readonly opts: PreviewOptions) {
    this.root.className = "pv-root";
    this.scroller.className = "pv-scroll";
    this.scroller.tabIndex = 0;
    this.scroller.setAttribute("role", "document");
    this.scroller.setAttribute("aria-label", t("PDF preview"));
    this.docEl.className = "pv-doc";
    this.scroller.append(this.docEl);
    this.controls = new PreviewControls({
      zoomIn: () => this.zoomStep(1),
      zoomOut: () => this.zoomStep(-1),
      fit: () => this.zoomToFit(),
    });
    this.root.append(this.scroller, this.controls.el, this.placeholder);
    parent.append(this.root);

    const on = (target: EventTarget, type: string, fn: (e: Event) => void, options?: AddEventListenerOptions) => {
      target.addEventListener(type, fn, options);
      this.cleanups.push(() => target.removeEventListener(type, fn, options));
    };
    on(this.scroller, "scroll", () => this.schedule(), { passive: true });
    on(this.scroller, "wheel", (e) => this.onWheel(e as WheelEvent), { passive: false });
    on(this.scroller, "mousedown", (e) => this.onMouseDown(e as MouseEvent), { capture: true });
    on(this.scroller, "keydown", (e) => this.onKeyDown(e as KeyboardEvent));
    on(this.scroller, "gesturestart", (e) => { e.preventDefault(); this.gestureBase = this.scale; });
    on(this.scroller, "gesturechange", (e) => this.onGesture(e as GestureLike));
    on(this.scroller, "gestureend", (e) => e.preventDefault());
    on(document, "pointerup", () => this.endSelecting());

    this.resizeObserver = new ResizeObserver(() => this.onResize());
    this.resizeObserver.observe(this.scroller);
    this.measure();
    this.render();
  }

  // MARK: Loading

  async load(path: string): Promise<void> {
    if (this.destroyed) return;
    const gen = ++this.gen;
    this.loading++;
    this.render();
    let doc: PDFDocumentProxy | null = null;
    try {
      const { lib, worker } = await loadPdfJs();
      this.lib = lib;
      const data = await readPdf(path);
      if (gen !== this.gen) return;
      doc = await lib.getDocument({ data: new Uint8Array(data), worker, verbosity: 0 }).promise;
      if (gen !== this.gen) return;
      const proxies = await Promise.all(Array.from({ length: doc.numPages }, (_, i) => doc!.getPage(i + 1)));
      if (gen !== this.gen) return;
      const pages = proxies.map((proxy, index) => makePage(proxy, index));

      // Estimate where the reader will be and render those pages offscreen before swapping.
      this.measure();
      const first = !this.doc;
      const scale = first || this.fits ? fitWidthScale(pages, this.clientW) : this.scale;
      const size = layoutPages(pages, scale, this.clientW);
      const anchor = first ? null : this.captureAnchor(0, 0);
      const pos = anchor ? resolveAnchor(pages, scale, anchor) : null;
      const top = Math.min(Math.max(pos?.y ?? 0, 0), Math.max(0, size.height - this.clientH));
      const a = pageAtY(pages, top);
      const b = Math.min(pageAtY(pages, top + Math.max(this.clientH, 1)), a + 3);
      const pre: Array<Promise<void>> = [];
      for (let i = Math.max(0, a); i <= b; i++) pre.push(this.renderInto(pages[i]!, scale));
      let timer = 0;
      await Promise.race([Promise.all(pre), new Promise<void>((r) => { timer = window.setTimeout(r, SWAP_TIMEOUT_MS); })]);
      clearTimeout(timer);
      if (gen !== this.gen) {
        for (const p of pages) { p.dead = true; this.release(p); }
        return;
      }
      this.swap(doc, pages, first);
      doc = null; // owned by the view now
    } finally {
      if (doc) void doc.loadingTask.destroy();
      this.loading--;
      this.render();
      const pending = this.pendingHighlight;
      if (pending && this.loading === 0 && this.pages.length) {
        this.pendingHighlight = null;
        this.highlight(pending);
      }
    }
  }

  /** Replaces the shown document in one synchronous step (the browser paints only the final state). */
  private swap(doc: PDFDocumentProxy, pages: PageView[], first: boolean): void {
    this.measure();
    const anchor = first ? null : this.captureAnchor(0, 0);
    const oldDoc = this.doc;
    for (const p of this.pages) { p.dead = true; this.release(p); }
    this.clearFlashes();
    this.queue = [];

    this.doc = doc;
    this.pages = pages;
    if (first) this.fits = true;
    this.scale = this.fits ? fitWidthScale(pages, this.clientW) : clampScale(this.scale);
    this.docEl.replaceChildren(...pages.map((p) => p.el));
    this.applyLayout();
    if (anchor) this.restoreAnchor(anchor, 0, 0);
    else { this.scroller.scrollTop = 0; this.scroller.scrollLeft = 0; }
    if (oldDoc) void oldDoc.loadingTask.destroy();

    this.render();
    this.tick();
  }

  clear(): void {
    this.gen++;
    for (const p of this.pages) { p.dead = true; this.release(p); }
    this.clearFlashes();
    this.pendingHighlight = null;
    this.queue = [];
    this.pages = [];
    this.docEl.replaceChildren();
    this.docEl.style.width = this.docEl.style.height = "0px";
    if (this.doc) void this.doc.loadingTask.destroy();
    this.doc = null;
    this.fits = true;
    this.scale = 1;
    this.render();
    this.reportPage();
  }

  destroy(): void {
    if (this.destroyed) return;
    this.clear();
    this.destroyed = true;
    if (this.frame) cancelAnimationFrame(this.frame);
    clearTimeout(this.settleTimer);
    this.resizeObserver.disconnect();
    for (const c of this.cleanups) c();
    this.root.remove();
  }

  // MARK: Layout and reading position

  private measure(): void {
    this.clientW = this.scroller.clientWidth;
    this.clientH = this.scroller.clientHeight;
  }

  private applyLayout(): void {
    const size = layoutPages(this.pages, this.scale, this.clientW);
    this.docEl.style.width = `${size.width}px`;
    this.docEl.style.height = `${size.height}px`;
    const s = String(this.scale);
    for (const p of this.pages) {
      const st = p.el.style;
      st.left = `${p.x}px`;
      st.top = `${p.y}px`;
      st.width = `${p.cw}px`;
      st.height = `${p.ch}px`;
      st.setProperty("--total-scale-factor", s);
      if (p.textEl) sizeTextLayer(p, p.textEl);
    }
    this.layoutFlashes();
  }

  /** Reading position at the view point (vx, vy) (px from the scroller's top-left). */
  private captureAnchor(vx: number, vy: number): Anchor | null {
    return anchorAt(this.pages, this.scale, this.scroller.scrollLeft + vx, this.scroller.scrollTop + vy);
  }

  private restoreAnchor(anchor: Anchor, vx: number, vy: number): void {
    const pos = resolveAnchor(this.pages, this.scale, anchor);
    if (!pos) return;
    this.scroller.scrollLeft = Math.round(pos.x - vx);
    this.scroller.scrollTop = Math.round(pos.y - vy);
  }

  private onResize(): void {
    const oldW = this.clientW;
    const oldH = this.clientH;
    this.measure();
    if (this.clientW <= 1 || this.clientH <= 1) return; // hidden pane: keep everything as it is
    if (!this.pages.length || (oldW === this.clientW && oldH === this.clientH)) { this.schedule(); return; }
    const anchor = anchorAt(this.pages, this.scale, this.scroller.scrollLeft, this.scroller.scrollTop);
    if (this.fits) {
      const fit = fitWidthScale(this.pages, this.clientW);
      if (Math.abs(fit - this.scale) > 0.0005) { this.scale = fit; this.lastZoomAt = now(); }
    }
    this.applyLayout();
    if (anchor) this.restoreAnchor(anchor, 0, 0);
    this.schedule();
  }

  // MARK: Zoom

  zoomStep(direction: 1 | -1): void {
    if (!this.pages.length) return;
    this.zoomTo(stepZoom(this.pendingZoom?.scale ?? this.scale, direction), this.clientW / 2, this.clientH / 2, false);
  }

  setZoom(scale: number): void {
    if (!this.pages.length || !Number.isFinite(scale)) return;
    this.zoomTo(scale, this.clientW / 2, this.clientH / 2, false);
  }

  zoomToFit(): void {
    if (!this.pages.length) return;
    this.measure();
    this.zoomTo(fitWidthScale(this.pages, this.clientW), 0, 0, true);
  }

  /** Zooms keeping the document point under (vx, vy) in place. */
  private zoomTo(scale: number, vx: number, vy: number, fits: boolean): void {
    this.pendingZoom = null;
    const target = clampScale(scale);
    this.fits = fits;
    if (Math.abs(target - this.scale) < 0.00001) { this.render(); return; }
    const anchor = this.captureAnchor(vx, vy);
    this.scale = target;
    this.lastZoomAt = now();
    this.applyLayout();
    if (anchor) this.restoreAnchor(anchor, vx, vy);
    this.render();
    this.schedule();
  }

  private pointerInView(e: { clientX: number; clientY: number }): { vx: number; vy: number } {
    const r = this.scroller.getBoundingClientRect();
    return { vx: e.clientX - r.left, vy: e.clientY - r.top };
  }

  private onWheel(e: WheelEvent): void {
    if (!e.ctrlKey || !this.pages.length) return;
    e.preventDefault();
    let dy = e.deltaY * (e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? this.clientH : 1);
    dy = Math.max(-60, Math.min(60, dy));
    const base = this.pendingZoom?.scale ?? this.scale;
    this.pendingZoom = { scale: clampScale(base * Math.exp(-dy * 0.0035)), ...this.pointerInView(e) };
    this.schedule();
  }

  private onGesture(e: GestureLike): void {
    e.preventDefault();
    if (!this.pages.length || !Number.isFinite(e.scale)) return;
    this.pendingZoom = { scale: clampScale(this.gestureBase * e.scale), ...this.pointerInView(e) };
    this.schedule();
  }

  // MARK: Input

  private onMouseDown(e: MouseEvent): void {
    const target = e.target as Element | null;
    if (target?.closest?.(".textLayer")) target.closest(".textLayer")!.classList.add("selecting");
    if (e.button !== 0 || !(e.ctrlKey || e.metaKey) || e.altKey) return;
    const pageEl = target?.closest?.(".pv-page") as HTMLElement | null;
    const page = pageEl ? this.pages[Number(pageEl.dataset.page)] : undefined;
    if (!pageEl || !page) return;
    // Keep the click from starting a selection (Mac: the ⌘-click is kept from PDFKit).
    e.preventDefault();
    e.stopPropagation();
    this.endSelecting();
    const rect = pageEl.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    const vp = page.proxy.getViewport({ scale: 1 });
    const vx = ((e.clientX - rect.left) * vp.width) / rect.width;
    const vy = ((e.clientY - rect.top) * vp.height) / rect.height;
    const [x, y] = vp.convertToPdfPoint(vx, vy) as [number, number];
    if (Number.isFinite(x) && Number.isFinite(y)) this.opts.onInverseSync(page.index + 1, x, y);
  }

  private endSelecting(): void {
    for (const el of this.docEl.querySelectorAll(".textLayer.selecting")) el.classList.remove("selecting");
  }

  private onKeyDown(e: KeyboardEvent): void {
    if (!this.pages.length || e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
    let target = -1;
    if (e.key === "ArrowLeft") target = this.centerIndex - 1;
    else if (e.key === "ArrowRight") target = this.centerIndex + 1;
    else if (e.key === "Home") target = 0;
    else if (e.key === "End") target = this.pages.length - 1;
    else return;
    e.preventDefault();
    this.goToPage(Math.max(0, Math.min(this.pages.length - 1, target)));
  }

  private goToPage(index: number): void {
    const p = this.pages[index];
    if (!p) return;
    this.scroller.scrollTop = Math.max(0, Math.round(p.y - MARGIN_TOP * this.scale));
  }

  // MARK: Frame loop

  private schedule(): void {
    if (!this.frame && !this.destroyed) this.frame = requestAnimationFrame(() => { this.frame = 0; this.tick(); });
  }

  /** One pass per frame: apply a pending pinch, report the page, free, queue renders and text layers. */
  private tick(): void {
    const zoom = this.pendingZoom;
    if (zoom) this.zoomTo(zoom.scale, zoom.vx, zoom.vy, false);
    const pages = this.pages;
    if (!pages.length) return;

    const top = this.scroller.scrollTop;
    const h = Math.max(this.clientH, 1);
    const first = pageAtY(pages, top);
    const last = pageAtY(pages, top + h);
    this.centerIndex = pageAtY(pages, top + h / 2);
    this.reportPage();
    this.render();

    const sinceZoom = now() - this.lastZoomAt;
    if (sinceZoom < SETTLE_MS) {
      clearTimeout(this.settleTimer);
      this.settleTimer = window.setTimeout(() => this.schedule(), SETTLE_MS - sinceZoom + 5);
      return;
    }

    for (const p of pages) {
      if (p.index < first - KEEP || p.index > last + KEEP) {
        if (p.canvas || p.task || p.textEl) this.release(p);
      } else if (p.textEl && (p.index < first - 1 || p.index > last + 1)) {
        dropText(p);
      }
    }

    const wanted: PageView[] = [];
    for (let i = Math.max(0, first - BUFFER); i <= Math.min(pages.length - 1, last + BUFFER); i++) {
      const p = pages[i]!;
      const key = renderKey(this.scale, p);
      if (p.key !== key && p.taskKey !== key) wanted.push(p);
    }
    const c = this.centerIndex;
    wanted.sort((a, b) => Math.abs(a.index - c) - Math.abs(b.index - c));
    this.queue = wanted;
    this.pump();

    for (let i = first; i <= last; i++) {
      const p = pages[i]!;
      if (p.canvas) void this.ensureText(p);
    }
  }

  private pump(): void {
    while (this.inflight < CONCURRENCY && this.queue.length) {
      const p = this.queue.shift()!;
      if (p.dead) continue;
      const key = renderKey(this.scale, p);
      if (p.key === key || p.taskKey === key) continue;
      this.inflight++;
      void this.renderInto(p, this.scale).finally(() => {
        this.inflight--;
        this.pump();
      });
    }
  }

  // MARK: Rendering

  /** Renders `p` at `scale` into a fresh canvas offscreen, then swaps it in (the old one stays until then). */
  private async renderInto(p: PageView, scale: number): Promise<void> {
    if (p.task) { p.task.cancel(); p.task = null; p.taskKey = 0; }
    const cw = Math.max(1, Math.round(p.w * scale));
    const ch = Math.max(1, Math.round(p.h * scale));
    const ratio = pixelRatio(cw, ch);
    const key = Math.round(scale * ratio * 10000) / 10000;
    const viewport = p.proxy.getViewport({ scale });
    const canvas = document.createElement("canvas");
    canvas.className = "pv-canvas";
    canvas.width = Math.max(1, Math.round(viewport.width * ratio));
    canvas.height = Math.max(1, Math.round(viewport.height * ratio));
    const sx = canvas.width / viewport.width;
    const sy = canvas.height / viewport.height;
    const task = p.proxy.render({
      canvas,
      viewport,
      transform: sx !== 1 || sy !== 1 ? [sx, 0, 0, sy, 0, 0] : undefined,
      background: "#ffffff",
    });
    p.task = task;
    p.taskKey = key;
    try {
      await task.promise;
    } catch (e) {
      canvas.width = canvas.height = 0;
      if (!isCancel(e)) console.warn("PDF page render failed", e);
      return;
    } finally {
      if (p.task === task) { p.task = null; p.taskKey = 0; }
    }
    if (p.dead) { canvas.width = canvas.height = 0; return; }
    if (p.canvas) {
      const old = p.canvas;
      old.replaceWith(canvas);
      old.width = old.height = 0;
    } else {
      p.el.prepend(canvas);
    }
    p.canvas = canvas;
    p.key = key;
    this.schedule(); // text layer for the newly drawn page
  }

  private async ensureText(p: PageView): Promise<void> {
    const lib = this.lib;
    if (!lib || p.textBusy || p.dead) return;
    if (p.textEl && p.textScale === this.scale) return;
    const viewport = p.proxy.getViewport({ scale: this.scale });
    if (p.text && p.textEl) {
      p.text.update({ viewport });
      p.textScale = this.scale;
      sizeTextLayer(p, p.textEl);
      return;
    }
    p.textBusy = true;
    try {
      p.content ??= p.proxy.getTextContent();
      const content = await p.content;
      if (p.dead || p.textEl) return;
      const vp = p.proxy.getViewport({ scale: this.scale });
      const el = document.createElement("div");
      el.className = "textLayer";
      const layer = new lib.TextLayer({ textContentSource: content, container: el, viewport: vp });
      sizeTextLayer(p, el);
      p.el.append(el);
      p.textEl = el;
      p.text = layer;
      p.textScale = this.scale;
      await layer.render();
      if (p.textEl === el) {
        const end = document.createElement("div");
        end.className = "endOfContent";
        el.append(end);
      }
    } catch (e) {
      if (!isCancel(e)) console.warn("PDF text layer failed", e);
    } finally {
      p.textBusy = false;
    }
  }

  private release(p: PageView): void {
    if (p.task) { p.task.cancel(); p.task = null; p.taskKey = 0; }
    if (p.canvas) {
      p.canvas.remove();
      p.canvas.width = p.canvas.height = 0;
      p.canvas = null;
      p.key = 0;
    }
    dropText(p);
  }

  // MARK: Highlights (forward search)

  highlight(boxes: SyncBox[]): void {
    if (this.loading > 0 || !this.pages.length) {
      this.pendingHighlight = boxes; // shown once the document being loaded is on screen
      return;
    }
    this.clearFlashes();
    for (const b of boxes) {
      const page = this.pages[b.page - 1];
      if (!page || ![b.x, b.y, b.width, b.height].every(Number.isFinite)) continue;
      const vp = page.proxy.getViewport({ scale: 1 });
      const [ax, ay] = vp.convertToViewportPoint(b.x, b.y) as [number, number];
      const [bx, by] = vp.convertToViewportPoint(b.x + b.width, b.y + b.height) as [number, number];
      const el = document.createElement("div");
      el.className = reduceMotion() ? "pv-flash pv-flash-static" : "pv-flash";
      this.flashes.push({ page, x1: Math.min(ax, bx), y1: Math.min(ay, by), x2: Math.max(ax, bx), y2: Math.max(ay, by), el });
    }
    const firstFlash = this.flashes[0];
    if (!firstFlash) return;

    this.measure();
    const s = this.scale;
    const cx = firstFlash.page.x + ((firstFlash.x1 + firstFlash.x2) / 2) * s;
    const cy = firstFlash.page.y + ((firstFlash.y1 + firstFlash.y2) / 2) * s;
    this.scroller.scrollLeft = Math.round(cx - this.clientW / 2);
    this.scroller.scrollTop = Math.round(cy - this.clientH / 2);
    this.layoutFlashes();
    this.docEl.append(...this.flashes.map((f) => f.el));

    const flashes = this.flashes;
    const finish = () => {
      for (const f of flashes) f.el.remove();
      if (this.flashes === flashes) this.flashes = [];
    };
    if (reduceMotion()) {
      this.flashTimers.push(window.setTimeout(finish, HIGHLIGHT_HOLD_MS));
    } else {
      this.flashTimers.push(window.setTimeout(() => {
        for (const f of flashes) f.el.classList.add("pv-flash-out");
        this.flashTimers.push(window.setTimeout(finish, HIGHLIGHT_FADE_MS + 50));
      }, HIGHLIGHT_HOLD_MS));
    }
    this.schedule();
  }

  /** Positions the flashes in document px: a little air around the glyphs, never thinner than 8 px,
   *  snapped outward to the device pixel grid. */
  private layoutFlashes(): void {
    if (!this.flashes.length) return;
    const s = this.scale;
    const r = dpr();
    for (const f of this.flashes) {
      let x = f.page.x + f.x1 * s - 2.5;
      let y = f.page.y + f.y1 * s - 1.5;
      let w = (f.x2 - f.x1) * s + 5;
      let h = (f.y2 - f.y1) * s + 3;
      if (w < 8) { x -= (8 - w) / 2; w = 8; }
      if (h < 8) { y -= (8 - h) / 2; h = 8; }
      const x0 = Math.floor(x * r) / r;
      const y0 = Math.floor(y * r) / r;
      const x1 = Math.ceil((x + w) * r) / r;
      const y1 = Math.ceil((y + h) * r) / r;
      const st = f.el.style;
      st.left = `${x0}px`;
      st.top = `${y0}px`;
      st.width = `${x1 - x0}px`;
      st.height = `${y1 - y0}px`;
      st.borderRadius = `${Math.min(5, Math.min(x1 - x0, y1 - y0) / 2)}px`;
    }
  }

  private clearFlashes(): void {
    for (const id of this.flashTimers) clearTimeout(id);
    this.flashTimers = [];
    for (const f of this.flashes) f.el.remove();
    this.flashes = [];
  }

  // MARK: Chrome

  private render(): void {
    const has = this.pages.length > 0;
    this.root.classList.toggle("pv-has-doc", has);
    this.placeholder.hidden = has || this.loading > 0;
    const total = this.pages.length;
    const page = total ? Math.min(Math.max(this.centerIndex + 1, 1), total) : 0;
    this.controls.update(this.scale, page, total, has && this.scale < MAX_SCALE - 0.001, has && this.scale > MIN_SCALE + 0.001, this.fits);
  }

  private reportPage(): void {
    const total = this.pages.length;
    const page = total ? Math.min(this.centerIndex + 1, total) : 0;
    if (page === this.reportedPage && total === this.reportedCount) return;
    this.reportedPage = page;
    this.reportedCount = total;
    this.opts.onPageChange?.(page, total);
  }
}

function makePage(proxy: PDFPageProxy, index: number): PageView {
  const vp = proxy.getViewport({ scale: 1 });
  const el = document.createElement("div");
  el.className = "pv-page";
  el.dataset.page = String(index);
  return {
    index, proxy, rotation: vp.rotation, el, w: vp.width, h: vp.height, x: 0, y: 0, cw: 1, ch: 1,
    canvas: null, key: 0, task: null, taskKey: 0, textEl: null, text: null, textScale: 0, textBusy: false,
    content: null, dead: false,
  };
}

/** pdf.js sizes the text layer with CSS `round()`; set plain pixels instead (unrotated page size, pdf.js rotates). */
function sizeTextLayer(p: PageView, el: HTMLElement): void {
  const turned = p.rotation % 180 !== 0;
  el.style.width = `${turned ? p.ch : p.cw}px`;
  el.style.height = `${turned ? p.cw : p.ch}px`;
}

function dropText(p: PageView): void {
  p.text?.cancel();
  p.textEl?.remove();
  p.text = null;
  p.textEl = null;
  p.textScale = 0;
}

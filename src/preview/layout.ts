// Pure geometry of the preview (no DOM): PDFKit-like continuous layout, zoom steps and reading anchors.
// Units: "pt" are PDF points at scale 1 (already rotated as displayed); "px" are CSS pixels.

export const MIN_SCALE = 0.25;
export const MAX_SCALE = 8;
/** Preset zoom levels used by zoom in / out (PDFPreviewController.zoomStops). */
export const ZOOM_STOPS = [0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4, 6, 8];

/** PDFKit's `pageBreakMargins` (top 8, sides 10, bottom 8), measured in page space so they grow with the zoom. */
export const MARGIN_TOP = 8;
export const MARGIN_SIDE = 10;

export const clampScale = (s: number): number => Math.min(MAX_SCALE, Math.max(MIN_SCALE, s));

/** Next preset above (`direction` 1) or below (-1) `current`. */
export function stepZoom(current: number, direction: 1 | -1): number {
  if (direction > 0) return ZOOM_STOPS.find((z) => z > current * 1.004) ?? MAX_SCALE;
  for (let i = ZOOM_STOPS.length - 1; i >= 0; i--) {
    const z = ZOOM_STOPS[i]!;
    if (z < current * 0.996) return z;
  }
  return MIN_SCALE;
}

export interface PageBox {
  /** Size in pt (rotation applied). */ w: number; h: number;
  /** Laid-out frame in px inside the document element. */ x: number; y: number; cw: number; ch: number;
}

/** The scale at which the widest page plus its side margins exactly fills `visibleWidth`. */
export function fitWidthScale(pages: readonly PageBox[], visibleWidth: number): number {
  let maxW = 0;
  for (const p of pages) maxW = Math.max(maxW, p.w);
  if (maxW <= 0 || visibleWidth <= 1) return 1;
  return clampScale(visibleWidth / (maxW + 2 * MARGIN_SIDE));
}

/** Lays the pages out top to bottom, centred horizontally; returns the document size in px. */
export function layoutPages(pages: PageBox[], scale: number, clientWidth: number): { width: number; height: number } {
  let maxW = 0;
  for (const p of pages) maxW = Math.max(maxW, p.w);
  const width = Math.max(Math.ceil((maxW + 2 * MARGIN_SIDE) * scale), Math.floor(clientWidth));
  let y = MARGIN_TOP * scale;
  for (const p of pages) {
    p.cw = Math.max(1, Math.round(p.w * scale));
    p.ch = Math.max(1, Math.round(p.h * scale));
    p.x = Math.round((width - p.cw) / 2);
    p.y = Math.round(y);
    y += p.h * scale + 2 * MARGIN_TOP * scale;
  }
  const height = pages.length ? Math.ceil(y - MARGIN_TOP * scale) : 0;
  return { width, height };
}

/** Index of the last page whose top is at or above `docY` (the page under that line, or the one above a gap). */
export function pageAtY(pages: readonly PageBox[], docY: number): number {
  let lo = 0;
  let hi = pages.length - 1;
  if (hi < 0) return -1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (pages[mid]!.y <= docY) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/** A reading position that survives zoom changes and document swaps: page + offset in pt from its top-left. */
export interface Anchor { index: number; dx: number; dy: number }

export function anchorAt(pages: readonly PageBox[], scale: number, docX: number, docY: number): Anchor | null {
  const index = pageAtY(pages, docY);
  const p = pages[index];
  if (!p) return null;
  return { index, dx: (docX - p.x) / scale, dy: (docY - p.y) / scale };
}

/** Document coordinates (px) of `anchor` in the current layout. A page that no longer exists falls back to the
 *  top of the last page (the document got shorter than the reader's position). */
export function resolveAnchor(pages: readonly PageBox[], scale: number, anchor: Anchor): { x: number; y: number } | null {
  if (!pages.length) return null;
  const index = Math.min(Math.max(anchor.index, 0), pages.length - 1);
  const p = pages[index]!;
  const dy = index === anchor.index ? anchor.dy : -MARGIN_TOP;
  return { x: p.x + anchor.dx * scale, y: p.y + dy * scale };
}

// ---- Crisp rendering at high zoom: only the visible part of a huge page is rasterised ----

/** A rectangle in CSS px relative to the top-left of a page. */
export interface Region { x: number; y: number; w: number; h: number }

export const regionContains = (outer: Region, inner: Region): boolean =>
  inner.x >= outer.x && inner.y >= outer.y && inner.x + inner.w <= outer.x + outer.w && inner.y + inner.h <= outer.y + outer.h;

/** The part of page `p` inside the view rectangle (document px), or null when the page is off screen. */
export function visiblePart(p: PageBox, viewX: number, viewY: number, viewW: number, viewH: number): Region | null {
  const x0 = Math.max(0, viewX - p.x);
  const y0 = Math.max(0, viewY - p.y);
  const x1 = Math.min(p.cw, viewX + viewW - p.x);
  const y1 = Math.min(p.ch, viewY + viewH - p.y);
  return x1 > x0 && y1 > y0 ? { x: x0, y: y0, w: x1 - x0, h: y1 - y0 } : null;
}

/**
 * The region to rasterise for a page too big for one canvas: the visible part plus as much margin (up to 40 % of
 * the view on each side) as fits `maxPixels` at pixel ratio `ratio`. Integer CSS px, clamped to the page.
 */
export function tileRegion(visible: Region, cw: number, ch: number, viewW: number, viewH: number, ratio: number, maxPixels: number): Region {
  let m = 0.4;
  let region: Region = { x: 0, y: 0, w: 0, h: 0 };
  for (let i = 0; i < 10; i++) {
    const mx = Math.round(viewW * m);
    const my = Math.round(viewH * m);
    const x0 = Math.max(0, Math.floor(visible.x - mx));
    const y0 = Math.max(0, Math.floor(visible.y - my));
    const x1 = Math.min(cw, Math.ceil(visible.x + visible.w + mx));
    const y1 = Math.min(ch, Math.ceil(visible.y + visible.h + my));
    region = { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
    if (region.w * region.h * ratio * ratio <= maxPixels) return region;
    m *= 0.55;
  }
  const x0 = Math.max(0, Math.floor(visible.x));
  const y0 = Math.max(0, Math.floor(visible.y));
  return { x: x0, y: y0, w: Math.min(cw, Math.ceil(visible.x + visible.w)) - x0, h: Math.min(ch, Math.ceil(visible.y + visible.h)) - y0 };
}

/**
 * Size of pdf.js's text / annotation layer container: it is laid out unrotated (raw page size) and turned by the
 * `data-main-rotation` CSS, so for 90° / 270° pages its width is the displayed height.
 */
export function textLayerSize(rotation: number, cw: number, ch: number): { width: number; height: number } {
  const turned = ((Math.round(rotation / 90) % 4) + 4) % 2 === 1;
  return turned ? { width: ch, height: cw } : { width: cw, height: ch };
}

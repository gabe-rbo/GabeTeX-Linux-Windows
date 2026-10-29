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

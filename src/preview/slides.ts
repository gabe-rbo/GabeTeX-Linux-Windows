// Slide rendering for presentation mode (port of PreviewKit/Presentation/SlideRenderer.swift): opens a PDF with the
// shared pdf.js worker and renders one region of a page (the whole page, or the slide / notes half of a Beamer
// "notes on second screen" page) into a canvas at device pixel resolution. Small LRU cache so moving to the next
// slide shows an already rendered canvas.
import { readPdf } from "../ipc";
import { loadPdfJs, type PDFDocumentProxy, type PDFPageProxy, type RenderTask } from "./pdfjs";

export type SlideRegion = "whole" | "leftHalf" | "rightHalf" | "bottomHalf" | "topHalf";

export interface SlideDocument {
  readonly pageCount: number;
  /** Page sizes in points (crop box, rotation applied). */
  readonly sizes: ReadonlyArray<{ width: number; height: number }>;
  /** Renders `region` of page `index` (0-based) for a box of `cssWidth` × `cssHeight` CSS pixels. Cached. */
  render(index: number, region: SlideRegion, cssWidth: number, cssHeight: number): Promise<HTMLCanvasElement>;
  destroy(): void;
}

const UNIT: Record<SlideRegion, [number, number, number, number]> = {
  // x, y (from the bottom), width, height in unit page coordinates
  whole: [0, 0, 1, 1], leftHalf: [0, 0, 0.5, 1], rightHalf: [0.5, 0, 0.5, 1], bottomHalf: [0, 0, 1, 0.5], topHalf: [0, 0.5, 1, 0.5],
};
const CACHE_SIZE = 8;
const MAX_PIXELS = 16_777_216;

/** Opens `path` (read through the backend) for slide rendering. */
export async function openSlideDocument(path: string): Promise<SlideDocument> {
  const { lib, worker } = await loadPdfJs();
  const data = await readPdf(path);
  const doc: PDFDocumentProxy = await lib.getDocument({ data: new Uint8Array(data), worker, verbosity: 0 }).promise;
  const pages: PDFPageProxy[] = await Promise.all(Array.from({ length: doc.numPages }, (_, i) => doc.getPage(i + 1)));
  const sizes = pages.map((p) => {
    const v = p.getViewport({ scale: 1 });
    return { width: v.width, height: v.height };
  });
  const cache = new Map<string, Promise<HTMLCanvasElement>>();
  const tasks = new Set<RenderTask>();
  let dead = false;

  const draw = async (index: number, region: SlideRegion, w: number, h: number): Promise<HTMLCanvasElement> => {
    const page = pages[index];
    if (!page || dead) throw new Error("No such page");
    const [ux, uy, uw, uh] = UNIT[region];
    const base = sizes[index]!;
    let ratio = window.devicePixelRatio || 1;
    if (w * h * ratio * ratio > MAX_PIXELS) ratio = Math.sqrt(MAX_PIXELS / (w * h));
    // Scale so that the region fills the box (the caller sized the box to the region's aspect).
    const scale = Math.min((w * ratio) / (base.width * uw), (h * ratio) / (base.height * uh));
    const viewport = page.getViewport({ scale, offsetX: -ux * base.width * scale, offsetY: -(1 - uy - uh) * base.height * scale });
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(base.width * uw * scale));
    canvas.height = Math.max(1, Math.round(base.height * uh * scale));
    canvas.style.width = `${w}px`;
    canvas.style.height = `${h}px`;
    const task = page.render({ canvas, viewport, background: "#ffffff" });
    tasks.add(task);
    try {
      await task.promise;
    } finally {
      tasks.delete(task);
    }
    return canvas;
  };

  return {
    pageCount: pages.length,
    sizes,
    render(index, region, cssWidth, cssHeight) {
      const w = Math.max(1, Math.round(cssWidth));
      const h = Math.max(1, Math.round(cssHeight));
      const key = `${index}|${region}|${w}x${h}|${window.devicePixelRatio || 1}`;
      let hit = cache.get(key);
      if (hit) {
        cache.delete(key);
        cache.set(key, hit);
        return hit;
      }
      hit = draw(index, region, w, h);
      hit.catch(() => cache.delete(key));
      cache.set(key, hit);
      while (cache.size > CACHE_SIZE) cache.delete(cache.keys().next().value!);
      return hit;
    },
    destroy() {
      dead = true;
      for (const t of tasks) t.cancel();
      cache.clear();
      void doc.loadingTask.destroy();
    },
  };
}

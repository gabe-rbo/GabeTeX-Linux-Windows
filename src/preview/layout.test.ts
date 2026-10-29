import { describe, expect, it } from "vitest";
import {
  anchorAt, fitWidthScale, layoutPages, pageAtY, regionContains, resolveAnchor, stepZoom, textLayerSize, tileRegion, visiblePart,
  type PageBox,
} from "./layout";

const pages = (n: number): PageBox[] => Array.from({ length: n }, () => ({ w: 612, h: 792, x: 0, y: 0, cw: 0, ch: 0 }));

describe("preview layout", () => {
  it("fits the page width plus PDFKit's side margins", () => {
    expect(fitWidthScale(pages(1), 632)).toBeCloseTo(1);
  });
  it("steps through the Mac zoom stops", () => {
    expect(stepZoom(1, 1)).toBe(1.1);
    expect(stepZoom(1, -1)).toBe(0.9);
    expect(stepZoom(0.93, 1)).toBe(1);
    expect(stepZoom(8, 1)).toBe(8);
    expect(stepZoom(0.25, -1)).toBe(0.25);
  });
  it("stacks pages with margins that scale with the zoom", () => {
    const ps = pages(3);
    const size = layoutPages(ps, 2, 100);
    expect(ps[0]!.y).toBe(16);
    expect(ps[1]!.y).toBe(16 + 792 * 2 + 32);
    expect(size.width).toBe((612 + 20) * 2);
    expect(size.height).toBe(16 + 3 * 792 * 2 + 2 * 32 + 16);
    expect(pageAtY(ps, ps[1]!.y - 1)).toBe(0);
    expect(pageAtY(ps, ps[1]!.y)).toBe(1);
  });
  it("keeps the reading position across zoom changes and shorter documents", () => {
    const ps = pages(3);
    layoutPages(ps, 1, 800);
    const a = anchorAt(ps, 1, ps[1]!.x + 100, ps[1]!.y + 200)!;
    layoutPages(ps, 2, 800);
    expect(resolveAnchor(ps, 2, a)).toEqual({ x: ps[1]!.x + 200, y: ps[1]!.y + 400 });
    const short = pages(1);
    layoutPages(short, 1, 800);
    expect(resolveAnchor(short, 1, { index: 2, dx: 0, dy: 50 })!.y).toBe(short[0]!.y - 8);
  });
});

describe("tiled rendering", () => {
  const page: PageBox = { w: 595, h: 842, x: 100, y: 50, cw: 4760, ch: 6736 };
  it("finds the visible part of a page", () => {
    expect(visiblePart(page, 0, 0, 800, 600)).toEqual({ x: 0, y: 0, w: 700, h: 550 });
    expect(visiblePart(page, 0, 7000, 800, 600)).toBeNull();
    expect(visiblePart(page, 1000, 2000, 800, 600)).toEqual({ x: 900, y: 1950, w: 800, h: 600 });
  });
  it("keeps tiles within the pixel budget and around the visible area", () => {
    const vis = { x: 900, y: 1950, w: 800, h: 600 };
    const r = tileRegion(vis, page.cw, page.ch, 800, 600, 2, 16_777_216);
    expect(r.w * r.h * 4).toBeLessThanOrEqual(16_777_216);
    expect(regionContains(r, vis)).toBe(true);
    const edge = tileRegion({ x: 0, y: 0, w: 700, h: 550 }, page.cw, page.ch, 800, 600, 1, 16_777_216);
    expect(edge.x).toBe(0);
    expect(edge.y).toBe(0);
  });
  it("sizes the text layer for rotated pages", () => {
    expect(textLayerSize(0, 600, 800)).toEqual({ width: 600, height: 800 });
    expect(textLayerSize(90, 800, 600)).toEqual({ width: 600, height: 800 });
    expect(textLayerSize(180, 600, 800)).toEqual({ width: 600, height: 800 });
    expect(textLayerSize(270, 800, 600)).toEqual({ width: 600, height: 800 });
    expect(textLayerSize(-90, 800, 600)).toEqual({ width: 600, height: 800 });
  });
});

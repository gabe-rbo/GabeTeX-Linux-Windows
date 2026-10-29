import { describe, expect, it } from "vitest";
import { anchorAt, fitWidthScale, layoutPages, pageAtY, resolveAnchor, stepZoom, type PageBox } from "./layout";

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

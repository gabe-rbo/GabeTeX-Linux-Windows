import { describe, expect, it } from "vitest";
import {
  Navigator, aspectFit, commandForKey, formatElapsed, notesSideFor, planScreens, regionsFor, scanPresentationSource,
  timerElapsed, timerReset, timerToggle,
} from "./logic";

describe("Navigator", () => {
  it("clamps moves and never ends by itself", () => {
    const n = new Navigator(3);
    expect(n.perform({ kind: "previous" })).toEqual({ kind: "none" });
    n.perform({ kind: "last" });
    expect(n.currentIndex).toBe(2);
    expect(n.perform({ kind: "next" })).toEqual({ kind: "none" });
  });
  it("brings the slides back from a blank screen without moving", () => {
    const n = new Navigator(3);
    n.perform({ kind: "toggleBlack" });
    expect(n.perform({ kind: "next" })).toEqual({ kind: "blankChanged", blank: "none" });
    expect(n.currentIndex).toBe(0);
  });
  it("jumps to a typed slide number", () => {
    const n = new Navigator(12);
    n.perform({ kind: "digit", value: 1 });
    n.perform({ kind: "digit", value: 0 });
    expect(n.typedTargetIndex).toBe(9);
    expect(n.perform({ kind: "confirm" })).toEqual({ kind: "moved", from: 0, to: 9 });
    n.perform({ kind: "digit", value: 9 });
    n.perform({ kind: "digit", value: 9 });
    n.perform({ kind: "confirm" });
    expect(n.currentIndex).toBe(11);
    n.perform({ kind: "digit", value: 3 });
    expect(n.perform({ kind: "escape" })).toEqual({ kind: "digitsChanged" });
    expect(n.perform({ kind: "escape" })).toEqual({ kind: "end" });
  });
  it("keeps the slide on reload", () => {
    const n = new Navigator(10, 8);
    n.updatePageCount(5);
    expect(n.currentIndex).toBe(4);
  });
});

describe("timer and keys", () => {
  it("counts, pauses and formats", () => {
    let s = timerToggle({ accumulated: 0, runningSince: null }, 1000);
    expect(timerElapsed(s, 6000)).toBe(5000);
    s = timerToggle(s, 6000);
    expect(timerElapsed(s, 99999)).toBe(5000);
    s = timerReset(s, 7000);
    expect(s).toEqual({ accumulated: 0, runningSince: null });
    expect(formatElapsed(247_000)).toBe("04:07");
    expect(formatElapsed(3_723_000)).toBe("1:02:03");
  });
  it("maps keys", () => {
    expect(commandForKey({ key: " ", shift: true })).toEqual({ kind: "previous" });
    expect(commandForKey({ key: "7" })).toEqual({ kind: "digit", value: 7 });
    expect(commandForKey({ key: "B" })).toEqual({ kind: "toggleBlack" });
    expect(commandForKey({ key: "Enter", ctrl: true, alt: true })).toEqual({ kind: "end" });
    expect(commandForKey({ key: "p", ctrl: true })).toBeNull();
  });
});

describe("geometry", () => {
  it("splits Beamer notes pages", () => {
    const wide = { width: 2 * 364, height: 273 };
    expect(notesSideFor("automatic", wide, null)).toBe("right");
    expect(notesSideFor("automatic", wide, "left")).toBe("left");
    expect(notesSideFor("automatic", { width: 364, height: 273 }, "right")).toBeNull();
    expect(notesSideFor("off", wide, "right")).toBeNull();
    expect(regionsFor(wide, "right")).toEqual({ slide: "leftHalf", notes: "rightHalf" });
    expect(regionsFor({ width: 364, height: 273 }, "right")).toEqual({ slide: "whole", notes: null });
  });
  it("fits and plans displays", () => {
    expect(aspectFit({ width: 16, height: 9 }, { x: 0, y: 0, width: 160, height: 160 })).toEqual({ x: 0, y: 35, width: 160, height: 90 });
    expect(planScreens([{ id: "a", isBuiltIn: false }], null, false)).toEqual({ audience: "a", presenter: null });
    expect(planScreens([{ id: "a", isBuiltIn: false }, { id: "b", isBuiltIn: true }], null, false)).toEqual({ audience: "a", presenter: "b" });
    expect(planScreens([{ id: "a", isBuiltIn: false }, { id: "b", isBuiltIn: false }], "b", true)).toEqual({ audience: "b", presenter: "a" });
  });
  it("reads the preamble", () => {
    const src = "% \\documentclass{article}\n\\documentclass[11pt]{beamer}\n\\setbeameroption{show notes on second screen=left}\n\\begin{document}\n\\setbeameroption{show notes on second screen=top}";
    expect(scanPresentationSource(src)).toEqual({ documentClass: "beamer", notesSide: "left" });
    expect(scanPresentationSource("\\documentclass{article}")).toEqual({ documentClass: "article", notesSide: null });
  });
});

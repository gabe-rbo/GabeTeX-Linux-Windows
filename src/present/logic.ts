// Pure presentation logic (port of Sources/PreviewKit/Presentation/PresentationLogic.swift and SlideGeometry.swift):
// the navigation state machine, slide-number entry, the timer, the keyboard model, the Beamer notes split, aspect
// fitting, which display gets which window, and what the root document's preamble says about presenting.
// No DOM and no Tauri: covered by logic.test.ts.

// ---- commands ---------------------------------------------------------------------------------------------------

export type PresentationCommand =
  | { kind: "next" } | { kind: "previous" } | { kind: "first" } | { kind: "last" }
  | { kind: "digit"; value: number } | { kind: "deleteBackward" } | { kind: "confirm" }
  | { kind: "toggleBlack" } | { kind: "toggleWhite" } | { kind: "escape" } | { kind: "end" }
  | { kind: "togglePresenterView" } | { kind: "goTo"; index: number } | { kind: "clearDigits" };

export type Blank = "none" | "black" | "white";

export type Outcome =
  | { kind: "none" } | { kind: "moved"; from: number; to: number } | { kind: "blankChanged"; blank: Blank }
  | { kind: "digitsChanged" } | { kind: "end" } | { kind: "togglePresenterView" };

const NONE: Outcome = { kind: "none" };
const DIGITS: Outcome = { kind: "digitsChanged" };

// ---- navigation -------------------------------------------------------------------------------------------------

/**
 * Current slide, blank screen and the slide number being typed. Moving is clamped (the last slide stays when
 * advancing past it); while blanked, a navigation key only brings the slides back; digits + Return jump.
 */
export class Navigator {
  static readonly maximumDigits = 5;
  pageCount: number;
  currentIndex: number;
  blank: Blank = "none";
  typedDigits = "";

  constructor(pageCount: number, startIndex = 0) {
    this.pageCount = Math.max(0, pageCount);
    this.currentIndex = clampIndex(startIndex, this.pageCount);
  }

  get nextIndex(): number | null { return this.currentIndex + 1 < this.pageCount ? this.currentIndex + 1 : null; }
  get previousIndex(): number | null { return this.currentIndex > 0 && this.pageCount > 0 ? this.currentIndex - 1 : null; }
  get typedTargetIndex(): number | null {
    if (!this.typedDigits || this.pageCount <= 0) return null;
    return clampIndex(Number.parseInt(this.typedDigits, 10) - 1, this.pageCount);
  }

  perform(c: PresentationCommand): Outcome {
    switch (c.kind) {
      case "next": return this.navigate(this.currentIndex + 1);
      case "previous": return this.navigate(this.currentIndex - 1);
      case "first": return this.navigate(0);
      case "last": return this.navigate(this.pageCount - 1);
      case "digit":
        if (c.value < 0 || c.value > 9 || this.typedDigits.length >= Navigator.maximumDigits || this.pageCount <= 0) return NONE;
        this.typedDigits += String(c.value);
        return DIGITS;
      case "deleteBackward":
        if (!this.typedDigits) return this.navigate(this.currentIndex - 1);
        this.typedDigits = this.typedDigits.slice(0, -1);
        return DIGITS;
      case "confirm": {
        const target = this.typedTargetIndex;
        if (target === null) {
          if (this.typedDigits) { this.typedDigits = ""; return DIGITS; }
          return this.navigate(this.currentIndex + 1);
        }
        this.typedDigits = "";
        return this.jump(target, true);
      }
      case "toggleBlack": return this.setBlank(this.blank === "black" ? "none" : "black");
      case "toggleWhite": return this.setBlank(this.blank === "white" ? "none" : "white");
      case "escape":
        if (!this.typedDigits) return { kind: "end" };
        this.typedDigits = "";
        return DIGITS;
      case "end": this.typedDigits = ""; return { kind: "end" };
      case "togglePresenterView": return { kind: "togglePresenterView" };
      case "goTo": this.typedDigits = ""; return this.jump(c.index, false);
      case "clearDigits":
        if (!this.typedDigits) return NONE;
        this.typedDigits = "";
        return DIGITS;
    }
  }

  /** A reload changed the page count: keeps the slide index, clamped. */
  updatePageCount(count: number): void {
    this.pageCount = Math.max(0, count);
    this.currentIndex = clampIndex(this.currentIndex, this.pageCount);
    if (this.pageCount === 0) this.typedDigits = "";
  }

  private navigate(index: number): Outcome {
    const hadDigits = !!this.typedDigits;
    this.typedDigits = "";
    if (this.blank !== "none") { this.blank = "none"; return { kind: "blankChanged", blank: "none" }; }
    if (this.pageCount <= 0) return hadDigits ? DIGITS : NONE;
    const target = clampIndex(index, this.pageCount);
    if (target === this.currentIndex) return hadDigits ? DIGITS : NONE;
    const from = this.currentIndex;
    this.currentIndex = target;
    return { kind: "moved", from, to: target };
  }

  private jump(index: number, reportsDigits: boolean): Outcome {
    const wasBlank = this.blank !== "none";
    this.blank = "none";
    if (this.pageCount <= 0) return wasBlank ? { kind: "blankChanged", blank: "none" } : reportsDigits ? DIGITS : NONE;
    const target = clampIndex(index, this.pageCount);
    if (target !== this.currentIndex) {
      const from = this.currentIndex;
      this.currentIndex = target;
      return { kind: "moved", from, to: target };
    }
    if (wasBlank) return { kind: "blankChanged", blank: "none" };
    return reportsDigits ? DIGITS : NONE;
  }

  private setBlank(value: Blank): Outcome {
    this.typedDigits = "";
    if (value === this.blank) return NONE;
    this.blank = value;
    return { kind: "blankChanged", blank: value };
  }
}

export function clampIndex(index: number, count: number): number {
  if (count <= 0) return 0;
  return Math.min(Math.max(index, 0), count - 1);
}

// ---- timer ------------------------------------------------------------------------------------------------------

/** The elapsed-time stopwatch as plain data (so both presentation windows compute the same value). */
export interface TimerState { accumulated: number; runningSince: number | null }

export const timerElapsed = (s: TimerState, now: number): number =>
  Math.max(0, s.accumulated + (s.runningSince !== null ? Math.max(0, now - s.runningSince) : 0));

export function timerToggle(s: TimerState, now: number): TimerState {
  if (s.runningSince === null) return { accumulated: s.accumulated, runningSince: now };
  return { accumulated: s.accumulated + Math.max(0, now - s.runningSince), runningSince: null };
}

export const timerReset = (s: TimerState, now: number): TimerState => ({ accumulated: 0, runningSince: s.runningSince !== null ? now : null });

/** "04:07" below an hour, "1:02:03" from an hour on (milliseconds in, whole seconds rounded down). */
export function formatElapsed(ms: number): string {
  const seconds = Number.isFinite(ms) ? Math.max(0, Math.floor(ms / 1000)) : 0;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  const two = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${two(m)}:${two(s)}` : `${two(m)}:${two(s)}`;
}

// ---- keyboard ---------------------------------------------------------------------------------------------------

export interface KeyPress { key: string; code?: string; shift?: boolean; alt?: boolean; ctrl?: boolean; meta?: boolean }

/**
 * The presentation keyboard (the same in every presentation window): → ↓ Space PageDown next (⇧Space previous) ·
 * Enter go to the typed slide, else next · ← ↑ PageUp previous · Backspace/Delete remove a digit, else previous ·
 * Home/End · 0–9 type a slide number · B . black · W , white · P presenter view · Esc cancel / end ·
 * Ctrl+Alt+Enter (the shortcut that starts a presentation) ends it. Other Ctrl/Meta combinations are not ours.
 */
export function commandForKey(k: KeyPress): PresentationCommand | null {
  if (k.ctrl || k.meta) {
    if (k.ctrl && k.alt && k.key === "Enter") return { kind: "end" };
    return null;
  }
  switch (k.key) {
    case "ArrowRight": case "ArrowDown": case "PageDown": return { kind: "next" };
    case " ": case "Spacebar": return k.shift ? { kind: "previous" } : { kind: "next" };
    case "Enter": return { kind: "confirm" };
    case "ArrowLeft": case "ArrowUp": case "PageUp": return { kind: "previous" };
    case "Backspace": case "Delete": return { kind: "deleteBackward" };
    case "Home": return { kind: "first" };
    case "End": return { kind: "last" };
    case "Escape": return { kind: "escape" };
  }
  if (k.key.length !== 1) return null;
  const ch = k.key.toLowerCase();
  if (ch >= "0" && ch <= "9") return { kind: "digit", value: ch.charCodeAt(0) - 48 };
  switch (ch) {
    case "b": case ".": return { kind: "toggleBlack" };
    case "w": case ",": return { kind: "toggleWhite" };
    case "p": return { kind: "togglePresenterView" };
  }
  return null;
}

// ---- Beamer notes -----------------------------------------------------------------------------------------------

export type NotesSide = "left" | "right" | "top" | "bottom";
export type NotesMode = "automatic" | "right" | "left" | "off";
export const NOTES_MODES: NotesMode[] = ["automatic", "right", "left", "off"];
export type Region = "whole" | "leftHalf" | "rightHalf" | "bottomHalf" | "topHalf";
export interface Size { width: number; height: number }
export interface Rect { x: number; y: number; width: number; height: number }

/** Unit rectangle of a region (origin bottom-left, like PDF page space). */
export function unitRect(r: Region): Rect {
  switch (r) {
    case "whole": return { x: 0, y: 0, width: 1, height: 1 };
    case "leftHalf": return { x: 0, y: 0, width: 0.5, height: 1 };
    case "rightHalf": return { x: 0.5, y: 0, width: 0.5, height: 1 };
    case "bottomHalf": return { x: 0, y: 0, width: 1, height: 0.5 };
    case "topHalf": return { x: 0, y: 0.5, width: 1, height: 0.5 };
  }
}

export function regionSize(r: Region, page: Size): Size {
  const u = unitRect(r);
  return { width: page.width * u.width, height: page.height * u.height };
}

/** A page at least this many times wider than tall is a slide with its notes beside it. */
export const DOUBLE_WIDTH_RATIO = 2.2;
export const isDoubleWidth = (s: Size): boolean => s.width > 0 && s.height > 0 && s.width / s.height >= DOUBLE_WIDTH_RATIO;
export const isDoubleHeight = (s: Size): boolean => s.width > 0 && s.height > s.width;

/** Where the notes are for a document whose first page has `page`, or null when pages are not split. */
export function notesSideFor(mode: NotesMode, page: Size, hint: NotesSide | null): NotesSide | null {
  const wide = isDoubleWidth(page);
  switch (mode) {
    case "off": return null;
    case "right": return wide ? "right" : null;
    case "left": return wide ? "left" : null;
    case "automatic":
      if (hint === "left" || hint === "right") return wide ? hint : null;
      if (hint === "top" || hint === "bottom") return isDoubleHeight(page) ? hint : wide ? "right" : null;
      return wide ? "right" : null;
  }
}

/** The slide and notes regions of one page; a page without the split shape is shown whole, without notes. */
export function regionsFor(page: Size, side: NotesSide | null): { slide: Region; notes: Region | null } {
  if (side === "right" && isDoubleWidth(page)) return { slide: "leftHalf", notes: "rightHalf" };
  if (side === "left" && isDoubleWidth(page)) return { slide: "rightHalf", notes: "leftHalf" };
  if (side === "bottom" && isDoubleHeight(page)) return { slide: "topHalf", notes: "bottomHalf" };
  if (side === "top" && isDoubleHeight(page)) return { slide: "bottomHalf", notes: "topHalf" };
  return { slide: "whole", notes: null };
}

/** The largest rectangle of `size`'s aspect that fits in `rect`, centred (zero for empty input). */
export function aspectFit(size: Size, rect: Rect): Rect {
  if (size.width <= 0 || size.height <= 0 || rect.width <= 0 || rect.height <= 0) return { x: 0, y: 0, width: 0, height: 0 };
  const f = Math.min(rect.width / size.width, rect.height / size.height);
  const width = Math.floor(size.width * f);
  const height = Math.floor(size.height * f);
  return { x: Math.round(rect.x + (rect.width - width) / 2), y: Math.round(rect.y + (rect.height - height) / 2), width, height };
}

// ---- displays ---------------------------------------------------------------------------------------------------

export interface ScreenInfo { id: string; isBuiltIn: boolean }
export interface ScreenPlan { audience: string; presenter: string | null }

/**
 * One display: the slides on it. Several: the presenter view on the built-in display, else on `preferredPresenter`
 * (the project window's display), else the first; the slides on the first other display. `swapped` exchanges them.
 */
export function planScreens(screens: ScreenInfo[], preferredPresenter: string | null, swapped: boolean): ScreenPlan | null {
  const first = screens[0];
  if (!first) return null;
  if (screens.length < 2) return { audience: first.id, presenter: null };
  const presenter = screens.find((s) => s.isBuiltIn) ?? screens.find((s) => s.id === preferredPresenter) ?? first;
  const audience = screens.find((s) => s.id !== presenter.id);
  if (!audience) return { audience: presenter.id, presenter: null };
  return swapped ? { audience: presenter.id, presenter: audience.id } : { audience: audience.id, presenter: presenter.id };
}

/** Linux connector names of laptop panels (eDP-1, LVDS-1, DSI-1). Windows does not tell. */
export const isBuiltInDisplayName = (name: string | null | undefined): boolean => !!name && /^(eDP|LVDS|DSI)/i.test(name);

// ---- source hints -----------------------------------------------------------------------------------------------

export const PRESENTATION_CLASSES = new Set(["beamer", "ltx-talk"]);

export interface PresentationSourceInfo { documentClass: string | null; notesSide: NotesSide | null }

export const isPresentationSource = (info: PresentationSourceInfo): boolean =>
  !!info.documentClass && PRESENTATION_CLASSES.has(info.documentClass);

/** Scans the preamble (before `\begin{document}`, at most 4 000 lines) ignoring comments. */
export function scanPresentationSource(source: string): PresentationSourceInfo {
  let preamble = "";
  const lines = source.split(/\r\n|\r|\n/);
  for (let i = 0; i < lines.length && i < 4000; i++) {
    const line = withoutComment(lines[i]!);
    const at = line.indexOf("\\begin{document}");
    if (at >= 0) { preamble += line.slice(0, at); break; }
    preamble += line + "\n";
  }
  const info: PresentationSourceInfo = { documentClass: null, notesSide: null };
  const cls = /\\documentclass\s*(?:\[[^\]]*\]\s*)?\{\s*([^}\s]+)\s*\}/.exec(preamble);
  if (cls?.[1]) info.documentClass = cls[1];
  const re = /\\setbeameroption\s*\{\s*show\s+notes\s+on\s+second\s+screen\s*(?:=\s*(left|right|top|bottom)\s*)?\}/gi;
  let m: RegExpExecArray | null;
  let last: RegExpExecArray | null = null;
  while ((m = re.exec(preamble))) last = m;
  if (last) info.notesSide = ((last[1]?.toLowerCase() as NotesSide | undefined) ?? "right");
  return info;
}

function withoutComment(line: string): string {
  let backslashes = 0;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (c === "%" && backslashes % 2 === 0) return line.slice(0, i);
    backslashes = c === "\\" ? backslashes + 1 : 0;
  }
  return line;
}

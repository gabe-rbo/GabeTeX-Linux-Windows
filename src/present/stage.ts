// What a presentation window shows (port of PresentationWindow.swift, PresenterView.swift, PresenterControlBar.swift):
// the audience view (the slide on black, black / white screen, click advances, the pointer hides) or the presenter
// view (elapsed timer, slide counter, clock, current and next slide, Beamer notes, control bar, go-to-slide entry).
// Pure rendering of a `PresentationState`; every user action is handed to `act`.
import "./present.css";
import { t } from "../i18n";
import { h } from "../ui/dom";
import type { SlideDocument, SlideRegion } from "../preview/slides";
import {
  NOTES_MODES, aspectFit, formatElapsed, notesSideFor, regionSize, regionsFor, timerElapsed,
  type Blank, type NotesMode, type NotesSide, type PresentationCommand, type Size, type TimerState,
} from "./logic";

export interface PresentationState {
  pdf: string;
  /** Bumped by every reload of the PDF. */
  version: number;
  index: number;
  pageCount: number;
  blank: Blank;
  typed: string;
  timer: TimerState;
  notesMode: NotesMode;
  notesHint: NotesSide | null;
  /** Two displays (audience + presenter window) or one (P toggles the presenter view). */
  layout: "dual" | "single";
  showsPresenter: boolean;
  swapped: boolean;
  audienceName: string;
  title: string;
}

export type PresentationAction =
  | { type: "cmd"; cmd: PresentationCommand }
  | { type: "timer"; op: "toggle" | "reset" }
  | { type: "notes"; mode: NotesMode }
  | { type: "swap" };

export type StageMode = "audience" | "presenter";

const NOTES_TITLES: Record<NotesMode, string> = { automatic: "Automatic", right: "Right Half", left: "Left Half", off: "No Notes" };
const CAPTION_H = 26;
const COLUMN_GAP = 40;
const PANEL_GAP = 22;
const CURSOR_HIDE_MS = 2000;

const S = (d: string) => `<path d="${d}" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>`;
const F = (d: string) => `<path d="${d}" fill="currentColor"/>`;
const ICONS: Record<string, string> = {
  left: S("M14.5 6.5 9 12l5.5 5.5"),
  right: S("M9.5 6.5 15 12l-5.5 5.5"),
  pause: F("M7.5 5h2.6v14H7.5zM13.9 5h2.6v14h-2.6z"),
  play: F("M8 5.2v13.6c0 .8.9 1.3 1.6.9l10.6-6.8c.6-.4.6-1.3 0-1.7L9.6 4.3C8.9 3.9 8 4.4 8 5.2Z"),
  reset: S("M5 12a7 7 0 1 0 2.1-5M5 4.5V8h3.5"),
  eyeSlash: S("M3 12s3.3-6 9-6 9 6 9 6-3.3 6-9 6-9-6-9-6Z") + S("M4 4l16 16"),
  sun: `<circle cx="12" cy="12" r="4" fill="none" stroke="currentColor" stroke-width="1.8"/>` + S("M12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4"),
  note: S("M6 3.5h12a1.5 1.5 0 0 1 1.5 1.5v14a1.5 1.5 0 0 1-1.5 1.5H6A1.5 1.5 0 0 1 4.5 19V5A1.5 1.5 0 0 1 6 3.5ZM8 8h8M8 12h8M8 16h5"),
  chevronDown: S("M6.5 9.5 12 15l5.5-5.5"),
  swap: S("M4 8h15M15 4l4 4-4 4M20 16H5M9 12l-4 4 4 4"),
  slides: S("M4 5.5h16a1.5 1.5 0 0 1 1.5 1.5v10a1.5 1.5 0 0 1-1.5 1.5H4A1.5 1.5 0 0 1 2.5 17V7A1.5 1.5 0 0 1 4 5.5Z") + F("M6 8.5h12v7H6z"),
  xmark: S("M6.5 6.5l11 11M17.5 6.5l-11 11"),
  flag: S("M5 21V4M5 4h11l-2 4 2 4H5"),
  check: S("M5 12.5l4.5 4.5L19 7.5"),
};
const svg = (name: string, size = 16): SVGSVGElement => {
  const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  el.setAttribute("viewBox", "0 0 24 24");
  el.setAttribute("width", String(size));
  el.setAttribute("height", String(size));
  el.setAttribute("aria-hidden", "true");
  el.innerHTML = ICONS[name] ?? "";
  return el;
};

/** One place a page region is drawn; late renders are dropped, a moved slide can dissolve in. */
class SlideSlot {
  readonly el = h("div", { class: "ps-slot" });
  private token = 0;
  private key = "";

  async show(doc: SlideDocument | null, index: number | null, region: SlideRegion, w: number, hgt: number, fade: boolean): Promise<void> {
    this.el.style.width = `${w}px`;
    this.el.style.height = `${hgt}px`;
    const key = doc && index !== null ? `${index}|${region}|${w}|${hgt}` : "";
    if (key === this.key) return;
    const token = ++this.token;
    if (!doc || index === null || w < 2 || hgt < 2) { this.key = ""; this.el.replaceChildren(); return; }
    let src: HTMLCanvasElement;
    try {
      src = await doc.render(index, region, w, hgt);
    } catch {
      return;
    }
    if (token !== this.token) return;
    this.key = key;
    const canvas = document.createElement("canvas");
    canvas.width = src.width;
    canvas.height = src.height;
    canvas.style.width = `${w}px`;
    canvas.style.height = `${hgt}px`;
    canvas.getContext("2d")?.drawImage(src, 0, 0);
    const old = [...this.el.children];
    const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    if (fade && old.length && !reduce) {
      canvas.className = "ps-fade-in";
      this.el.append(canvas);
      window.setTimeout(() => { for (const o of old) o.remove(); canvas.className = ""; }, 260);
    } else {
      this.el.replaceChildren(canvas);
    }
  }

  reset(): void { this.key = ""; }
}

export class Stage {
  readonly el: HTMLElement;
  private mode: StageMode = "audience";
  private state: PresentationState | null = null;
  private doc: SlideDocument | null = null;
  private lastIndex = -1;
  private tick = 0;
  private cursorTimer = 0;
  private readonly ro: ResizeObserver;

  // audience
  private readonly audience = h("div", { class: "ps-audience" });
  private readonly audienceSlot = new SlideSlot();
  private readonly blankEl = h("div", { class: "ps-blank" });

  // presenter
  private readonly presenter = h("div", { class: "ps-presenter", attrs: { role: "region", "aria-label": t("Presenter View") } });
  private readonly timerCaption = h("div", { class: "ps-caption" });
  private readonly timerValue = h("div", { class: "ps-timer" });
  private readonly counterCurrent = h("span", { class: "ps-count" });
  private readonly counterTotal = h("span", { class: "ps-total" });
  private readonly progress = h("div", { class: "ps-progress-fill" });
  private readonly displayStatus = h("div", { class: "ps-display" });
  private readonly clockValue = h("div", { class: "ps-clock" });
  private readonly slidesArea = h("div", { class: "ps-slides" });
  private readonly currentCol = h("div", { class: "ps-col" });
  private readonly sideCol = h("div", { class: "ps-col" });
  private readonly currentCaption = h("div", { class: "ps-caption-row" });
  private readonly currentSlot = new SlideSlot();
  private readonly currentCover = h("div", { class: "ps-cover" });
  private readonly nextSlot = new SlideSlot();
  private readonly nextEnd = h("div", { class: "ps-end" }, svg("flag", 22), h("div", { text: t("End of the presentation") }));
  private readonly notesSlot = new SlideSlot();
  private readonly noNotes = h("div", { class: "ps-no-notes" });
  private readonly bar = h("div", { class: "ps-bar" });
  private readonly goto = h("div", { class: "ps-goto", attrs: { "aria-live": "polite" } });
  private readonly gotoSlot = new SlideSlot();
  private menu: HTMLElement | null = null;

  constructor(private readonly act: (a: PresentationAction) => void) {
    this.audience.append(this.audienceSlot.el, this.blankEl);
    this.audience.addEventListener("click", () => this.act({ type: "cmd", cmd: { kind: "next" } }));
    this.audience.addEventListener("pointermove", () => this.pointerMoved());

    const header = h("div", { class: "ps-header" },
      h("div", { class: "ps-readout" }, this.timerCaption, this.timerValue),
      h("div", { class: "ps-counter" },
        h("div", { class: "ps-caption", text: t("Slide") }),
        h("div", { class: "ps-count-line" }, this.counterCurrent, this.counterTotal),
        h("div", { class: "ps-progress" }, this.progress),
        this.displayStatus),
      h("div", { class: "ps-readout end" }, h("div", { class: "ps-caption", text: t("Time") }), this.clockValue));

    const currentFrame = h("div", { class: "ps-frame current" }, this.currentSlot.el, this.currentCover);
    currentFrame.addEventListener("click", () => this.act({ type: "cmd", cmd: { kind: "next" } }));
    this.currentCol.append(this.currentCaption, currentFrame);
    const nextFrame = h("div", { class: "ps-frame next" }, this.nextSlot.el, this.nextEnd);
    nextFrame.addEventListener("click", () => this.act({ type: "cmd", cmd: { kind: "next" } }));
    this.sideCol.append(
      h("div", { class: "ps-caption ps-cap", text: t("Next Slide") }), nextFrame,
      h("div", { class: "ps-gap" }),
      h("div", { class: "ps-caption ps-cap", text: t("Notes") }), h("div", { class: "ps-frame notes" }, this.notesSlot.el), this.noNotes);
    this.slidesArea.append(this.currentCol, this.sideCol);
    this.presenter.append(header, this.slidesArea, this.bar, this.goto);

    this.el = h("div", { class: "ps-root" }, this.audience, this.presenter);
    this.ro = new ResizeObserver(() => this.layout(false));
    this.ro.observe(this.el);
    this.tick = window.setInterval(() => this.renderClock(), 500);
  }

  setMode(mode: StageMode): void {
    if (mode === this.mode && this.el.dataset.mode) return;
    this.mode = mode;
    this.el.dataset.mode = mode;
    this.closeMenu();
    this.audienceSlot.reset();
    this.currentSlot.reset();
    if (mode === "audience") this.pointerMoved();
    else { clearTimeout(this.cursorTimer); this.el.classList.remove("ps-hide-cursor"); }
    this.layout(false);
  }

  setDocument(doc: SlideDocument | null): void {
    this.doc = doc;
    for (const s of [this.audienceSlot, this.currentSlot, this.nextSlot, this.notesSlot, this.gotoSlot]) s.reset();
    this.layout(false);
  }

  update(state: PresentationState): void {
    const moved = !!this.state && state.index !== this.lastIndex;
    this.state = state;
    this.layout(moved);
    this.lastIndex = state.index;
  }

  destroy(): void {
    clearInterval(this.tick);
    clearTimeout(this.cursorTimer);
    this.ro.disconnect();
    this.el.remove();
  }

  // ---- geometry ---------------------------------------------------------------------------------------------------

  private notesSide(): NotesSide | null {
    const s = this.state;
    const first = this.doc?.sizes[0];
    if (!s || !first) return null;
    return notesSideFor(s.notesMode, first, s.notesHint);
  }

  private part(index: number, which: "slide" | "notes"): { region: SlideRegion; size: Size } | null {
    const page = this.doc?.sizes[index];
    if (!page) return null;
    const r = regionsFor(page, this.notesSide());
    const region = which === "slide" ? r.slide : r.notes;
    if (!region) return null;
    return { region, size: regionSize(region, page) };
  }

  private fit(size: Size | undefined, w: number, hgt: number): { width: number; height: number } {
    const f = aspectFit(size ?? { width: 16, height: 9 }, { x: 0, y: 0, width: Math.max(0, w), height: Math.max(0, hgt) });
    return { width: f.width, height: f.height };
  }

  // ---- rendering --------------------------------------------------------------------------------------------------

  private layout(moved: boolean): void {
    const s = this.state;
    if (!s) return;
    if (this.mode === "audience") this.renderAudience(s, moved);
    else this.renderPresenter(s);
  }

  private renderAudience(s: PresentationState, moved: boolean): void {
    const w = this.el.clientWidth;
    const hgt = this.el.clientHeight;
    const part = this.part(s.index, "slide");
    const fitted = this.fit(part?.size, w, hgt);
    void this.audienceSlot.show(this.doc, part ? s.index : null, part?.region ?? "whole", fitted.width, fitted.height, moved);
    this.blankEl.className = `ps-blank ${s.blank}`;
    // Pre-render the next slide so advancing is instant.
    const next = s.index + 1 < s.pageCount ? this.part(s.index + 1, "slide") : null;
    if (next && this.doc) {
      const nf = this.fit(next.size, w, hgt);
      void this.doc.render(s.index + 1, next.region, nf.width, nf.height).catch(() => {});
    }
  }

  private renderPresenter(s: PresentationState): void {
    // header
    const current = s.pageCount > 0 ? s.index + 1 : 0;
    this.counterCurrent.textContent = String(current);
    this.counterTotal.textContent = `/ ${s.pageCount}`;
    this.counterCurrent.parentElement?.setAttribute("aria-label", t("Slide %lld of %lld", current, s.pageCount));
    this.progress.style.width = `${s.pageCount > 0 ? (100 * current) / s.pageCount : 0}%`;
    this.displayStatus.textContent = s.layout === "dual"
      ? (s.audienceName ? t("Slides on %@", s.audienceName) : t("Slides on the other display"))
      : t("One display · P shows the slides");
    this.renderClock();

    // slides
    const W = this.slidesArea.clientWidth;
    const H = this.slidesArea.clientHeight;
    const currentWidth = Math.max(0, Math.floor((W - COLUMN_GAP) * 0.62));
    const sideWidth = Math.max(0, W - COLUMN_GAP - currentWidth);
    this.currentCol.style.width = `${currentWidth}px`;
    this.sideCol.style.width = `${sideWidth}px`;

    const cur = this.part(s.index, "slide");
    const cf = this.fit(cur?.size, currentWidth, H - CAPTION_H);
    void this.currentSlot.show(this.doc, cur ? s.index : null, cur?.region ?? "whole", cf.width, cf.height, false);
    this.currentCaption.replaceChildren(h("div", { class: "ps-caption", text: t("Current Slide") }));
    this.currentCaption.style.width = `${cf.width}px`;
    if (s.blank !== "none") {
      this.currentCaption.append(h("span", { class: "ps-badge" }, svg(s.blank === "white" ? "sun" : "eyeSlash", 12), h("span", { text: s.blank === "white" ? t("White Screen") : t("Black Screen") })));
    }
    this.currentCover.hidden = s.blank === "none";
    this.currentCover.replaceChildren(s.blank === "none" ? "" : h("div", { class: "ps-cover-card" },
      svg(s.blank === "white" ? "sun" : "eyeSlash", 34),
      h("div", { class: "ps-cover-title", text: s.blank === "white" ? t("The audience sees a white screen") : t("The audience sees a black screen") }),
      h("div", { class: "ps-cover-hint", text: s.blank === "white" ? t("Press W or any arrow key to show the slides") : t("Press B or any arrow key to show the slides") })));

    const nextIndex = s.index + 1 < s.pageCount ? s.index + 1 : null;
    const notes = this.part(s.index, "notes");
    const hasNotes = !!notes;
    const usable = Math.max(0, H - 2 * CAPTION_H - PANEL_GAP);
    const boxH = hasNotes ? usable / 2 : usable * 0.55;
    const nextPart = this.part(nextIndex ?? s.index, "slide");
    const nf = this.fit(nextPart?.size, sideWidth, boxH);
    void this.nextSlot.show(this.doc, nextIndex !== null && nextPart ? nextIndex : null, nextPart?.region ?? "whole", nf.width, nf.height, false);
    const nextFrame = this.nextSlot.el.parentElement!;
    nextFrame.style.width = `${nf.width}px`;
    nextFrame.style.height = `${nf.height}px`;
    this.nextEnd.hidden = nextIndex !== null;
    nextFrame.classList.toggle("ended", nextIndex === null);

    const notesFrame = this.notesSlot.el.parentElement!;
    notesFrame.hidden = !hasNotes;
    this.noNotes.hidden = hasNotes;
    if (hasNotes) {
      const nn = this.fit(notes.size, sideWidth, boxH);
      void this.notesSlot.show(this.doc, s.index, notes.region, nn.width, nn.height, false);
    } else {
      this.noNotes.replaceChildren(h("div", { class: "ps-no-notes-title", text: t("No notes for these slides") }),
        s.notesMode !== "off" ? h("div", { class: "ps-no-notes-hint", text: t("To see Beamer notes here, add \\setbeameroption{show notes on second screen} to the preamble and write \\note{…} in your frames.") }) : "");
    }
    // Pre-render the slide after next for the "next" panel.
    if (nextIndex !== null && nextIndex + 1 < s.pageCount && this.doc) {
      const after = this.part(nextIndex + 1, "slide");
      if (after) { const af = this.fit(after.size, sideWidth, boxH); void this.doc.render(nextIndex + 1, after.region, af.width, af.height).catch(() => {}); }
      if (nextPart) void this.doc.render(nextIndex, nextPart.region, cf.width, cf.height).catch(() => {});
    }

    this.renderBar(s);
    this.renderGoto(s);
  }

  private renderClock(): void {
    const s = this.state;
    if (!s || this.mode !== "presenter") return;
    const running = s.timer.runningSince !== null;
    this.timerCaption.textContent = running ? t("Elapsed") : t("Paused");
    const elapsed = formatElapsed(timerElapsed(s.timer, Date.now()));
    this.timerValue.textContent = elapsed;
    this.timerValue.classList.toggle("paused", !running);
    this.timerValue.setAttribute("aria-label", `${running ? t("Elapsed time") : t("Timer paused")} ${elapsed}`);
    this.clockValue.textContent = new Date().toLocaleTimeString(document.documentElement.lang || undefined, { hour: "2-digit", minute: "2-digit" });
  }

  private renderBar(s: PresentationState): void {
    const button = (name: string, label: string, help: string, run: () => void, opts: { on?: boolean; disabled?: boolean } = {}) => {
      const b = h("button", { class: "ps-btn" + (opts.on ? " on" : ""), title: help, attrs: { "aria-label": label, tabindex: -1, "aria-pressed": opts.on === undefined ? undefined : String(opts.on) } }, svg(name, 16));
      b.disabled = !!opts.disabled;
      b.addEventListener("click", (e) => { e.stopPropagation(); run(); });
      return b;
    };
    const cmd = (c: PresentationCommand) => () => this.act({ type: "cmd", cmd: c });
    const running = s.timer.runningSince !== null;
    const cluster = (...items: HTMLElement[]) => h("div", { class: "ps-cluster" }, ...items);
    const notesBtn = h("button", { class: "ps-btn wide", title: t("Where the notes are on the pages"), attrs: { "aria-label": t("Notes On"), "aria-haspopup": "menu", tabindex: -1 } },
      svg("note", 15), h("span", { text: t("Notes") }), svg("chevronDown", 10));
    notesBtn.addEventListener("click", (e) => { e.stopPropagation(); this.openNotesMenu(notesBtn, s.notesMode); });
    const end = h("button", { class: "ps-end-btn", title: t("End Presentation (Esc)"), attrs: { tabindex: -1 } }, svg("xmark", 14), h("span", { text: t("End Presentation") }));
    end.addEventListener("click", (e) => { e.stopPropagation(); this.act({ type: "cmd", cmd: { kind: "end" } }); });
    this.bar.replaceChildren(
      cluster(
        button("left", t("Previous Slide"), t("Previous Slide (←)"), cmd({ kind: "previous" }), { disabled: s.index <= 0 && s.blank === "none" }),
        button("right", t("Next Slide"), t("Next Slide (→)"), cmd({ kind: "next" }), { disabled: s.index + 1 >= s.pageCount && s.blank === "none" })),
      cluster(
        running ? button("pause", t("Pause Timer"), t("Pause Timer"), () => this.act({ type: "timer", op: "toggle" }))
          : button("play", t("Resume Timer"), t("Resume Timer"), () => this.act({ type: "timer", op: "toggle" })),
        button("reset", t("Reset Timer"), t("Reset Timer"), () => this.act({ type: "timer", op: "reset" }))),
      cluster(
        button("eyeSlash", t("Black Screen"), t("Black Screen (B)"), cmd({ kind: "toggleBlack" }), { on: s.blank === "black" }),
        button("sun", t("White Screen"), t("White Screen (W)"), cmd({ kind: "toggleWhite" }), { on: s.blank === "white" })),
      cluster(notesBtn,
        s.layout === "dual" ? button("swap", t("Swap Displays"), t("Swap Displays"), () => this.act({ type: "swap" }))
          : button("slides", t("Show Slides"), t("Show Slides (P)"), cmd({ kind: "togglePresenterView" }))),
      end);
  }

  private openNotesMenu(anchor: HTMLElement, current: NotesMode): void {
    if (this.menu) { this.closeMenu(); return; }
    const menu = h("div", { class: "ps-menu", attrs: { role: "menu", "aria-label": t("Notes On") } },
      h("div", { class: "ps-menu-header", text: t("Notes On") }),
      ...NOTES_MODES.map((mode) => {
        const item = h("button", { class: "ps-menu-item", attrs: { role: "menuitemradio", "aria-checked": String(mode === current), tabindex: -1 } },
          h("span", { class: "ps-check" }, mode === current ? svg("check", 13) : null), h("span", { text: t(NOTES_TITLES[mode]) }));
        item.addEventListener("click", (e) => { e.stopPropagation(); this.closeMenu(); this.act({ type: "notes", mode }); });
        return item;
      }));
    const r = anchor.getBoundingClientRect();
    menu.style.left = `${Math.round(r.left)}px`;
    menu.style.bottom = `${Math.round(window.innerHeight - r.top + 8)}px`;
    this.el.append(menu);
    this.menu = menu;
    const close = (e: Event) => { if (!menu.contains(e.target as Node)) { this.closeMenu(); document.removeEventListener("pointerdown", close, true); } };
    document.addEventListener("pointerdown", close, true);
  }

  private closeMenu(): void {
    this.menu?.remove();
    this.menu = null;
  }

  private renderGoto(s: PresentationState): void {
    this.goto.hidden = !s.typed;
    if (!s.typed) return;
    const target = s.pageCount > 0 ? Math.min(Math.max(Number.parseInt(s.typed, 10) - 1, 0), s.pageCount - 1) : null;
    const part = target !== null ? this.part(target, "slide") : null;
    const f = this.fit(part?.size, 320, 200);
    void this.gotoSlot.show(this.doc, part ? target : null, part?.region ?? "whole", f.width, f.height, false);
    this.goto.replaceChildren(h("div", { class: "ps-goto-card" },
      h("div", { class: "ps-caption", text: t("Go to Slide") }),
      h("div", { class: "ps-goto-line" }, h("span", { class: "ps-goto-digits", text: s.typed }), h("span", { class: "ps-goto-total", text: `/ ${s.pageCount}` })),
      part ? this.gotoSlot.el : null,
      h("div", { class: "ps-goto-hint", text: t("Return to go · Esc to cancel") })));
  }

  // ---- pointer ----------------------------------------------------------------------------------------------------

  private pointerMoved(): void {
    this.el.classList.remove("ps-hide-cursor");
    clearTimeout(this.cursorTimer);
    if (this.mode !== "audience") return;
    this.cursorTimer = window.setTimeout(() => this.el.classList.add("ps-hide-cursor"), CURSOR_HIDE_MS);
  }
}

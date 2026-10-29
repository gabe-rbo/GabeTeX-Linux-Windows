// Editor tabs as glass pills (port of TabStrip.swift): click to switch, drag to reorder, middle-click or × to close,
// unsaved dot that turns into × on hover, main-document star.
import { t } from "../i18n";
import type { Workspace } from "../app/workspace";
import { h, basename, toggleClass } from "./dom";
import { icon, fileIconName } from "./icons";
import { popupMenu } from "./overlay";

export function createTabStrip(ws: Workspace, opts: { onClose(path: string): void; onCloseOthers(path: string): void }): { el: HTMLElement; destroy(): void } {
  const strip = h("div", { class: "tab-strip", attrs: { role: "tablist", "aria-label": t("Open files") } });
  const el = h("div", { class: "tab-strip-wrap" }, strip);
  const pills = new Map<string, HTMLElement>();
  let dragging: string | null = null;

  function pill(path: string): HTMLElement {
    const name = basename(path);
    const close = h("button", { class: "tab-close", title: t("Close tab (⌘W)"), attrs: { "aria-label": t("Close tab"), tabindex: -1 } }, icon("xmark", 10));
    const p = h("div", { class: "tab-pill", draggable: true, title: ws.relative(path), attrs: { role: "tab" } },
      icon(fileIconName(path), 13, "tab-icon"),
      h("span", { class: "tab-name", text: name }),
      h("span", { class: "tab-star", attrs: { "aria-label": t("Main document") } }, icon("starFill", 10)),
      h("span", { class: "tab-trail" }, h("span", { class: "tab-dot" }), close));
    p.addEventListener("mousedown", (e) => { if (e.button === 1) e.preventDefault(); });
    p.addEventListener("click", (e) => { if (!close.contains(e.target as Node)) ws.select(path); });
    p.addEventListener("auxclick", (e) => { if (e.button === 1) opts.onClose(path); });
    close.addEventListener("click", (e) => { e.stopPropagation(); opts.onClose(path); });
    p.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      popupMenu([
        { title: t("Close Tab"), action: () => opts.onClose(path) },
        { title: t("Close Other Tabs"), disabled: ws.tabs.length < 2, action: () => opts.onCloseOthers(path) },
        { kind: "separator" },
        { title: t("Set as Main Document"), disabled: !/\.(tex|ltx)$/i.test(path) || ws.isMainFile(path), action: () => ws.setMainFile(path) },
        { title: t("Reveal in File Manager"), action: () => ws.reveal(path) },
      ], { x: e.clientX, y: e.clientY });
    });
    p.addEventListener("dragstart", (e) => { dragging = path; e.dataTransfer?.setData("text/x-gabetex-tab", path); p.classList.add("dragging"); });
    p.addEventListener("dragend", () => { dragging = null; p.classList.remove("dragging"); });
    p.addEventListener("dragover", (e) => { if (dragging && dragging !== path) { e.preventDefault(); p.classList.add("drop-before"); } });
    p.addEventListener("dragleave", () => p.classList.remove("drop-before"));
    p.addEventListener("drop", (e) => {
      p.classList.remove("drop-before");
      if (dragging && dragging !== path) { e.preventDefault(); ws.moveTab(dragging, path); }
    });
    return p;
  }

  function render(): void {
    el.hidden = ws.tabs.length === 0;
    // Keep existing pills (and their transitions); add new ones; drop closed ones; fix order.
    for (const [path, p] of pills) if (!ws.tabs.includes(path)) { p.remove(); pills.delete(path); }
    let prev: Element | null = null;
    for (const path of ws.tabs) {
      let p = pills.get(path);
      if (!p) { p = pill(path); pills.set(path, p); p.classList.add("entering"); requestAnimationFrame(() => p!.classList.remove("entering")); }
      const expected: Element | null = prev ? prev.nextElementSibling : strip.firstElementChild;
      if (expected !== p) strip.insertBefore(p, expected);
      prev = p;
    }
    marks();
    const cur = ws.current ? pills.get(ws.current) : undefined;
    cur?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }

  function marks(): void {
    for (const [path, p] of pills) {
      const selected = path === ws.current;
      toggleClass(p, "selected", selected);
      p.setAttribute("aria-selected", String(selected));
      toggleClass(p, "dirty", ws.isDirty(path));
      toggleClass(p, "main", ws.isMainFile(path));
      p.setAttribute("aria-label", ws.isDirty(path) ? t("%@, unsaved changes", basename(path)) : basename(path));
    }
  }

  strip.addEventListener("wheel", (e) => {
    if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) { strip.scrollLeft += e.deltaY; e.preventDefault(); }
  }, { passive: false });

  const offs = [ws.events.on("tabs", render), ws.events.on(["dirty", "main"], marks)];
  render();
  return { el, destroy: () => offs.forEach((f) => f()) };
}

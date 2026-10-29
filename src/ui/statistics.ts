// Document Statistics popover (simplified port of StatisticsPopover.swift): total words (texcount rules, from the
// project index) and pages of the last PDF.
import { t, formatInt } from "../i18n";
import type { Workspace } from "../app/workspace";
import { h } from "./dom";

let open: HTMLElement | null = null;

export function showStatistics(ws: Workspace, anchor?: HTMLElement): void {
  if (open) { open.remove(); open = null; return; }
  const words = ws.index?.stats.words;
  const row = (label: string, value: string) => h("div", { class: "stat-row" }, h("span", { text: label }), h("span", { class: "stat-value", text: value }));
  const pop = h("div", { class: "popover glass-panel", attrs: { role: "dialog", "aria-label": t("Document Statistics") } },
    h("div", { class: "popover-title", text: t("Document Statistics") }),
    words === undefined ? h("div", { class: "dim", text: t("Counting…") }) : null,
    row(t("Words"), words === undefined ? "–" : formatInt(words)),
    row(t("Pages"), ws.pageCount > 0 ? formatInt(ws.pageCount) : "–"),
    row(t("Files"), formatInt(ws.flatFiles.length)),
    h("div", { class: "popover-footnote", text: t("Words are counted the way texcount does: running text, headings, captions and footnotes. Commands, formulas, table contents, other text inside figures, comments, verbatim code and the preamble are left out, and a hyphenated word counts once. Each row includes its subsections.") }));
  document.body.appendChild(pop);
  const r = pop.getBoundingClientRect();
  const a = anchor?.getBoundingClientRect();
  const x = a ? Math.min(innerWidth - r.width - 10, Math.max(10, a.left + a.width / 2 - r.width / 2)) : (innerWidth - r.width) / 2;
  const y = a ? a.top - r.height - 8 : (innerHeight - r.height) / 2;
  pop.style.transform = `translate(${Math.round(x)}px, ${Math.round(Math.max(10, y))}px)`;
  requestAnimationFrame(() => pop.classList.add("open"));
  open = pop;
  const close = (e: Event) => {
    if (e instanceof KeyboardEvent && e.key !== "Escape") return;
    if (e instanceof MouseEvent && (pop.contains(e.target as Node) || anchor?.contains(e.target as Node))) return;
    pop.remove();
    if (open === pop) open = null;
    document.removeEventListener("mousedown", close, true);
    document.removeEventListener("keydown", close, true);
  };
  setTimeout(() => {
    document.addEventListener("mousedown", close, true);
    document.addEventListener("keydown", close, true);
  });
}

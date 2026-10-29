// Floating glass palettes (port of QuickOpenView / CommandPaletteView): a search field over a fuzzy-ranked list,
// arrow keys to move, Return to run, Escape to close. Quick Open lists project files; the Command Palette lists
// commands with their category, state and shortcut.
import { t } from "../i18n";
import { fuzzyScore, fuzzyPositions } from "../app/fuzzy";
import { commands, commandTitle, CATEGORY_TITLES, type CommandHost } from "../app/commands";
import { shortcuts } from "../app/shortcuts";
import type { Workspace } from "../app/workspace";
import { h, basename } from "./dom";
import { icon, fileIconName, symbolIcon } from "./icons";

interface Row {
  key: string;
  title: string;
  /** Text matched against the query. */
  haystack: string;
  /** Characters of `title` to highlight are found by matching the query against the title. */
  detail?: string;
  icon: string;
  trailing?: string;
  isOn?: boolean;
  enabled: boolean;
  run(): void;
  secondary?: { title: string; icon: string; run(): void };
}

let current: { close(): void; kind: string } | null = null;

export const isPaletteOpen = (): boolean => current !== null;

function openPalette(kind: string, placeholder: string, rows: () => Row[], emptyText: string, maxRows = 10): void {
  if (current) {
    const same = current.kind === kind;
    current.close();
    if (same) return;
  }
  const previous = document.activeElement as HTMLElement | null;
  const input = h("input", { class: "palette-input", attrs: { type: "text", placeholder, "aria-label": placeholder, spellcheck: "false", autocomplete: "off" } });
  const list = h("div", { class: "palette-list", attrs: { role: "listbox" } });
  const panel = h("div", { class: "palette glass-panel", attrs: { role: "dialog", "aria-label": placeholder } },
    h("div", { class: "palette-field" }, icon(kind === "commands" ? "command" : "magnifier", 18), input), list);
  const layer = h("div", { class: "palette-layer" }, panel);
  document.body.appendChild(layer);
  requestAnimationFrame(() => layer.classList.add("open"));

  const all = rows();
  let shown: Row[] = [];
  let index = 0;

  const render = () => {
    const q = input.value.trim();
    if (!q) shown = all.slice(0, 200);
    else {
      const scored: Array<{ r: Row; s: number }> = [];
      for (const r of all) {
        const s = fuzzyScore(q, r.haystack);
        if (s !== null) scored.push({ r, s: s + (r.enabled ? 0 : -1000) });
      }
      scored.sort((a, b) => b.s - a.s);
      shown = scored.slice(0, 200).map((x) => x.r);
    }
    index = Math.min(index, Math.max(0, shown.length - 1));
    list.textContent = "";
    if (!shown.length) {
      list.appendChild(h("div", { class: "palette-empty", text: emptyText }));
      return;
    }
    const frag = document.createDocumentFragment();
    shown.forEach((r, i) => {
      const titleEl = h("span", { class: "palette-title" });
      highlight(titleEl, r.title, q);
      const row = h("div", { class: "palette-row" + (i === index ? " selected" : "") + (r.enabled ? "" : " disabled"), attrs: { role: "option", "aria-selected": String(i === index), "aria-disabled": String(!r.enabled) } },
        icon(r.icon, 16, "palette-icon"),
        h("span", { class: "palette-texts" }, titleEl, r.detail ? h("span", { class: "palette-detail", text: r.detail }) : null),
        r.isOn ? h("span", { class: "palette-on", text: "✓", attrs: { "aria-label": t("On") } }) : null,
        r.trailing ? h("span", { class: "kbd", text: r.trailing }) : null,
        r.secondary ? h("button", { class: "icon-btn palette-secondary", title: r.secondary.title, attrs: { "aria-label": r.secondary.title } }, icon(r.secondary.icon, 13)) : null);
      row.addEventListener("mousemove", () => { if (index !== i) { index = i; mark(); } });
      row.addEventListener("click", (e) => {
        const sec = (e.target as HTMLElement).closest(".palette-secondary");
        if (sec && r.secondary) { close(); r.secondary.run(); return; }
        choose(i);
      });
      frag.appendChild(row);
    });
    list.appendChild(frag);
    list.style.maxHeight = `${maxRows * 40 + 12}px`;
  };
  const mark = () => {
    list.querySelectorAll(".palette-row").forEach((el, i) => {
      el.classList.toggle("selected", i === index);
      el.setAttribute("aria-selected", String(i === index));
    });
    list.children[index]?.scrollIntoView({ block: "nearest" });
  };
  const choose = (i: number) => {
    const r = shown[i];
    if (!r || !r.enabled) return;
    close();
    r.run();
  };
  const close = () => {
    if (current?.close !== close) return;
    current = null;
    layer.remove();
    document.removeEventListener("mousedown", outside, true);
    if (previous && document.contains(previous)) previous.focus();
  };
  const outside = (e: MouseEvent) => { if (!panel.contains(e.target as Node)) close(); };

  input.addEventListener("input", () => { index = 0; render(); });
  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) { e.preventDefault(); index = Math.min(shown.length - 1, index + 1); mark(); }
    else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p" && !e.shiftKey)) { e.preventDefault(); index = Math.max(0, index - 1); mark(); }
    else if (e.key === "Enter") { e.preventDefault(); choose(index); }
    else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); }
    else if (e.key === "Tab") e.preventDefault();
  });
  setTimeout(() => document.addEventListener("mousedown", outside, true));
  current = { close, kind };
  render();
  input.focus();
}

function highlight(el: HTMLElement, text: string, query: string): void {
  const pos = query ? new Set(fuzzyPositions(query, text)) : new Set<number>();
  if (!pos.size) { el.textContent = text; return; }
  let run = "";
  let inMatch = false;
  const flush = () => {
    if (!run) return;
    el.appendChild(inMatch ? h("mark", { text: run }) : document.createTextNode(run));
    run = "";
  };
  for (let i = 0; i < text.length; i++) {
    const m = pos.has(i);
    if (m !== inMatch) { flush(); inMatch = m; }
    run += text[i];
  }
  flush();
}

export function closePalette(): void {
  current?.close();
}

/** Quick Open (Ctrl+P): project files by fuzzy name; recently used tabs first when the query is empty. */
export function openQuickOpen(ws: Workspace, open: (path: string) => void): void {
  openPalette("files", t("Open file by name"), () => {
    const recent = [...ws.tabs].reverse();
    const files = [...new Set([...recent, ...ws.flatFiles])];
    return files.map((path) => {
      const rel = ws.relative(path);
      return { key: path, title: basename(path), detail: rel.includes("/") ? rel.slice(0, rel.lastIndexOf("/")) : undefined, haystack: rel, icon: fileIconName(path), enabled: true, run: () => open(path) } satisfies Row;
    });
  }, t("No matching files"), 9);
}

/** Command Palette (Ctrl+Shift+P): every command with its category, state and shortcut. */
export function openCommandPalette(host: CommandHost): void {
  openPalette("commands", t("Search commands"), () =>
    commands().filter((c) => c.showsInPalette).map((c) => {
      const title = commandTitle(c, host);
      const category = t(CATEGORY_TITLES[c.category]);
      return {
        key: c.id, title, detail: category, haystack: `${title} ${c.title} ${category} ${c.keywords.join(" ")} ${c.id}`,
        icon: symbolIcon(c.symbol), trailing: shortcuts.combo(c.id), isOn: c.isOn?.(host), enabled: c.isEnabled(host), run: () => c.run(host),
      } satisfies Row;
    }), t("No matching commands"), 10);
}

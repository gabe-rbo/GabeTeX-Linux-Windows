// Glass banners above the editor (port of Banners.swift): no TeX found, missing packages, file changed on disk.
import { t, formatList } from "../i18n";
import type { Workspace } from "../app/workspace";
import { h, basename } from "./dom";
import { icon } from "./icons";

interface BannerSpec {
  key: string;
  symbol: string;
  tint: string;
  message: string;
  detail?: string;
  progress?: boolean;
  actions: Array<{ title: string; prominent?: boolean; run: () => void }>;
}

export function createBanners(ws: Workspace, opts: { openSettings(): void }): { el: HTMLElement; destroy(): void } {
  const el = h("div", { class: "banners", attrs: { role: "region", "aria-live": "polite" } });
  const shown = new Map<string, HTMLElement>();

  function specs(): BannerSpec[] {
    const out: BannerSpec[] = [];
    if (ws.tex === null) {
      out.push({
        key: "notex", symbol: "warningFill", tint: "orange", message: t("No TeX distribution was found"),
        detail: t("Install TeX Live or MiKTeX to compile documents."),
        actions: [
          { title: t("Open Settings"), run: () => opts.openSettings() },
          { title: t("Get TeX Live"), prominent: true, run: () => void openUrl("https://tug.org/texlive/") },
        ],
      });
    }
    const p = ws.packages;
    if (p.kind === "offer") {
      const first = p.files[0] ?? "";
      out.push({
        key: "pkg", symbol: "shippingbox", tint: "blue",
        message: p.files.length === 1 ? t("“%@” is not installed", first) : t("“%@” and %lld more files are not installed", first, p.files.length - 1),
        actions: [
          { title: t("Dismiss"), run: () => ws.dismissPackages() },
          { title: t("Install"), prominent: true, run: () => void ws.installPackages() },
        ],
      });
    } else if (p.kind === "installing") {
      out.push({ key: "pkg", symbol: "shippingbox", tint: "blue", progress: true, message: t("Installing %@…", formatList(p.files)), detail: t("This can take a minute."), actions: [] });
    }
    for (const [path, change] of [...ws.externalChanges].sort((a, b) => a[0].localeCompare(b[0]))) {
      const name = basename(path);
      if (change === "modified") {
        out.push({
          key: "ext:" + path, symbol: "sync", tint: "orange", message: t("“%@” changed on disk", name), detail: t("You also have unsaved changes in the editor."),
          actions: [{ title: t("Keep Mine"), run: () => ws.keepMine(path) }, { title: t("Reload"), prominent: true, run: () => void ws.reloadFromDisk(path) }],
        });
      } else {
        out.push({
          key: "del:" + path, symbol: "trash", tint: "red", message: t("“%@” was deleted from disk", name), detail: t("Saving will create the file again."),
          actions: [{ title: t("Close Tab"), run: () => ws.closeTabNow(path) }, { title: t("Keep Mine"), prominent: true, run: () => ws.keepMine(path) }],
        });
      }
    }
    return out;
  }

  function build(s: BannerSpec): HTMLElement {
    return h("div", { class: "banner glass-panel", data: { key: s.key } },
      s.progress ? h("span", { class: "spinner small" }) : h("span", { class: `banner-icon tint-${s.tint}` }, icon(s.symbol, 18)),
      h("div", { class: "banner-text" }, h("div", { class: "banner-message", text: s.message }), s.detail ? h("div", { class: "banner-detail", text: s.detail }) : null),
      h("div", { class: "banner-actions" }, s.actions.map((a) => h("button", { class: "btn small " + (a.prominent ? "prominent" : "glass"), text: a.title, on: { click: a.run } }))));
  }

  function render(): void {
    const list = specs();
    const keys = new Set(list.map((s) => s.key + "|" + s.message));
    for (const [k, b] of shown) {
      if (keys.has(k)) continue;
      shown.delete(k);
      b.classList.add("leaving");
      setTimeout(() => b.remove(), 220);
    }
    let prev: Element | null = null;
    for (const s of list) {
      const k = s.key + "|" + s.message;
      let b = shown.get(k);
      if (!b) {
        b = build(s);
        shown.set(k, b);
        b.classList.add("entering");
        const node = b;
        requestAnimationFrame(() => requestAnimationFrame(() => node.classList.remove("entering")));
      }
      const expected: Element | null = prev ? prev.nextElementSibling : el.firstElementChild;
      if (expected !== b) el.insertBefore(b, expected);
      prev = b;
    }
    el.classList.toggle("has-banners", list.length > 0);
  }

  const offs = [ws.events.on(["banners", "tex"], render)];
  render();
  return { el, destroy: () => offs.forEach((f) => f()) };
}

async function openUrl(url: string): Promise<void> {
  // No opener plugin is installed; the web view hands external links to the system browser where it can.
  window.open(url, "_blank", "noopener");
}

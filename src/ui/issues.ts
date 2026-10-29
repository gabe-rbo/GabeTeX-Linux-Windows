// Bottom panel (port of IssuesPanel.swift): compile problems (filterable, click to jump) and the raw TeX log.
import * as ipc from "../ipc";
import type { Diagnostic } from "../ipc";
import { t, formatInt } from "../i18n";
import type { Workspace } from "../app/workspace";
import { h, toggleClass } from "./dom";
import { icon } from "./icons";

export function createIssuesPanel(ws: Workspace, opts: { hide(): void }): { el: HTMLElement; destroy(): void; refresh(): void } {
  let tab: "issues" | "log" = "issues";
  const filters = { error: true, warning: true, info: false };
  let selected: Diagnostic | null = null;

  const segIssues = h("button", { class: "seg", text: t("Issues") });
  const segLog = h("button", { class: "seg", text: t("Log") });
  const seg = h("div", { class: "segmented small" }, segIssues, segLog);
  const filterBtn = (kind: "error" | "warning" | "info", ic: string, help: string) => {
    const count = h("span", { class: "num" });
    const b = h("button", { class: "capsule glass-capsule interactive filter " + kind, title: t(help), attrs: { "aria-label": t(help) } }, icon(ic, 12), count);
    b.addEventListener("click", () => { filters[kind] = !filters[kind]; render(); });
    return { b, count };
  };
  const fErr = filterBtn("error", "errorFill", "Show errors");
  const fWarn = filterBtn("warning", "warningFill", "Show warnings");
  const fOther = filterBtn("info", "badbox", "Show bad boxes and notes");
  const filterGroup = h("div", { class: "filters" }, fErr.b, fWarn.b, fOther.b);
  const closeBtn = h("button", { class: "icon-btn", title: t("Hide the Issues panel (⇧⌘M)"), attrs: { "aria-label": t("Hide Issues panel") } }, icon("xmark", 12));
  closeBtn.addEventListener("click", () => opts.hide());
  const list = h("div", { class: "issues-list", attrs: { role: "listbox", tabindex: 0 } });
  const log = h("pre", { class: "log-view", attrs: { tabindex: 0 } });
  const el = h("section", { class: "issues-panel", attrs: { "aria-label": t("Issues") } },
    h("div", { class: "issues-header" }, seg, filterGroup, h("div", { class: "spacer" }), closeBtn), list, log);

  segIssues.addEventListener("click", () => { tab = "issues"; render(); });
  segLog.addEventListener("click", () => { tab = "log"; render(); void loadLog(); });

  let logFor: string | undefined;
  async function loadLog(): Promise<void> {
    if (tab !== "log") return;
    const path = ws.logPath;
    if (path && path === logFor) return;
    logFor = path;
    if (!path) { log.textContent = ws.toolOutput; return; }
    try {
      log.textContent = (await ipc.readFile(path)).text;
    } catch {
      log.textContent = ws.toolOutput;
    }
  }

  function render(): void {
    toggleClass(segIssues, "selected", tab === "issues");
    toggleClass(segLog, "selected", tab === "log");
    seg.dataset.index = tab === "issues" ? "0" : "1";
    filterGroup.hidden = tab !== "issues";
    list.hidden = tab !== "issues";
    log.hidden = tab !== "log";
    fErr.count.textContent = formatInt(ws.errorCount);
    fWarn.count.textContent = formatInt(ws.warningCount);
    fOther.count.textContent = formatInt(ws.otherCount);
    toggleClass(fErr.b, "off", !filters.error);
    toggleClass(fWarn.b, "off", !filters.warning);
    toggleClass(fOther.b, "off", !filters.info);
    if (tab !== "issues") return;
    const items = ws.diagnostics.filter((d) => filters[d.severity]);
    list.textContent = "";
    if (!items.length) {
      const none = ws.diagnostics.length === 0;
      list.appendChild(h("div", { class: "empty-state small" }, icon(none ? "checkCircle" : "filter", 26),
        h("div", { class: "empty-title", text: none ? t("No Issues") : t("No Matching Issues") }),
        h("div", { class: "empty-detail", text: none ? t("Errors and warnings from TeX appear here.") : t("Adjust the filters above to see other messages.") })));
      return;
    }
    const frag = document.createDocumentFragment();
    for (const d of items) {
      const isSel = selected !== null && sameIssue(selected, d);
      const row = h("div", { class: "issue-row" + (isSel ? " selected" : ""), attrs: { role: "option", "aria-selected": String(isSel) } },
        h("span", { class: "issue-icon " + d.severity }, icon(d.severity === "error" ? "errorFill" : d.severity === "warning" ? "warningFill" : "info", 14)),
        h("div", { class: "issue-text" },
          h("div", { class: "issue-message", text: d.message }),
          isSel && d.detail ? h("pre", { class: "issue-detail", text: d.detail }) : null),
        d.path ? h("span", { class: "issue-location", text: `${ws.relative(d.path)}:${d.line}` }) : null);
      row.addEventListener("click", () => { selected = d; render(); ws.goToIssue(d); });
      frag.appendChild(row);
    }
    list.appendChild(frag);
    list.querySelector(".selected")?.scrollIntoView({ block: "nearest" });
  }

  const offs = [ws.events.on("diagnostics", render), ws.events.on("log", () => { logFor = undefined; void loadLog(); })];
  render();
  return { el, destroy: () => offs.forEach((f) => f()), refresh: render };
}

const sameIssue = (a: Diagnostic, b: Diagnostic) => a.path === b.path && a.line === b.line && a.message === b.message;

// The unified toolbar (port of ProjectToolbar.swift): project title and file subtitle on the left;
// Compile · Stop │ Engine │ Live · Preview · PDF only · Issues as glass controls on the right.
import { t } from "../i18n";
import type { Engine } from "../ipc";
import { ENGINES, engineName } from "../app/commands";
import { settings } from "../app/settings";
import type { Workspace } from "../app/workspace";
import { h, setText, toggleClass } from "./dom";
import { icon } from "./icons";
import { popupMenu } from "./overlay";

export interface ToolbarActions {
  toggleSidebar(): void;
  togglePreview(): void;
  togglePdfOnly(): void;
  toggleIssues(): void;
}

export function createToolbar(ws: Workspace, actions: ToolbarActions): { el: HTMLElement; sidebarButton: HTMLElement; destroy(): void } {
  const title = h("div", { class: "tb-title", text: ws.name });
  const subtitle = h("div", { class: "tb-subtitle" });
  const sidebarButton = h("button", { class: "glass-circle tb-sidebar", title: t("Hide Sidebar"), attrs: { "aria-label": t("Hide Sidebar") } }, icon("sidebar", 17));
  sidebarButton.addEventListener("click", () => actions.toggleSidebar());

  const compile = h("button", { class: "tb-compile", title: t("Compile the document (⌘B)"), attrs: { "aria-label": t("Compile") } }, icon("play", 16), h("span", { class: "spinner" }));
  const stop = h("button", { class: "glass-circle tb-stop", title: t("Stop compiling (⌘.)"), attrs: { "aria-label": t("Stop") } }, icon("stop", 14));
  const engineLabel = h("span", { class: "tb-engine-name" });
  const engine = h("button", { class: "glass-capsule tb-engine", title: t("TeX engine used to compile"), attrs: { "aria-label": t("TeX engine") } }, icon("gears", 18), engineLabel, icon("chevronDown", 10, "chev"));
  const live = h("button", { class: "tb-toggle", attrs: { "aria-label": t("Live compile") } }, icon("bolt", 17));
  const preview = h("button", { class: "tb-toggle", attrs: { "aria-label": t("PDF preview") } }, icon("docRich", 17));
  const pdfOnly = h("button", { class: "tb-toggle" }, icon("detach", 17));
  const issues = h("button", { class: "tb-toggle", title: t("Show or hide the Issues panel (⇧⌘M)"), attrs: { "aria-label": t("Issues panel") } }, icon("warning", 18));
  const group = h("div", { class: "glass-capsule tb-group" }, live, preview, pdfOnly, issues);

  compile.addEventListener("click", () => ws.compile());
  stop.addEventListener("click", () => ws.stopCompile());
  live.addEventListener("click", () => settings.set("liveCompile", !settings.get("liveCompile")));
  preview.addEventListener("click", () => actions.togglePreview());
  pdfOnly.addEventListener("click", () => actions.togglePdfOnly());
  issues.addEventListener("click", () => actions.toggleIssues());
  engine.addEventListener("click", () => {
    const pick = (e: Engine | undefined) => ws.setEngineOverride(e);
    popupMenu([
      { kind: "header", title: t("Engine") },
      { title: t("Automatic (%@)", engineName(settings.get("defaultEngine"))), checked: ws.engineOverride === undefined, action: () => pick(undefined) },
      ...ENGINES.map((e) => ({ title: e.name, checked: ws.engineOverride === e.id, action: () => pick(e.id) })),
    ], engine, { align: "end", minWidth: 200 });
  });

  const el = h("header", { class: "toolbar" },
    h("div", { class: "tb-leading" }, h("div", { class: "tb-titles" }, title, subtitle)),
    h("div", { class: "tb-spacer" }),
    h("div", { class: "tb-trailing" }, compile, stop, engine, group));

  const renderTitle = () => {
    setText(subtitle, ws.current ? ws.relative(ws.current) : "");
    document.title = ws.current ? `${ws.name} — ${ws.relative(ws.current)}` : ws.name;
  };
  const renderCompile = () => {
    const compiling = ws.isCompiling;
    toggleClass(compile, "busy", compiling);
    compile.disabled = !ws.mainFile;
    stop.disabled = !compiling;
  };
  const renderEngine = () => setText(engineLabel, engineName(ws.effectiveEngine));
  const renderToggles = () => {
    const on = settings.get("liveCompile");
    toggleClass(live, "on", on);
    live.title = on ? t("Live compile is on: the document is compiled while you type") : t("Live compile is off");
    const previewOn = settings.get("showPreview") || ws.pdfOnly;
    toggleClass(preview, "on", previewOn);
    preview.disabled = ws.pdfOnly;
    preview.title = t("Show or hide the PDF preview (⌥⌘P)");
    toggleClass(pdfOnly, "on", ws.pdfOnly);
    pdfOnly.replaceChildren(icon(ws.pdfOnly ? "attach" : "detach", 17));
    const pdfTitle = ws.pdfOnly ? t("Show the editor again") : t("Maximize the PDF preview");
    pdfOnly.title = pdfTitle;
    pdfOnly.setAttribute("aria-label", pdfTitle);
    toggleClass(issues, "on", settings.get("showIssues"));
    issues.replaceChildren(icon(ws.errorCount > 0 ? "warningFill" : "warning", 18));
  };

  const offs = [
    ws.events.on("tabs", renderTitle),
    ws.events.on(["compile", "main"], renderCompile),
    ws.events.on(["engine", "main"], renderEngine),
    ws.events.on(["layout", "diagnostics"], renderToggles),
    settings.subscribe(() => { renderToggles(); renderEngine(); }, ["liveCompile", "showPreview", "showIssues", "defaultEngine"]),
  ];
  renderTitle();
  renderCompile();
  renderEngine();
  renderToggles();
  return { el, sidebarButton, destroy: () => offs.forEach((f) => f()) };
}

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
import { setWindowTitle } from "../app/windows";
import type { PresentationLink } from "../present/launch";

/** The project's separate windows, attached after the toolbar is built (PDF window, presentation). */
export interface ToolbarWindows {
  presentation: PresentationLink;
  isPdfDetached(): boolean;
  /** False without separate windows (browser mock): the button is PDF Only then. */
  canDetach(): boolean;
  togglePdfWindow(): void;
}

export interface ToolbarActions {
  toggleSidebar(): void;
  togglePreview(): void;
  togglePdfOnly(): void;
  toggleIssues(): void;
}

export function createToolbar(ws: Workspace, actions: ToolbarActions): { el: HTMLElement; sidebarButton: HTMLElement; attachWindows(w: ToolbarWindows): void; destroy(): void } {
  let windows: ToolbarWindows | null = null;
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
  // Present (Beamer only): click presents from the first slide (or ends); the chevron also offers the current slide.
  const presentMain = h("button", { class: "tb-present-main" }, icon("playRect", 17), h("span", { text: t("Present") }));
  const presentMore = h("button", { class: "tb-present-more", title: t("Present"), attrs: { "aria-label": t("Present"), "aria-haspopup": "menu" } }, icon("chevronDown", 10));
  const present = h("div", { class: "glass-capsule tb-present" }, presentMain, presentMore);
  present.hidden = true;
  presentMain.addEventListener("click", () => windows?.presentation.toggle());
  presentMore.addEventListener("click", () => {
    const p = windows?.presentation;
    if (!p) return;
    popupMenu([
      { title: t("Present from Beginning"), action: () => void p.present(false) },
      { title: t("Present from Current Slide"), action: () => void p.present(true) },
    ], presentMore, { align: "end", minWidth: 220 });
  });

  compile.addEventListener("click", () => ws.compile());
  stop.addEventListener("click", () => ws.stopCompile());
  live.addEventListener("click", () => settings.set("liveCompile", !settings.get("liveCompile")));
  preview.addEventListener("click", () => actions.togglePreview());
  pdfOnly.addEventListener("click", () => (windows?.canDetach() ? windows.togglePdfWindow() : actions.togglePdfOnly()));
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
    h("div", { class: "tb-trailing" }, compile, stop, present, engine, group));

  const renderTitle = () => {
    setText(subtitle, ws.current ? ws.relative(ws.current) : "");
    setWindowTitle(ws.current ? `${ws.name} — ${ws.relative(ws.current)}` : ws.name);
  };
  const renderCompile = () => {
    const compiling = ws.isCompiling;
    toggleClass(compile, "busy", compiling);
    compile.disabled = !ws.mainFile;
    stop.disabled = !compiling;
  };
  const renderPresent = () => {
    const p = windows?.presentation;
    present.hidden = !p || !(p.isPresentationDocument || p.presenting);
    if (!p || present.hidden) return;
    const can = p.canPresent || p.presenting;
    presentMain.disabled = !can;
    presentMore.disabled = !p.canPresent || p.presenting;
    toggleClass(present, "on", p.presenting);
    const help = p.presenting ? t("End Presentation") : p.canPresent ? t("Present the slides full screen (⌥⌘↩)") : t("Compile the document to present it");
    presentMain.title = help;
    presentMain.setAttribute("aria-label", p.presenting ? t("End Presentation") : t("Present"));
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
    const detached = !!windows?.isPdfDetached();
    if (detached) {
      preview.disabled = true;
      preview.title = t("The PDF preview is in a separate window");
      toggleClass(preview, "on", true);
    }
    const multi = !!windows?.canDetach();
    const pdfOn = multi ? detached : ws.pdfOnly;
    toggleClass(pdfOnly, "on", pdfOn);
    pdfOnly.replaceChildren(icon(pdfOn ? "attach" : "detach", 17));
    const pdfTitle = multi
      ? (detached ? t("Attach PDF to Project Window") : t("Open PDF in Separate Window"))
      : (ws.pdfOnly ? t("Show the editor again") : t("Maximize the PDF preview"));
    pdfOnly.title = pdfTitle;
    pdfOnly.setAttribute("aria-label", pdfTitle);
    renderPresent();
    toggleClass(issues, "on", settings.get("showIssues"));
    issues.replaceChildren(icon(ws.errorCount > 0 ? "warningFill" : "warning", 18));
  };

  const offs = [
    ws.events.on("tabs", renderTitle),
    ws.events.on(["compile", "main"], renderCompile),
    ws.events.on(["compile", "pdf"], renderPresent),
    ws.events.on(["engine", "main"], renderEngine),
    ws.events.on(["layout", "diagnostics"], renderToggles),
    settings.subscribe(() => { renderToggles(); renderEngine(); }, ["liveCompile", "showPreview", "showIssues", "defaultEngine"]),
  ];
  renderTitle();
  renderCompile();
  renderEngine();
  renderToggles();
  return {
    el, sidebarButton,
    attachWindows: (w) => { windows = w; renderToggles(); },
    destroy: () => offs.forEach((f) => f()),
  };
}

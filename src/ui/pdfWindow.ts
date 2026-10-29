// The detached PDF window (port of DetachedPDFWindow.swift): the project's PDF preview in a window of its own, with
// one toolbar button to attach it again. It runs its own pdf.js preview and stays in sync with its project window
// through window-targeted events (see src/app/pdfLink.ts): reloads on every compile, forward-sync highlights, and
// PDF → source clicks; shortcuts typed here run in the project window (Ctrl+B compiles, etc.).
import "./pdfWindow.css";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type { SyncBox } from "../ipc";
import { t } from "../i18n";
import { PDF_EVENT, type PdfFrame } from "../app/pdfLink";
import { commands } from "../app/commands";
import { comboFromEvent, shortcuts } from "../app/shortcuts";
import { setWindowTitle } from "../app/windows";
import { h } from "./dom";
import { icon } from "./icons";

export async function bootPdfWindow(root: HTMLElement): Promise<void> {
  const params = new URLSearchParams(location.search);
  const owner = params.get("owner") ?? "";
  const name = params.get("name") ?? "";
  setWindowTitle(t("%@ — PDF", name));
  document.documentElement.classList.add("pdf-window");

  const { emitTo } = await import("@tauri-apps/api/event");
  const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  const me = getCurrentWebviewWindow();
  const send = (event: string, payload: unknown = {}) => void emitTo(owner, event, payload).catch(() => {});

  const attachLabel = t("Attach PDF to Project Window");
  const attach = h("button", { class: "glass-circle pdfw-attach", title: attachLabel, attrs: { "aria-label": attachLabel } }, icon("attach", 17));
  attach.addEventListener("click", () => send(PDF_EVENT.attach));
  const host = h("div", { class: "pdfw-host" });
  root.append(h("div", { class: "pdfw-root" }, h("header", { class: "pdfw-toolbar" }, h("div", { class: "pdfw-title", text: name }), h("div", { class: "tb-spacer" }), attach), host));

  const { createPreview } = await import("../preview");
  const preview = createPreview(host, {
    onInverseSync: (page, x, y) => send(PDF_EVENT.inverse, { page, x, y }),
    onPageChange: (page, count) => send(PDF_EVENT.page, { page, count }),
  });

  const unlisten: UnlistenFn[] = [];
  unlisten.push(
    await me.listen<{ path: string }>(PDF_EVENT.load, (e) => void preview.load(e.payload.path).catch((err) => console.warn("PDF load failed", err))),
    await me.listen<{ boxes: SyncBox[] }>(PDF_EVENT.highlight, (e) => preview.highlight(e.payload.boxes)),
    await me.listen(PDF_EVENT.clear, () => preview.clear()),
  );

  // Shortcuts: Ctrl+W closes this window (the preview docks again); every other command runs in the project window.
  commands();
  document.addEventListener("keydown", (e) => {
    if (e.defaultPrevented || e.isComposing) return;
    const combo = comboFromEvent(e);
    if (!combo) return;
    if (combo === "Ctrl+W") { e.preventDefault(); void me.close(); return; }
    const id = shortcuts.commands(combo)[0];
    if (!id) return;
    e.preventDefault();
    send(PDF_EVENT.command, { id });
  }, true);
  document.addEventListener("contextmenu", (e) => {
    if (!(e.target as HTMLElement).closest(".textLayer")) e.preventDefault();
  });

  // Closing the window docks the preview again (the project remembers where the window was).
  await me.onCloseRequested(async () => {
    let frame: PdfFrame | undefined;
    try {
      const [pos, size, scale] = await Promise.all([me.outerPosition(), me.innerSize(), me.scaleFactor()]);
      frame = { x: Math.round(pos.x / scale), y: Math.round(pos.y / scale), width: Math.round(size.width / scale), height: Math.round(size.height / scale) };
    } catch {
      frame = undefined;
    }
    for (const u of unlisten) u();
    await emitTo(owner, PDF_EVENT.closed, { frame }).catch(() => {});
  });

  send(PDF_EVENT.ready);
}

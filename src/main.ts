// Entry point: localization and settings first (one IPC round-trip each), then the shell. The editor and the PDF
// preview are separate chunks, loaded when a project opens.
import "./styles/main.css";
import { initI18n } from "./i18n";
import { settings } from "./app/settings";
import { App } from "./app/app";
import { windowRole } from "./app/windows";

async function boot(): Promise<void> {
  const root = document.getElementById("app");
  if (!root) return;
  // Plain browser (no Tauri): a fake backend for development and visual QA. The condition is a build-time constant,
  // so production builds drop the mock chunk entirely.
  if ((import.meta.env.MODE === "development" || import.meta.env.MODE === "mock") && !("__TAURI_INTERNALS__" in window)) {
    (await import("./mock/ipc-mock")).installMockBackend();
  }
  await Promise.all([initI18n(), settings.load()]);
  settings.syncAcrossWindows();
  // The window's label says what it shows (src/app/windows.ts): the PDF and presentation windows load only their
  // own chunk; everything else is the app shell (Welcome or a project).
  const role = windowRole();
  if (role === "pdf") await (await import("./ui/pdfWindow")).bootPdfWindow(root);
  else if (role === "present") await (await import("./present/presentation")).bootPresentationWindow(root);
  else if (role === "presenter") await (await import("./present/presentation")).bootPresenterWindow(root);
  else await new App(root).start();
  document.documentElement.classList.add("ready");
  window.addEventListener("beforeunload", () => void settings.flush());
}

void boot();

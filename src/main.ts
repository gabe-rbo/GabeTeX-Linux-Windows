// Entry point: localization and settings first (one IPC round-trip each), then the shell. The editor and the PDF
// preview are separate chunks, loaded when a project opens.
import "./styles/main.css";
import { initI18n } from "./i18n";
import { settings } from "./app/settings";
import { App } from "./app/app";

async function boot(): Promise<void> {
  const root = document.getElementById("app");
  if (!root) return;
  // Plain browser (no Tauri): a fake backend for development and visual QA. The condition is a build-time constant,
  // so production builds drop the mock chunk entirely.
  if ((import.meta.env.MODE === "development" || import.meta.env.MODE === "mock") && !("__TAURI_INTERNALS__" in window)) {
    (await import("./mock/ipc-mock")).installMockBackend();
  }
  await Promise.all([initI18n(), settings.load()]);
  const app = new App(root);
  await app.start();
  document.documentElement.classList.add("ready");
  window.addEventListener("beforeunload", () => void settings.flush());
}

void boot();

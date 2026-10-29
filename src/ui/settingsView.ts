// Settings (port of SettingsView and its panes): General, Editor, Shortcuts and TeX, shown as a glass sheet with
// a macOS-style pane bar. Changes apply immediately and persist through the settings store.
import * as ipc from "../ipc";
import { t, formatDecimal } from "../i18n";
import { settings, type AppSettingsData } from "../app/settings";
import { ENGINES, commands, CATEGORY_TITLES, type Category } from "../app/commands";
import { shortcuts, comboFromEvent, hasCommandModifier } from "../app/shortcuts";
import { h, setText } from "./dom";
import { icon } from "./icons";
import { showSheet, alertDialog } from "./overlay";

type Pane = "general" | "editor" | "shortcuts" | "tex";
let lastPane: Pane = "general";

type BoolKey = { [K in keyof AppSettingsData]: AppSettingsData[K] extends boolean ? K : never }[keyof AppSettingsData];

function toggleRow(label: string, key: BoolKey): HTMLElement {
  const input = h("input", { class: "switch", checked: settings.get(key), attrs: { type: "checkbox", role: "switch", "aria-label": label } });
  input.addEventListener("change", () => settings.set(key, input.checked));
  return h("label", { class: "set-row" }, h("span", { class: "set-label", text: label }), input);
}

function section(title: string, rows: Array<HTMLElement | null>, footer?: string): HTMLElement {
  return h("div", { class: "set-section" },
    h("div", { class: "set-section-title", text: title }),
    h("div", { class: "set-group" }, rows),
    footer ? h("div", { class: "set-footer", text: footer }) : null);
}

function row(label: string, control: HTMLElement): HTMLElement {
  return h("div", { class: "set-row" }, h("span", { class: "set-label", text: label }), control);
}

function select<T extends string>(label: string, value: T, options: Array<{ value: T; title: string }>, onChange: (v: T) => void): HTMLElement {
  const s = h("select", { class: "popup", attrs: { "aria-label": label } }, options.map((o) => h("option", { value: o.value, text: o.title, selected: o.value === value })));
  s.addEventListener("change", () => onChange(s.value as T));
  return row(label, s);
}

function slider(label: string, min: number, max: number, step: number, value: number, format: (v: number) => string, onChange: (v: number) => void): HTMLElement {
  const out = h("span", { class: "set-value", text: format(value) });
  const input = h("input", { class: "slider", value: String(value), attrs: { type: "range", min, max, step, "aria-label": label } });
  input.addEventListener("input", () => { const v = Number(input.value); setText(out, format(v)); onChange(v); });
  return row(label, h("span", { class: "set-inline" }, input, out));
}

function stepper(label: string, min: number, max: number, value: number, format: (v: number) => string, onChange: (v: number) => void): HTMLElement {
  let v = value;
  const out = h("span", { class: "set-value", text: format(v) });
  const btn = (delta: number, ic: string) => h("button", { class: "step-btn", attrs: { "aria-label": `${label} ${delta > 0 ? "+" : "−"}` }, on: { click: () => { v = Math.min(max, Math.max(min, v + delta)); setText(out, format(v)); onChange(v); } } }, icon(ic, 11));
  return row(label, h("span", { class: "set-inline" }, out, h("span", { class: "stepper" }, btn(-1, "minus"), btn(1, "plus"))));
}

function generalPane(): HTMLElement {
  return h("div", { class: "set-pane" },
    section(t("Compilation"), [
      select(t("Default engine"), settings.get("defaultEngine"), ENGINES.map((e) => ({ value: e.id, title: e.name })), (v) => settings.set("defaultEngine", v)),
      toggleRow(t("Compile while typing"), "liveCompile"),
      slider(t("Wait after typing"), 0.5, 5, 0.5, settings.get("liveDelay"), (v) => t("%@ s", formatDecimal(v, 1)), (v) => settings.set("liveDelay", v)),
      toggleRow(t("Compile when saving"), "compileOnSave"),
    ], t("Live compiling uses a private copy of your project, so unsaved text is checked without touching your files. A “% !TEX program = …” comment in the main file overrides the engine.")),
    section(t("PDF Preview"), [
      toggleRow(t("Show the PDF position after each build"), "syncAfterBuild"),
      toggleRow(t("Follow cursor in PDF"), "followCursorInPDF"),
    ], t("Clicking text in the PDF takes you to the same word in the source, and ⇧⌘J (Build ▸ Show in PDF) or ⌘-click in the editor shows where a word is in the PDF. “Follow cursor” keeps the PDF scrolled to the cursor once it rests.")),
    h("div", { class: "set-footer standalone", text: `${t("Language follows the system.")} ${t("Appearance follows the system (Light or Dark).")}` }),
  );
}

function editorPane(): HTMLElement {
  const preview = h("pre", { class: "font-preview", text: "\\begin{equation}\n  e^{i\\pi} + 1 = 0\n\\end{equation}" });
  const applyPreview = () => {
    preview.style.fontSize = `${settings.get("fontSize")}px`;
    preview.style.lineHeight = String(settings.get("lineHeight"));
  };
  applyPreview();
  return h("div", { class: "set-pane" },
    section(t("Font"), [
      stepper(t("Size"), 8, 40, settings.get("fontSize"), (v) => t("%lld pt", v), (v) => { settings.set("fontSize", v); applyPreview(); }),
      slider(t("Line height"), 1, 2.2, 0.05, settings.get("lineHeight"), (v) => t("%@×", formatDecimal(v, 2)), (v) => { settings.set("lineHeight", v); applyPreview(); }),
      h("div", { class: "set-row preview-row" }, preview),
    ]),
    section(t("Display"), [
      toggleRow(t("Show line numbers"), "showLineNumbers"),
      toggleRow(t("Wrap long lines"), "wrapLines"),
      toggleRow(t("Check spelling while typing"), "spellChecking"),
    ]),
    section(t("Indentation"), [
      stepper(t("Indent width"), 1, 8, settings.get("indentWidth"), (v) => String(v), (v) => settings.set("indentWidth", v)),
    ]),
    section(t("Keyboard"), [toggleRow(t("Vim mode"), "vimMode")], t("Modal editing with motions, operators, text objects and ex commands such as :w and :make.")),
  );
}

function shortcutsPane(): HTMLElement {
  const search = h("input", { class: "search-input", attrs: { type: "search", placeholder: t("Search Shortcuts"), "aria-label": t("Search Shortcuts") } });
  const restore = h("button", { class: "btn glass small", text: t("Restore Defaults…"), title: t("Restore every keyboard shortcut to its default") });
  const list = h("div", { class: "shortcut-list" });
  const render = () => {
    const q = search.value.trim().toLowerCase();
    list.textContent = "";
    const cats: Category[] = ["file", "edit", "insert", "view", "build", "window", "app"];
    let any = false;
    for (const cat of cats) {
      const cmds = commands().filter((c) => c.category === cat && c.showsInSettings && (!q || c.title.toLowerCase().includes(q) || (shortcuts.combo(c.id) ?? "").toLowerCase().includes(q)));
      if (!cmds.length) continue;
      any = true;
      list.appendChild(section(t(CATEGORY_TITLES[cat]), cmds.map((c) => row(c.title, recorder(c.id, c.title, render)))));
    }
    if (!any) list.appendChild(h("div", { class: "empty-detail", text: t("No matching commands") }));
  };
  search.addEventListener("input", render);
  restore.addEventListener("click", async () => {
    const answer = await alertDialog({
      title: t("Restore all keyboard shortcuts to their defaults?"), message: t("Shortcuts you recorded will be replaced by GabeTeX’s defaults."),
      buttons: [{ title: t("Restore Defaults"), role: "destructive", value: "restore" }, { title: t("Cancel"), role: "cancel", value: "cancel" }],
    });
    if (answer === "restore") { shortcuts.restoreAll(); render(); }
  });
  render();
  return h("div", { class: "set-pane" }, h("div", { class: "shortcut-tools" }, h("div", { class: "search-field glass-capsule" }, icon("magnifier", 13), search), restore), list);
}

/** A click-to-record shortcut field (port of ShortcutRecorderView). */
function recorder(id: string, title: string, rerender: () => void): HTMLElement {
  const customizable = shortcuts.isCustomizable(id);
  const field = h("button", {
    class: "recorder" + (shortcuts.isCustomized(id) ? " custom" : ""), disabled: !customizable,
    title: customizable ? t("Click, then press the new shortcut. ⌫ removes it, Esc cancels.") : t("This shortcut can’t be changed."),
    attrs: { "aria-label": t("Shortcut for %@", title) },
  });
  const show = () => setText(field, shortcuts.combo(id) ?? t("None"));
  show();
  let recording = false;
  const stopRecording = () => { recording = false; field.classList.remove("recording"); show(); document.removeEventListener("keydown", onKey, true); };
  const onKey = async (e: KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") { stopRecording(); return; }
    if ((e.key === "Backspace" || e.key === "Delete") && !e.ctrlKey && !e.altKey && !e.metaKey) {
      shortcuts.assign(id, undefined);
      stopRecording();
      rerender();
      return;
    }
    const combo = comboFromEvent(e);
    if (!combo) { setText(field, [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift"].filter(Boolean).join("+") + "+…"); return; }
    if (!hasCommandModifier(combo)) { setText(field, t("Type Shortcut")); return; }
    stopRecording();
    const outcome = shortcuts.assign(id, combo);
    if (!outcome.ok) {
      const names = outcome.conflicts.map((c) => commands().find((x) => x.id === c)?.title ?? c).join(", ");
      if (outcome.reason === "conflict") {
        const answer = await alertDialog({
          title: t("%@ is already used by %@.", combo, `“${names}”`),
          buttons: [{ title: t("Replace"), role: "default", value: "replace" }, { title: t("Cancel"), role: "cancel", value: "cancel" }],
        });
        if (answer === "replace") shortcuts.assign(id, combo, true);
      } else {
        await alertDialog({ title: t("This shortcut can’t be changed."), message: `${combo} — ${names}` });
      }
    }
    rerender();
  };
  field.addEventListener("click", () => {
    if (recording) { stopRecording(); return; }
    recording = true;
    field.classList.add("recording");
    setText(field, t("Type Shortcut"));
    document.addEventListener("keydown", onKey, true);
  });
  field.addEventListener("blur", () => { if (recording) stopRecording(); });
  const reset = h("button", { class: "icon-btn reset", title: t("Restore Default"), attrs: { "aria-label": t("Restore Default") } }, icon("sync", 12));
  reset.hidden = !shortcuts.isCustomized(id);
  reset.addEventListener("click", () => { shortcuts.restoreDefault(id); rerender(); });
  return h("span", { class: "set-inline" }, reset, field);
}

function texPane(): HTMLElement {
  const body = h("div", { class: "set-group" }, h("div", { class: "set-row" }, h("span", { class: "set-label dim", text: t("Looking for TeX…") })));
  const render = (d: ipc.TeXDistribution | null) => {
    body.textContent = "";
    if (!d) {
      body.append(h("div", { class: "set-row" }, h("span", { class: "set-label" }, icon("warningFill", 14, "tint-orange"), " ", t("No TeX distribution was found"))),
        h("div", { class: "set-row" }, h("span", { class: "set-label dim", text: t("Install TeX Live or MiKTeX to compile documents.") })));
      return;
    }
    const yes = (ok: boolean) => h("span", { class: "set-value " + (ok ? "ok" : "bad"), text: ok ? t("Available") : t("Not found") });
    body.append(
      row(t("Distribution"), h("span", { class: "set-value", text: d.name })),
      row(t("Location"), h("span", { class: "set-value mono selectable", text: d.binDir })),
      row("latexmk", yes(d.latexmk)),
      row("tlmgr", yes(d.tlmgr)),
      row("synctex", yes(d.synctex)),
    );
  };
  const detect = h("button", { class: "btn glass small", title: t("Look for TeX again") }, icon("sync", 13), h("span", { text: t("Detect Again") }));
  const run = () => { ipc.detectTex().then(render).catch(() => render(null)); };
  detect.addEventListener("click", run);
  run();
  return h("div", { class: "set-pane" },
    h("div", { class: "set-section" }, h("div", { class: "set-section-title row" }, h("span", { text: t("Distribution") }), detect), body));
}

/** Opens Settings on `pane` (default: the last one shown). */
export function openSettings(pane?: string): void {
  let active: Pane = (["general", "editor", "shortcuts", "tex"] as const).find((p) => p === pane) ?? lastPane;
  const content = h("div", { class: "set-content" });
  const bar = h("div", { class: "set-bar", attrs: { role: "tablist" } });
  const panes: Array<{ id: Pane; title: string; icon: string }> = [
    { id: "general", title: t("General"), icon: "gear" },
    { id: "editor", title: t("Editor"), icon: "cursorText" },
    { id: "shortcuts", title: t("Shortcuts"), icon: "keyboard" },
    { id: "tex", title: t("TeX"), icon: "function" },
  ];
  const title = h("div", { class: "set-title" });
  const render = () => {
    lastPane = active;
    bar.textContent = "";
    for (const p of panes) {
      const b = h("button", { class: "set-tab" + (p.id === active ? " selected" : ""), attrs: { role: "tab", "aria-selected": String(p.id === active) } }, icon(p.icon, 20), h("span", { text: p.title }));
      b.addEventListener("click", () => { active = p.id; render(); });
      bar.appendChild(b);
    }
    setText(title, panes.find((p) => p.id === active)!.title);
    content.replaceChildren(active === "general" ? generalPane() : active === "editor" ? editorPane() : active === "shortcuts" ? shortcutsPane() : texPane());
  };
  render();
  const close = h("button", { class: "icon-btn set-close", title: t("Close"), attrs: { "aria-label": t("Close") } }, icon("xmark", 13));
  const sheet = showSheet(h("div", { class: "settings" }, h("div", { class: "set-head" }, title, close), bar, content), { class: "settings-sheet", label: t("Settings") });
  close.addEventListener("click", () => sheet.close());
}

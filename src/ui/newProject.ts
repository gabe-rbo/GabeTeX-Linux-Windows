// “New Project…” sheet (port of NewProjectView.swift): choose a template, a title, an author and where to create
// the folder. The Rust side writes the template (`create_project`).
import * as ipc from "../ipc";
import { t } from "../i18n";
import { settings } from "../app/settings";
import { h, abbreviateHome, joinPath, ltr } from "./dom";
import { icon, symbolIcon } from "./icons";
import { showSheet } from "./overlay";

interface TemplateInfo { id: string; name: string; summary: string; symbol: string; files: string[] }

/** The Mac app's templates, in its order (data from `src-tauri/src/core/data/templates.json`). */
const TEMPLATES: TemplateInfo[] = [
  { id: "article", name: "Article", summary: "A simple article in a single file.", symbol: "doc.text", files: ["main.tex"] },
  { id: "article-sections", name: "Article with Sections", summary: "A main file that inputs one file per section, plus a bibliography.", symbol: "list.bullet.indent", files: ["main.tex", "sections/introduction.tex", "sections/preliminaries.tex", "sections/main-results.tex", "sections/conclusion.tex", "references.bib"] },
  { id: "thesis", name: "Thesis", summary: "A book-class thesis with front matter, chapters and biblatex.", symbol: "book.closed", files: ["main.tex", "chapters/introduction.tex", "chapters/background.tex", "chapters/conclusion.tex", "references.bib", "figures/README.txt"] },
  { id: "beamer", name: "Beamer Presentation", summary: "Slides with a title page, blocks and columns.", symbol: "play.rectangle", files: ["main.tex"] },
  { id: "math-notes", name: "Math Notes", summary: "Theorem environments, macros, a commutative diagram and hyperlinks.", symbol: "function", files: ["main.tex", "macros.tex"] },
  { id: "problem-set", name: "Problem Set", summary: "Numbered problems and solutions with a running header.", symbol: "checklist", files: ["main.tex"] },
];

/** Folder name suggested for a title (mirrors `suggested_folder_name`: path separators and controls removed). */
export function suggestedFolderName(title: string): string {
  const cleaned = title.replace(/[/\\:*?"<>|\u0000-\u001f]/g, " ").replace(/\s+/g, " ").trim().replace(/^\.+/, "");
  return cleaned || t("Untitled Project");
}

async function defaultLocation(): Promise<string> {
  const saved = settings.get("newProjectLocation");
  if (saved) return saved;
  try {
    const { documentDir, homeDir } = await import("@tauri-apps/api/path");
    return await documentDir().catch(() => homeDir());
  } catch {
    return "";
  }
}

/** Opens the sheet; `onCreated` receives the new project (root + main file). */
export function openNewProject(onCreated: (info: ipc.ProjectInfo) => void): void {
  let templateId = TEMPLATES[0]!.id;
  let location = "";
  const listEl = h("div", { class: "template-list", attrs: { role: "listbox", "aria-label": t("Choose a Template") } });
  const detailHead = h("div", { class: "template-head" });
  const filesEl = h("div", { class: "template-files" });
  const title = h("input", { class: "field", attrs: { type: "text", placeholder: t("Untitled Project"), "aria-label": t("Title"), autofocus: true } });
  const author = h("input", { class: "field", value: settings.get("author"), attrs: { type: "text", "aria-label": t("Author") } });
  const folderLabel = h("span", { class: "form-value mono" });
  const locationLabel = h("span", { class: "form-value path" });
  const errorEl = h("div", { class: "form-error", attrs: { role: "alert" } });
  const create = h("button", { class: "btn prominent large", text: t("Create") });

  const renderTemplates = () => {
    listEl.textContent = "";
    for (const tp of TEMPLATES) {
      const row = h("button", { class: "template-row" + (tp.id === templateId ? " selected" : ""), attrs: { role: "option", "aria-selected": String(tp.id === templateId) } },
        icon(symbolIcon(tp.symbol), 17), h("span", { text: t(tp.name) }));
      row.addEventListener("click", () => { templateId = tp.id; renderTemplates(); renderDetail(); });
      row.addEventListener("dblclick", () => void submit());
      listEl.appendChild(row);
    }
  };
  const renderDetail = () => {
    const tp = TEMPLATES.find((x) => x.id === templateId)!;
    detailHead.textContent = "";
    detailHead.append(h("span", { class: "template-symbol" }, icon(symbolIcon(tp.symbol), 28)),
      h("div", null, h("div", { class: "template-name", text: t(tp.name) }), h("div", { class: "template-summary", text: t(tp.summary) })));
    filesEl.textContent = "";
    filesEl.append(h("div", { class: "form-section-title", text: t("Files") }),
      ...tp.files.map((f, i) => h("div", { class: "template-file" }, icon(i === 0 ? "starFill" : "docText", 12, i === 0 ? "star" : ""), h("span", { class: "mono", text: f }))));
  };
  const renderFolder = () => {
    folderLabel.textContent = suggestedFolderName(title.value);
    locationLabel.textContent = location ? ltr(abbreviateHome(location)) : "—";
    locationLabel.title = location;
  };
  title.addEventListener("input", renderFolder);

  const chooseLocation = async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true, multiple: false, defaultPath: location || undefined, title: t("Choose the folder in which the project folder will be created.") }).catch(() => null);
    if (typeof picked === "string") { location = picked; renderFolder(); }
  };

  const submit = async () => {
    errorEl.textContent = "";
    if (!location) { await chooseLocation(); if (!location) return; }
    const name = title.value.trim() || t("Untitled Project");
    create.disabled = true;
    try {
      settings.set("author", author.value.trim());
      settings.set("newProjectLocation", location);
      await settings.flush(); // the backend reads the author from the saved settings
      const info = await ipc.createProject(location, name, templateId);
      sheet.close();
      onCreated(info);
    } catch (e) {
      const msg = String(e);
      errorEl.textContent = /already exists|not empty/i.test(msg)
        ? t("“%@” already exists in this location. Choose another name or location.", joinPath(location, suggestedFolderName(name)))
        : t("The project could not be created: %@", msg);
      create.disabled = false;
    }
  };
  create.addEventListener("click", () => void submit());
  for (const el of [title, author]) el.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); void submit(); } });

  renderTemplates();
  renderDetail();
  renderFolder();
  void defaultLocation().then((l) => { location = l; renderFolder(); });

  const row = (label: string, control: HTMLElement) => h("label", { class: "form-row" }, h("span", { class: "form-label", text: label }), control);
  const sheet = showSheet(
    h("div", { class: "new-project" },
      h("div", { class: "np-templates" }, h("div", { class: "sheet-title", text: t("Choose a Template") }), listEl),
      h("div", { class: "vdivider" }),
      h("div", { class: "np-details" },
        detailHead,
        h("div", { class: "form" },
          row(t("Title"), title),
          row(t("Author"), author),
          row(t("Folder name"), folderLabel),
          h("div", { class: "form-row" }, h("span", { class: "form-label", text: t("Location") }),
            h("span", { class: "form-inline" }, locationLabel, h("button", { class: "btn glass small", text: t("Choose…"), on: { click: () => void chooseLocation() } })))),
        filesEl, errorEl,
        h("div", { class: "spacer-v" }),
        h("div", { class: "alert-buttons row end" }, h("button", { class: "btn glass large", text: t("Cancel"), on: { click: () => sheet.close() } }), create))),
    { class: "new-project-sheet", label: t("New Project…") });
}

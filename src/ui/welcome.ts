// The Welcome view (port of WelcomeView.swift + RecentProjectsColumn.swift): app identity, the three ways to start,
// and recent projects with colour labels, label filter chips and (for long lists) a search field.
import * as ipc from "../ipc";
import type { RecentProject } from "../ipc";
import { t } from "../i18n";
import * as labels from "../app/labels";
import { settings } from "../app/settings";
import { h, basename, dirname, abbreviateHome, ltr } from "./dom";
import { icon, appIcon } from "./icons";
import { alertDialog, popupMenu, type MenuItem } from "./overlay";
import { openLabelEditor } from "./labelEditor";

export interface WelcomeActions {
  newProject(): void;
  openFolder(): void;
  openFile(): void;
  openProject(path: string): void;
}

const SEARCH_THRESHOLD = 8;

export function createWelcome(parent: HTMLElement, actions: WelcomeActions): { destroy(): void; refresh(): void } {
  let recents: RecentProject[] = [];
  let filter: string | null = null;
  let query = "";

  const hero = h("div", { class: "welcome-hero" },
    h("div", { class: "welcome-icon" }, appIcon(104)),
    h("div", { class: "welcome-name", text: "GabeTeX" }),
    h("div", { class: "welcome-tagline", text: t("A LaTeX editor for projects, not just files.") }),
    h("div", { class: "welcome-actions" },
      h("button", { class: "btn prominent large", title: t("Create a project from a template (⇧⌘N)"), on: { click: () => actions.newProject() } }, icon("plus", 15), h("span", { text: t("New Project…") })),
      h("button", { class: "btn glass large", title: t("Open a folder as a project (⌘O)"), on: { click: () => actions.openFolder() } }, icon("folder", 16), h("span", { text: t("Open Folder…") })),
      h("button", { class: "btn glass large", title: t("Open a .tex file; its folder becomes the project (⌥⌘O)"), on: { click: () => actions.openFile() } }, icon("docText", 16), h("span", { text: t("Open File…") })),
    ),
  );

  const chips = h("div", { class: "chips", attrs: { role: "toolbar", "aria-label": t("Label filter") } });
  const searchInput = h("input", { class: "search-input", attrs: { type: "search", placeholder: t("Search Projects"), "aria-label": t("Search Projects") } });
  const search = h("div", { class: "search-field glass-capsule" }, icon("magnifier", 13), searchInput);
  const list = h("div", { class: "recent-list", attrs: { role: "list" } });
  const clearBtn = h("button", { class: "link-btn", text: t("Clear"), title: t("Clear the list of recent projects (labeled projects stay)") });
  const tagBtn = h("button", { class: "icon-btn", title: t("Create a label to organize your projects"), attrs: { "aria-label": t("Labels") } }, icon("tag", 17));

  const column = h("div", { class: "welcome-recents" },
    h("div", { class: "recents-header" }, h("div", { class: "recents-title", text: t("Recent Projects") }), h("div", { class: "spacer" }), tagBtn, clearBtn),
    chips, search, list);

  const card = h("div", { class: "welcome-card glass-window" }, hero, h("div", { class: "vdivider" }), column);
  const root = h("div", { class: "welcome-root" }, card);
  parent.appendChild(root);

  tagBtn.addEventListener("click", () => popupMenu([{ title: t("New Label…"), icon: "tag", action: () => openLabelEditor({ onDone: render }) }], tagBtn, { align: "end" }));
  clearBtn.addEventListener("click", () => {
    settings.update("hiddenRecents", (map) => {
      for (const r of recents) if (!labels.labelsFor(r.path).length) map[r.path] = r.lastOpened;
    });
    render();
  });
  searchInput.addEventListener("input", () => { query = searchInput.value.trim(); renderList(); });

  const visibleRecents = () => {
    const hidden = settings.get("hiddenRecents");
    return recents.filter((r) => {
      const stamp = hidden[r.path];
      return stamp === undefined || r.lastOpened > stamp || labels.labelsFor(r.path).length > 0;
    });
  };

  function renderChips(): void {
    const all = labels.allLabels();
    chips.textContent = "";
    chips.hidden = all.length === 0;
    if (filter && !all.some((l) => l.id === filter)) filter = null;
    const chip = (title: string, id: string | null, color?: string) => {
      const b = h("button", { class: "chip" + (filter === id ? " selected" : ""), attrs: { "aria-pressed": String(filter === id) } },
        color ? h("span", { class: "label-dot", style: { background: color } }) : null, h("span", { text: title }));
      b.addEventListener("click", () => { filter = filter === id ? null : id; render(); });
      if (id) {
        b.addEventListener("contextmenu", (e) => {
          e.preventDefault();
          const label = all.find((l) => l.id === id);
          if (!label) return;
          popupMenu([
            { title: t("Rename…"), action: () => openLabelEditor({ editId: id, onDone: render }) },
            { kind: "header", title: t("Color") },
            ...labels.LABEL_COLORS.map((c): MenuItem => ({ title: labels.colorTitle(c), dot: labels.labelCSS(c), checked: label.color === c, action: () => { labels.editLabel(id, label.name, c); render(); } })),
            { kind: "separator" },
            { title: t("Delete Label"), destructive: true, action: () => void confirmDelete(id, label.name) },
          ], { x: e.clientX, y: e.clientY });
        });
        // Drop a project row on a chip to label it.
        b.addEventListener("dragover", (e) => { if (e.dataTransfer?.types.includes("text/x-gabetex-project")) { e.preventDefault(); b.classList.add("drop"); } });
        b.addEventListener("dragleave", () => b.classList.remove("drop"));
        b.addEventListener("drop", (e) => {
          b.classList.remove("drop");
          const path = e.dataTransfer?.getData("text/x-gabetex-project");
          if (path) { e.preventDefault(); labels.toggle(id, path, true); render(); }
        });
      }
      chips.appendChild(b);
    };
    chip(t("All"), null);
    for (const l of all) chip(l.name, l.id, labels.labelCSS(l.color));
  }

  async function confirmDelete(id: string, name: string): Promise<void> {
    const answer = await alertDialog({
      title: t("Delete the label “%@”?", name),
      message: t("The label is removed from all projects. Your project folders are not changed."),
      buttons: [{ title: t("Delete Label"), role: "destructive", value: "delete" }, { title: t("Cancel"), role: "cancel", value: "cancel" }],
    });
    if (answer === "delete") { labels.deleteLabel(id); if (filter === id) filter = null; render(); }
  }

  function badges(path: string): HTMLElement | null {
    const ls = labels.labelsFor(path);
    if (!ls.length) return null;
    const compact = ls.length > 2 || ls.reduce((n, l) => n + l.name.length, 0) > 22;
    if (compact) {
      return h("span", { class: "badges compact", title: ls.map((l) => l.name).join(", ") },
        ls.slice(0, 4).map((l) => h("span", { class: "label-dot big", style: { background: labels.labelCSS(l.color) } })),
        ls.length > 4 ? h("span", { class: "badge-more", text: `+${ls.length - 4}` }) : null);
    }
    return h("span", { class: "badges" }, ls.map((l) =>
      h("span", { class: "badge", style: `--tint: ${labels.labelCSS(l.color)}` },
        h("span", { class: "label-dot", style: { background: labels.labelCSS(l.color) } }), h("span", { text: l.name }))));
  }

  function renderList(): void {
    const all = visibleRecents();
    search.hidden = all.length <= SEARCH_THRESHOLD;
    clearBtn.hidden = all.length === 0;
    const needle = search.hidden ? "" : query.toLowerCase();
    const items = all.filter((r) => {
      if (filter && !labels.isAssigned(filter, r.path)) return false;
      if (needle) return r.name.toLowerCase().includes(needle) || r.path.toLowerCase().includes(needle);
      return true;
    });
    list.textContent = "";
    if (!all.length || !items.length) {
      const none = !all.length;
      list.appendChild(h("div", { class: "empty-state" },
        icon(none ? "clock" : "filter", 30),
        h("div", { class: "empty-title", text: none ? t("No Recent Projects") : t("No Matching Projects") }),
        h("div", { class: "empty-detail", text: none ? t("Projects you open appear here.") : filter ? t("No recent project has this label.") : t("No recent project matches your search.") })));
      return;
    }
    const frag = document.createDocumentFragment();
    for (const r of items) {
      const name = r.name || basename(r.path);
      const row = h("button", { class: "recent-row", title: r.path, draggable: true, attrs: { role: "listitem", "aria-label": t("Open %@", name) } },
        h("span", { class: "folder-icon" }, icon("folderFill", 30)),
        h("span", { class: "recent-text" },
          h("span", { class: "recent-name-line" }, h("span", { class: "recent-name", text: name }), badges(r.path)),
          h("span", { class: "recent-path", text: ltr(abbreviateHome(dirname(r.path))) })));
      row.addEventListener("click", () => actions.openProject(r.path));
      row.addEventListener("dragstart", (e) => e.dataTransfer?.setData("text/x-gabetex-project", r.path));
      row.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        popupMenu([
          { title: t("Open"), action: () => actions.openProject(r.path) },
          { kind: "header", title: t("Labels") },
          ...labels.allLabels().map((l): MenuItem => ({ title: l.name, dot: labels.labelCSS(l.color), checked: labels.isAssigned(l.id, r.path), action: () => { labels.toggle(l.id, r.path); render(); } })),
          { title: t("New Label…"), icon: "tag", action: () => openLabelEditor({ assignTo: r.path, onDone: render }) },
          { kind: "separator" },
          { title: t("Reveal in File Manager"), action: () => void ipc.revealInFileManager(r.path).catch(() => {}) },
          { title: t("Remove from Recents"), action: () => { void ipc.removeRecentProject(r.path).then((list) => { recents = list; render(); }).catch(() => { settings.update("hiddenRecents", (m) => { m[r.path] = r.lastOpened; }); render(); }); } },
        ], { x: e.clientX, y: e.clientY });
      });
      frag.appendChild(row);
    }
    list.appendChild(frag);
  }

  function render(): void {
    renderChips();
    renderList();
  }

  const refresh = () => {
    ipc.recentProjects().then((r) => { recents = r; render(); }).catch(() => render());
  };
  refresh();

  return { destroy: () => root.remove(), refresh };
}

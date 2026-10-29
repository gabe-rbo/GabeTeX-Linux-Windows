// The sidebar (port of SidebarView / FileTreeView / OutlineSidebarView): a translucent glass column with a
// Files | Outline segmented control, the file tree (main document star, unsaved dot, context menu) and the outline.
import type { FileNode, Label, OutlineItem } from "../ipc";
import { t, formatInt } from "../i18n";
import type { Workspace } from "../app/workspace";
import { h, toggleClass } from "./dom";
import { icon, fileIconName } from "./icons";
import { popupMenu, promptDialog, toast, type MenuItem } from "./overlay";

const LABEL_LEVEL = 99;

/** Interleaves `\label` entries after the heading that precedes them in the same file. */
function withLabels(outline: OutlineItem[], labels: Label[]): OutlineItem[] {
  if (!labels.length) return outline;
  const out = outline.slice();
  for (const l of [...labels].sort((a, b) => b.line - a.line)) {
    const entry: OutlineItem = { level: LABEL_LEVEL, title: "", path: l.path, line: l.line, label: l.name };
    let at = -1;
    for (let i = 0; i < out.length; i++) {
      const o = out[i]!;
      if (o.path === l.path && o.line <= l.line) at = i;
    }
    if (at < 0) out.push(entry);
    else {
      let j = at + 1;
      while (j < out.length && out[j]!.level === LABEL_LEVEL && out[j]!.path === l.path && out[j]!.line <= l.line) j++;
      out.splice(j, 0, entry);
    }
  }
  return out;
}

export function createSidebar(ws: Workspace, opts: { onOpen(path: string): void }): { el: HTMLElement; destroy(): void } {
  const collapsed = new Set<string>();
  let showLabels = false;

  const segFiles = h("button", { class: "seg", text: t("Files"), attrs: { role: "tab" } });
  const segOutline = h("button", { class: "seg", text: t("Outline"), attrs: { role: "tab" } });
  const segmented = h("div", { class: "segmented", attrs: { role: "tablist", "aria-label": t("Sidebar") } }, segFiles, segOutline);
  const tree = h("div", { class: "file-tree", attrs: { role: "tree", tabindex: 0 } });
  const outline = h("div", { class: "outline", attrs: { role: "tree" } });
  const labelsToggle = h("button", { class: "outline-labels-toggle glass-capsule", title: t("Show \\label entries in the outline") }, icon("tag", 12), h("span", { text: t("Labels") }));
  const outlineWrap = h("div", { class: "outline-wrap" }, h("div", { class: "outline-tools" }, labelsToggle), outline);
  const body = h("div", { class: "sidebar-body" }, tree, outlineWrap);
  const el = h("aside", { class: "sidebar", attrs: { "aria-label": t("Sidebar") } }, h("div", { class: "sidebar-top" }, segmented), body);

  const setMode = (mode: "files" | "outline") => {
    ws.sidebarMode = mode;
    renderMode();
  };
  segFiles.addEventListener("click", () => setMode("files"));
  segOutline.addEventListener("click", () => setMode("outline"));
  labelsToggle.addEventListener("click", () => { showLabels = !showLabels; renderOutline(); });

  function renderMode(): void {
    const files = ws.sidebarMode === "files";
    toggleClass(segFiles, "selected", files);
    toggleClass(segOutline, "selected", !files);
    segFiles.setAttribute("aria-selected", String(files));
    segOutline.setAttribute("aria-selected", String(!files));
    segmented.dataset.index = files ? "0" : "1";
    tree.hidden = !files;
    outlineWrap.hidden = files;
    if (!files) renderOutline();
  }

  // ---- files -------------------------------------------------------------------------------------------------

  const rowByPath = new Map<string, HTMLElement>();

  function renderTree(): void {
    rowByPath.clear();
    tree.textContent = "";
    if (!ws.files.length) {
      tree.appendChild(h("div", { class: "empty-state small" },
        icon("folder", 26), h("div", { class: "empty-title", text: t("Empty Folder") }),
        h("div", { class: "empty-detail", text: t("Create a file to get started.") }),
        h("button", { class: "btn glass small", text: t("New File…"), on: { click: () => void newFile(ws.root) } })));
      return;
    }
    const frag = document.createDocumentFragment();
    const walk = (nodes: FileNode[], depth: number) => {
      for (const n of nodes) {
        const open = !collapsed.has(n.path);
        const row = h("div", {
          class: "tree-row" + (n.isDir ? " dir" : ""), title: ws.relative(n.path),
          attrs: { role: "treeitem", "aria-expanded": n.isDir ? String(open) : undefined, tabindex: -1 },
          style: { paddingLeft: `${10 + depth * 16}px` },
        },
        n.isDir ? h("span", { class: "disclosure" + (open ? " open" : "") }, icon("chevronRight", 11)) : h("span", { class: "disclosure" }),
        icon(n.isDir ? "folder" : fileIconName(n.name), 16, n.isDir ? "tree-icon folder" : "tree-icon"),
        h("span", { class: "tree-name", text: n.name }),
        n.isDir ? null : h("span", { class: "tree-trail" }));
        row.addEventListener("click", () => {
          if (n.isDir) {
            if (open) collapsed.add(n.path); else collapsed.delete(n.path);
            renderTree();
          } else opts.onOpen(n.path);
        });
        row.addEventListener("contextmenu", (e) => { e.preventDefault(); contextMenu(n, e); });
        rowByPath.set(n.path, row);
        frag.appendChild(row);
        if (n.isDir && open) walk(n.children ?? [], depth + 1);
      }
    };
    walk(ws.files, 0);
    tree.appendChild(frag);
    renderMarks();
  }

  /** Selection, main-document star and unsaved dots (cheap; runs on tab / dirty / main changes). */
  function renderMarks(): void {
    for (const [path, row] of rowByPath) {
      toggleClass(row, "selected", path === ws.current);
      const trail = row.querySelector(".tree-trail");
      if (!trail) continue;
      const main = ws.isMainFile(path);
      const dirty = ws.isDirty(path);
      const key = `${main ? "m" : ""}${dirty ? "d" : ""}`;
      if ((trail as HTMLElement).dataset.key === key) continue;
      (trail as HTMLElement).dataset.key = key;
      trail.textContent = "";
      if (dirty) trail.appendChild(h("span", { class: "dirty-dot", title: t("Unsaved changes"), attrs: { "aria-label": t("Unsaved changes") } }));
      if (main) trail.appendChild(h("span", { class: "main-star", title: t("Main document"), attrs: { "aria-label": t("Main document") } }, icon("starFill", 13)));
    }
  }

  tree.addEventListener("contextmenu", (e) => {
    if (e.target !== tree) return;
    e.preventDefault();
    popupMenu([
      { title: t("New File…"), icon: "docPlus", action: () => void newFile(ws.root) },
      { kind: "separator" },
      { title: t("Reveal in File Manager"), action: () => ws.reveal(ws.root) },
    ], { x: e.clientX, y: e.clientY });
  });

  async function newFile(dir: string): Promise<void> {
    const name = await promptDialog({
      title: t("New File"), message: t("Enter a name for the new file. “.tex” is added if you leave out an extension."),
      label: t("Name"), confirm: t("Create"),
    });
    if (name) await ws.createFile(name, dir);
  }

  function contextMenu(n: FileNode, e: MouseEvent): void {
    const dir = n.isDir ? n.path : n.path.slice(0, Math.max(n.path.lastIndexOf("/"), n.path.lastIndexOf("\\")));
    const items: MenuItem[] = [{ title: t("New File…"), icon: "docPlus", action: () => void newFile(dir) }, { kind: "separator" }];
    if (!n.isDir) {
      items.push({ title: t("Open"), action: () => opts.onOpen(n.path) });
      if (/\.(tex|ltx)$/i.test(n.name)) {
        items.push(ws.mainOverride === n.path
          ? { title: t("Use Automatic Main Document"), icon: "star", action: () => ws.setMainFile(undefined) }
          : { title: t("Set as Main Document"), icon: "star", action: () => ws.setMainFile(n.path) });
      }
      items.push({ kind: "separator" });
    }
    items.push(
      { title: t("Reveal in File Manager"), action: () => ws.reveal(n.path) },
      { title: t("Copy Path"), icon: "copy", action: () => void navigator.clipboard.writeText(n.path).then(() => toast(t("Copy Path"))).catch(() => {}) },
    );
    popupMenu(items, { x: e.clientX, y: e.clientY });
  }

  // ---- outline -----------------------------------------------------------------------------------------------

  function renderOutline(): void {
    if (ws.sidebarMode !== "outline") return;
    toggleClass(labelsToggle, "on", showLabels);
    labelsToggle.setAttribute("aria-pressed", String(showLabels));
    const items = withLabels(ws.index?.outline ?? [], showLabels ? ws.index?.labels ?? [] : []);
    outline.textContent = "";
    if (!items.length) {
      outline.appendChild(h("div", { class: "empty-state small" }, icon("listIndent", 26),
        h("div", { class: "empty-title", text: t("No Outline") }),
        h("div", { class: "empty-detail", text: t("Sections, frames and labels of your document appear here.") })));
      return;
    }
    const minLevel = Math.min(...items.filter((o) => !isLabelEntry(o)).map((o) => o.level), 1);
    const frag = document.createDocumentFragment();
    for (const o of items) {
      const label = isLabelEntry(o);
      const depth = label ? 0 : Math.max(0, o.level - minLevel);
      const row = h("button", {
        class: "outline-row" + (label ? " label" : "") + (o.level <= 0 && !label ? " major" : ""),
        title: t("%@, line %lld", ws.relative(o.path), o.line), style: { paddingLeft: `${12 + depth * 14}px` },
        attrs: { role: "treeitem" },
      }, label ? icon("tag", 12) : null, h("span", { class: "outline-title", text: label ? o.label ?? o.title : o.title || t("Untitled") }));
      row.addEventListener("click", () => void ws.open(o.path, { line: o.line, flash: true }).then(() => ws.editor?.focus()));
      frag.appendChild(row);
    }
    outline.appendChild(frag);
    const words = ws.index?.stats.words;
    if (words !== undefined) outline.appendChild(h("div", { class: "outline-footer", text: t("%@ words", formatInt(words)) }));
  }

  const isLabelEntry = (o: OutlineItem) => o.level === LABEL_LEVEL;

  const offs = [
    ws.events.on("files", renderTree),
    ws.events.on(["tabs", "dirty", "main"], renderMarks),
    ws.events.on("index", renderOutline),
  ];
  renderTree();
  renderMode();
  return { el, destroy: () => offs.forEach((f) => f()) };
}

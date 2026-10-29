// The sidebar (port of SidebarView / FileTreeView / OutlineSidebarView): a translucent glass column with a
// Files | Outline segmented control, the file tree (main document star, unsaved dot, context menu) and the outline.
import * as ipc from "../ipc";
import type { FileNode, Label, OutlineItem } from "../ipc";
import { t, formatInt } from "../i18n";
import type { Workspace } from "../app/workspace";
import { h, toggleClass, basename, dirname } from "./dom";
import { icon, fileIconName } from "./icons";
import { popupMenu, promptDialog, type MenuItem } from "./overlay";

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
  const nodeByPath = new Map<string, FileNode>();
  /** The row the keyboard acts on (F2, Delete); falls back to the open file. */
  let selected: string | undefined;
  /** Path being renamed inline, and a tree refresh that arrived meanwhile. */
  let renaming: string | undefined;
  let staleTree = false;

  function renderTree(): void {
    if (renaming) { staleTree = true; return; }
    rowByPath.clear();
    nodeByPath.clear();
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
          selected = n.path;
          renderMarks();
          if (n.isDir) {
            if (open) collapsed.add(n.path); else collapsed.delete(n.path);
            renderTree();
          } else opts.onOpen(n.path);
        });
        row.addEventListener("contextmenu", (e) => { e.preventDefault(); selected = n.path; renderMarks(); contextMenu(n, e); });
        rowByPath.set(n.path, row);
        nodeByPath.set(n.path, n);
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
      toggleClass(row, "selected", path === (selected ?? ws.current));
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
      { title: t("New File…"), action: () => void newFile(ws.root) },
      { title: t("New Folder…"), action: () => void newFolder(ws.root) },
      { kind: "separator" },
      { title: t("Clean Auxiliary Files"), action: () => void ws.cleanAuxiliaryFiles() },
      { title: t("Reveal in File Manager"), action: () => ws.reveal(ws.root) },
    ], { x: e.clientX, y: e.clientY });
  });

  tree.addEventListener("keydown", (e) => {
    if (e.target instanceof HTMLInputElement || e.ctrlKey || e.metaKey || e.altKey) return;
    const path = selected ?? ws.current;
    const node = path ? nodeByPath.get(path) : undefined;
    if (!node) return;
    if (e.key === "F2") { e.preventDefault(); startRename(node); }
    else if (e.key === "Delete") { e.preventDefault(); void ws.trashItem(node.path, { confirm: true }); }
    else if (e.key === "Enter" && !node.isDir) { e.preventDefault(); opts.onOpen(node.path); }
  });

  async function newFile(dir: string): Promise<void> {
    const name = await promptDialog({
      title: t("New File"), message: t("Enter a name for the new file. “.tex” is added if you leave out an extension."),
      label: t("Name"), confirm: t("Create"),
    });
    if (name) await ws.createFile(name, dir);
  }

  async function newFolder(dir: string): Promise<void> {
    const name = await promptDialog({ title: t("New Folder"), message: t("Enter a name for the new folder."), label: t("Name"), confirm: t("Create") });
    if (name) await ws.createFolder(name, dir);
  }

  /** Swaps the row's name for a text field (Return commits, Escape cancels, leaving the field commits). */
  function startRename(n: FileNode): void {
    const row = rowByPath.get(n.path);
    const label = row?.querySelector<HTMLElement>(".tree-name");
    if (!row || !label || renaming) return;
    renaming = n.path;
    const input = h("input", { class: "tree-rename", value: n.name, attrs: { type: "text", "aria-label": t("Rename “%@”", n.name), spellcheck: "false" } });
    label.replaceWith(input);
    let done = false;
    const finish = (commit: boolean) => {
      if (done) return;
      done = true;
      renaming = undefined;
      const value = input.value;
      const refresh = () => { if (staleTree) { staleTree = false; renderTree(); } tree.focus({ preventScroll: true }); };
      if (!commit || value.trim() === n.name) { input.replaceWith(label); refresh(); return; }
      void ws.renameItem(n.path, value).then((ok) => {
        if (selected === n.path && ok) selected = n.path.slice(0, n.path.length - basename(n.path).length) + value.trim();
        if (!ok) input.replaceWith(label);
        staleTree = false;
        renderTree();
        tree.focus({ preventScroll: true });
      });
    };
    input.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") { e.preventDefault(); finish(true); }
      else if (e.key === "Escape") { e.preventDefault(); finish(false); }
    });
    input.addEventListener("click", (e) => e.stopPropagation());
    input.addEventListener("blur", () => finish(true));
    input.focus();
    const dot = n.isDir ? -1 : n.name.lastIndexOf(".");
    if (dot > 0) input.setSelectionRange(0, dot); else input.select();
  }

  function contextMenu(n: FileNode, e: MouseEvent): void {
    const dir = n.isDir ? n.path : dirname(n.path);
    const items: MenuItem[] = [
      { title: t("New File…"), action: () => void newFile(dir) },
      { title: t("New Folder…"), action: () => void newFolder(dir) },
      { kind: "separator" },
    ];
    if (!n.isDir) items.push({ title: t("Open"), action: () => opts.onOpen(n.path) });
    items.push(
      { title: t("Rename…"), shortcut: "F2", action: () => startRename(n) },
      { title: t("Duplicate"), action: () => void ws.duplicateItem(n.path, n.isDir) },
      { kind: "separator" },
    );
    if (!n.isDir && /\.(tex|ltx)$/i.test(n.name)) {
      items.push(ws.mainOverride === n.path
        ? { title: t("Use Automatic Main Document"), action: () => ws.setMainFile(undefined) }
        : { title: t("Set as Main Document"), action: () => ws.setMainFile(n.path) });
    }
    if (!n.isDir) items.push({ title: t("Open in Default App"), action: () => void ipc.openInDefaultApp(n.path).catch(() => ws.flash(t("Could not open “%@”", n.name))) });
    items.push(
      { title: t("Reveal in File Manager"), action: () => ws.reveal(n.path) },
      { title: t("Copy Path"), action: () => ws.copyPath(n.path) },
      { kind: "separator" },
      { title: t("Move to Trash"), destructive: true, shortcut: "Del", action: () => void ws.trashItem(n.path) },
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
    ws.events.on("tabs", () => { selected = undefined; }),
    ws.events.on(["tabs", "dirty", "main"], renderMarks),
    ws.events.on("index", renderOutline),
  ];
  renderTree();
  renderMode();
  return { el, destroy: () => offs.forEach((f) => f()) };
}

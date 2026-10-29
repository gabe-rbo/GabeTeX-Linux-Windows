// The command catalog (port of CommandRegistry + CommandRegistry+Catalog.swift): every menu command plus the editor
// and markup actions, with the Mac defaults translated (⌘ → Ctrl, ⌥ → Alt). Commands run against a `CommandHost`
// (the open project window, or the Welcome window when no project is open).
import type { Engine } from "../ipc";
import { t } from "../i18n";
import { shortcuts, type Combo } from "./shortcuts";

export type Category = "file" | "edit" | "insert" | "view" | "build" | "window" | "app";

export const CATEGORY_TITLES: Record<Category, string> = {
  file: "File", edit: "Edit", insert: "Insert", view: "View", build: "Build", window: "Window", app: "GabeTeX",
};

/** Editor-native actions: CodeMirror owns their default key; the palette replays that key in the editor. */
export type EditorAction = "find" | "replace" | "toggleComment" | "indent" | "outdent" | "complete";

export interface CommandHost {
  hasProject(): boolean;
  hasFile(): boolean;
  tabCount(): number;
  canCompile(): boolean;
  isCompiling(): boolean;
  engineOverride(): Engine | undefined;
  hasMainOverride(): boolean;
  hasPDF(): boolean;
  showPreview(): boolean;
  isPdfOnly(): boolean;
  showIssues(): boolean;

  newProject(): void;
  newFile(): void;
  openFolder(): void;
  openFile(): void;
  closeTab(): void;
  save(): void;
  saveAll(): void;
  revealCurrent(): void;
  newProjectLabel(): void;
  editorAction(action: EditorAction): void;
  goToLine(): void;
  insertSnippet(snippet: string): void;
  togglePreview(): void;
  togglePdfOnly(): void;
  toggleIssues(): void;
  quickOpen(): void;
  commandPalette(): void;
  showSidebar(mode: "files" | "outline"): void;
  documentStatistics(): void;
  cycleTab(delta: number): void;
  selectTab(index: number): void;
  compile(): void;
  stop(): void;
  setEngine(engine: Engine | undefined): void;
  setMainDocument(): void;
  automaticMainDocument(): void;
  cleanAuxiliaryFiles(): void;
  showInPDF(): void;
  revealPDF(): void;
  nextIssue(forward: boolean): void;
  welcome(): void;
  openSettings(pane?: string): void;
  // Separate windows (PDF window, presentation)
  canDetachPdf(): boolean;
  isPdfDetached(): boolean;
  togglePdfWindow(): void;
  canPresent(): boolean;
  isPresenting(): boolean;
  togglePresentation(): void;
}

export interface Command {
  id: string;
  title: string;
  category: Category;
  symbol: string;
  defaultShortcut?: Combo;
  /** False for shortcuts the system owns (Close Tab, Close Window, Settings). */
  customizable: boolean;
  showsInPalette: boolean;
  showsInSettings: boolean;
  keywords: string[];
  /** CodeMirror handles the default key itself (see `EditorAction`). */
  editorAction?: EditorAction;
  dynamicTitle?: (host: CommandHost) => string;
  isEnabled: (host: CommandHost) => boolean;
  isOn?: (host: CommandHost) => boolean;
  run: (host: CommandHost) => void;
}

type Spec = Partial<Command> & Pick<Command, "id" | "title" | "category" | "symbol" | "run">;

const always = () => true;
const hasProject = (h: CommandHost) => h.hasProject();
const hasFile = (h: CommandHost) => h.hasFile();

function make(s: Spec): Command {
  return {
    customizable: true, showsInPalette: true, showsInSettings: true, keywords: [], isEnabled: always, ...s,
  };
}

export const ENGINES: Array<{ id: Engine; name: string }> = [
  { id: "pdflatex", name: "pdfLaTeX" },
  { id: "xelatex", name: "XeLaTeX" },
  { id: "lualatex", name: "LuaLaTeX" },
];
export const engineName = (e: Engine): string => ENGINES.find((x) => x.id === e)?.name ?? e;

const ENVIRONMENTS = ["theorem", "lemma", "proposition", "corollary", "definition", "remark", "example", "proof", "itemize", "enumerate", "figure", "table"];
const SECTIONS = ["section", "subsection", "subsubsection", "paragraph"];
const DISPLAY_MATH: Array<{ key: string; label: string; snippet: string }> = [
  { key: "brackets", label: "\\[ \\]", snippet: "\\[\n  ${1}\n\\]" },
  { key: "equation", label: "equation", snippet: "\\begin{equation}\n  ${1}\n\\end{equation}" },
  { key: "equationStar", label: "equation*", snippet: "\\begin{equation*}\n  ${1}\n\\end{equation*}" },
  { key: "align", label: "align", snippet: "\\begin{align}\n  ${1}\n\\end{align}" },
  { key: "alignStar", label: "align*", snippet: "\\begin{align*}\n  ${1}\n\\end{align*}" },
];

function environmentSnippet(name: string): string {
  switch (name) {
    case "itemize": case "enumerate": return `\\begin{${name}}\n  \\item \${1}\n\\end{${name}}`;
    case "figure": return "\\begin{figure}[htbp]\n  \\centering\n  ${1}\n  \\caption{${2}}\n  \\label{fig:${3}}\n\\end{figure}";
    case "table": return "\\begin{table}[htbp]\n  \\centering\n  ${1}\n  \\caption{${2}}\n  \\label{tab:${3}}\n\\end{table}";
    default: return `\\begin{${name}}\n  \${1}\n\\end{${name}}`;
  }
}

function catalog(): Command[] {
  const c: Command[] = [];
  const insert = (id: string, title: string, symbol: string, snippet: string, keywords: string[] = []) =>
    c.push(make({ id, title, category: "insert", symbol, keywords, isEnabled: hasFile, run: (h) => h.insertSnippet(snippet) }));

  // File
  c.push(
    make({ id: "file.newProject", title: t("New Project…"), category: "file", symbol: "plus.rectangle.on.folder", defaultShortcut: "Ctrl+Shift+N", run: (h) => h.newProject() }),
    make({ id: "file.newFile", title: t("New File…"), category: "file", symbol: "doc.badge.plus", defaultShortcut: "Ctrl+N", isEnabled: hasProject, run: (h) => h.newFile() }),
    make({ id: "file.openFolder", title: t("Open Folder…"), category: "file", symbol: "folder", defaultShortcut: "Ctrl+O", run: (h) => h.openFolder() }),
    make({ id: "file.openFile", title: t("Open File…"), category: "file", symbol: "doc", defaultShortcut: "Ctrl+Alt+O", run: (h) => h.openFile() }),
    make({ id: "file.closeTab", title: t("Close Tab"), category: "file", symbol: "xmark", defaultShortcut: "Ctrl+W", customizable: false, isEnabled: (h) => h.tabCount() > 0, run: (h) => h.closeTab() }),
    make({ id: "file.save", title: t("Save"), category: "file", symbol: "square.and.arrow.down", defaultShortcut: "Ctrl+S", isEnabled: hasFile, run: (h) => h.save() }),
    make({ id: "file.saveAll", title: t("Save All"), category: "file", symbol: "square.and.arrow.down.on.square", defaultShortcut: "Ctrl+Alt+S", isEnabled: hasProject, run: (h) => h.saveAll() }),
    make({ id: "file.revealInFinder", title: t("Reveal in File Manager"), category: "file", symbol: "folder.badge.questionmark", isEnabled: hasFile, run: (h) => h.revealCurrent() }),
    make({ id: "file.newProjectLabel", title: t("New Project Label…"), category: "file", symbol: "tag", keywords: [t("Project Labels")], isEnabled: hasProject, run: (h) => h.newProjectLabel() }),
  );

  // Edit (CodeMirror owns the default keys of these)
  const edit = (id: string, title: string, symbol: string, combo: Combo, action: EditorAction, keywords: string[] = []) =>
    c.push(make({ id, title, category: "edit", symbol, defaultShortcut: combo, editorAction: action, keywords, isEnabled: hasFile, run: (h) => h.editorAction(action) }));
  edit("edit.find", t("Find…"), "magnifyingglass", "Ctrl+F", "find");
  edit("edit.findAndReplace", t("Find and Replace…"), "text.magnifyingglass", "Ctrl+H", "replace");
  c.push(make({ id: "edit.goToLine", title: t("Go to Line…"), category: "edit", symbol: "arrow.right.to.line", defaultShortcut: "Ctrl+L", isEnabled: hasFile, run: (h) => h.goToLine() }));
  edit("edit.toggleComment", t("Toggle Comment"), "percent", "Ctrl+/", "toggleComment");
  edit("edit.indent", t("Indent"), "increase.indent", "Ctrl+]", "indent");
  edit("edit.outdent", t("Outdent"), "decrease.indent", "Ctrl+[", "outdent");
  edit("edit.complete", t("Complete"), "text.badge.plus", "Ctrl+Space", "complete", [t("Autocomplete")]);

  // Insert
  insert("insert.inlineMath", t("Inline Math"), "dollarsign", "$${1}$");
  for (const d of DISPLAY_MATH) insert(`insert.displayMath.${d.key}`, t("Display Math (%@)", d.label), "function", d.snippet, [t("Display Math")]);
  insert("insert.fraction", t("Fraction"), "divide", "\\frac{${1}}{${2}}", ["\\frac"]);
  insert("insert.superscript", t("Superscript"), "textformat.superscript", "^{${1}}");
  insert("insert.subscript", t("Subscript"), "textformat.subscript", "_{${1}}");
  insert("insert.bold", t("Bold"), "bold", "\\textbf{${1}}", ["\\textbf"]);
  insert("insert.italic", t("Italic"), "italic", "\\textit{${1}}", ["\\textit"]);
  insert("insert.emphasis", t("Emphasis"), "textformat", "\\emph{${1}}", ["\\emph"]);
  for (const env of ENVIRONMENTS) insert(`insert.environment.${env}`, t("Environment (%@)", env), "curlybraces", environmentSnippet(env), [t("Environment"), "\\begin"]);
  for (const s of SECTIONS) insert(`insert.section.${s}`, `\\${s}{…}`, "number", `\\${s}{\${1}}`, [t("Section")]);

  // View
  c.push(
    make({
      id: "view.togglePreview", title: t("Show or Hide PDF Preview"), category: "view", symbol: "doc.richtext", defaultShortcut: "Ctrl+Alt+P",
      dynamicTitle: (h) => (h.isPdfDetached() ? t("Show PDF Window") : h.showPreview() ? t("Hide PDF Preview") : t("Show PDF Preview")), isEnabled: hasProject, run: (h) => h.togglePreview(),
    }),
    make({
      id: "view.togglePDFWindow", title: t("Open or Attach PDF Window"), category: "view", symbol: "arrow.up.forward.app",
      // Without separate windows (browser mock) this is PDF Only.
      dynamicTitle: (h) => (h.canDetachPdf()
        ? (h.isPdfDetached() ? t("Attach PDF to Project Window") : t("Open PDF in Separate Window"))
        : (h.isPdfOnly() ? t("Show the editor again") : t("Maximize the PDF preview"))),
      isEnabled: hasProject, isOn: (h) => (h.canDetachPdf() ? h.isPdfDetached() : h.isPdfOnly()), run: (h) => h.togglePdfWindow(),
    }),
    make({
      id: "view.toggleIssues", title: t("Show or Hide Issues"), category: "view", symbol: "exclamationmark.triangle", defaultShortcut: "Ctrl+Shift+M",
      dynamicTitle: (h) => (h.showIssues() ? t("Hide Issues") : t("Show Issues")), isEnabled: hasProject, run: (h) => h.toggleIssues(),
    }),
    make({ id: "view.quickOpen", title: t("Quick Open…"), category: "view", symbol: "doc.text.magnifyingglass", defaultShortcut: "Ctrl+P", isEnabled: hasProject, run: (h) => h.quickOpen() }),
    make({ id: "view.commandPalette", title: t("Command Palette…"), category: "view", symbol: "command", defaultShortcut: "Ctrl+Shift+P", showsInPalette: false, run: (h) => h.commandPalette() }),
    make({ id: "view.showFiles", title: t("Show Files"), category: "view", symbol: "folder", defaultShortcut: "Ctrl+Alt+1", isEnabled: hasProject, run: (h) => h.showSidebar("files") }),
    make({ id: "view.showOutline", title: t("Show Outline"), category: "view", symbol: "list.bullet.indent", defaultShortcut: "Ctrl+Alt+2", isEnabled: hasProject, run: (h) => h.showSidebar("outline") }),
    make({ id: "view.documentStatistics", title: t("Document Statistics…"), category: "view", symbol: "chart.bar.doc.horizontal", keywords: [t("Word Count"), t("Pages")], isEnabled: hasProject, run: (h) => h.documentStatistics() }),
    make({
      id: "view.presentation", title: t("Start Presentation"), category: "view", symbol: "play.rectangle", defaultShortcut: "Ctrl+Alt+Enter",
      keywords: ["Beamer", t("Slides")], dynamicTitle: (h) => (h.isPresenting() ? t("End Presentation") : t("Start Presentation")),
      isEnabled: (h) => h.isPresenting() || h.canPresent(), run: (h) => h.togglePresentation(),
    }),
    make({ id: "view.nextTab", title: t("Show Next Tab"), category: "view", symbol: "arrow.right.square", defaultShortcut: "Ctrl+Tab", isEnabled: (h) => h.tabCount() >= 2, run: (h) => h.cycleTab(1) }),
    make({ id: "view.previousTab", title: t("Show Previous Tab"), category: "view", symbol: "arrow.left.square", defaultShortcut: "Ctrl+Shift+Tab", isEnabled: (h) => h.tabCount() >= 2, run: (h) => h.cycleTab(-1) }),
  );
  for (let n = 1; n <= 9; n++) {
    c.push(make({ id: `view.tab.${n}`, title: t("Go to Tab %lld", n), category: "view", symbol: `${n}.square`, defaultShortcut: `Ctrl+${n}`, isEnabled: (h) => h.tabCount() >= n, run: (h) => h.selectTab(n - 1) }));
  }

  // Build
  c.push(
    make({ id: "build.compile", title: t("Compile"), category: "build", symbol: "play.fill", defaultShortcut: "Ctrl+B", isEnabled: (h) => h.canCompile(), run: (h) => h.compile() }),
    make({ id: "build.stop", title: t("Stop"), category: "build", symbol: "stop.fill", defaultShortcut: "Ctrl+.", isEnabled: (h) => h.isCompiling(), run: (h) => h.stop() }),
    make({ id: "build.liveCompile", title: t("Live Compile"), category: "build", symbol: "bolt", isOn: () => liveCompileState(), run: () => toggleLiveCompile() }),
    make({
      id: "build.engine.automatic", title: t("Engine: Automatic"), category: "build", symbol: "gearshape.2", isEnabled: hasProject,
      isOn: (h) => h.engineOverride() === undefined, run: (h) => h.setEngine(undefined),
    }),
  );
  for (const e of ENGINES) {
    c.push(make({ id: `build.engine.${e.id}`, title: t("Engine: %@", e.name), category: "build", symbol: "gearshape.2", isEnabled: hasProject, isOn: (h) => h.engineOverride() === e.id, run: (h) => h.setEngine(e.id) }));
  }
  c.push(
    make({ id: "build.setMainDocument", title: t("Set as Main Document"), category: "build", symbol: "star", isEnabled: hasFile, run: (h) => h.setMainDocument() }),
    make({ id: "build.automaticMainDocument", title: t("Use Automatic Main Document"), category: "build", symbol: "star.slash", isEnabled: (h) => h.hasMainOverride(), run: (h) => h.automaticMainDocument() }),
    make({ id: "build.showInPDF", title: t("Show in PDF"), category: "build", symbol: "text.viewfinder", defaultShortcut: "Ctrl+Shift+J", keywords: ["SyncTeX"], isEnabled: hasFile, run: (h) => h.showInPDF() }),
    make({ id: "build.cleanAuxiliaryFiles", title: t("Clean Auxiliary Files"), category: "build", symbol: "trash", defaultShortcut: "Ctrl+Shift+K", isEnabled: hasProject, run: (h) => h.cleanAuxiliaryFiles() }),
    make({ id: "build.revealPDF", title: t("Show PDF in File Manager"), category: "build", symbol: "doc.viewfinder", isEnabled: (h) => h.hasPDF(), run: (h) => h.revealPDF() }),
    make({ id: "build.nextIssue", title: t("Next Issue"), category: "build", symbol: "chevron.down.circle", defaultShortcut: "Ctrl+'", isEnabled: hasProject, run: (h) => h.nextIssue(true) }),
    make({ id: "build.previousIssue", title: t("Previous Issue"), category: "build", symbol: "chevron.up.circle", defaultShortcut: "Ctrl+Shift+'", isEnabled: hasProject, run: (h) => h.nextIssue(false) }),
  );

  // Window and app
  c.push(
    make({ id: "window.welcome", title: t("Welcome to GabeTeX"), category: "window", symbol: "house", run: (h) => h.welcome() }),
    make({ id: "app.keyboardShortcuts", title: t("Keyboard Shortcuts…"), category: "app", symbol: "keyboard", keywords: [t("Settings")], run: (h) => h.openSettings("shortcuts") }),
    make({ id: "app.settings", title: t("Settings…"), category: "app", symbol: "gearshape", defaultShortcut: "Ctrl+,", customizable: false, showsInSettings: false, run: (h) => h.openSettings() }),
  );
  return c;
}

// Live compile is an app setting; set by the shell at start so commands stay free of UI imports.
let liveCompileState: () => boolean = () => false;
let toggleLiveCompile: () => void = () => {};
export function bindLiveCompile(get: () => boolean, toggle: () => void): void {
  liveCompileState = get;
  toggleLiveCompile = toggle;
}

let all: Command[] | null = null;
const byId = new Map<string, Command>();

/** The catalog in menu order (built once, after i18n is ready). */
export function commands(): Command[] {
  if (!all) {
    all = catalog();
    for (const cmd of all) byId.set(cmd.id, cmd);
    shortcuts.register(all.map((cmd) => ({ id: cmd.id, defaultCombo: cmd.defaultShortcut, customizable: cmd.customizable })));
  }
  return all;
}

export function command(id: string): Command | undefined {
  commands();
  return byId.get(id);
}

export const commandTitle = (cmd: Command, host: CommandHost): string => cmd.dynamicTitle?.(host) ?? cmd.title;

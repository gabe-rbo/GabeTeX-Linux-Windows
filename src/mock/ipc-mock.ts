// Browser-only fake backend. Loaded by main.ts ONLY when `window.__TAURI_INTERNALS__` is absent and the build mode
// is `development` or `mock`, so production builds never contain it (the import sits behind a constant condition).
// It answers every command in src/ipc.ts with a realistic multi-file project (the Homologia example), compiles to
// a small but real two-page PDF built here as bytes, and returns plausible diagnostics and SyncTeX boxes.
import { mockIPC } from "@tauri-apps/api/mocks";
import type {
  BibEntry, CompileResult, CompletionItem, CompletionResponse, Diagnostic, FileNode, Label, OutlineItem, ProjectIndex,
  ProjectInfo, RecentProject, Settings, SyncBox,
} from "../ipc";

const DEMO_DIR = "/home/demo/GabeTeX Demo";
const MAIN = String.raw`\documentclass[11pt]{article}
\usepackage{amsmath,amssymb,amsthm,mathtools}
\usepackage{tikz-cd}
\usepackage[hidelinks]{hyperref}
\usepackage{cleveref}

\theoremstyle{plain}
\newtheorem{theorem}{Teorema}[section]
\newtheorem{lemma}[theorem]{Lema}
\theoremstyle{definition}
\newtheorem{definition}[theorem]{Definição}

\newcommand{\Z}{\mathbb{Z}}
\DeclareMathOperator{\im}{im}

\title{Notas de Homologia Simplicial}
\author{Gabriel Ribeiro}

\begin{document}
\maketitle

\input{sections/introducao}
\input{sections/homologia}

\bibliographystyle{plain}
\bibliography{refs}
\end{document}
`;
const HOMOLOGIA = String.raw`% !TEX root = ../main.tex
\section{Homologia}\label{sec:homologia}

\begin{definition}\label{def:bordo}
Seja $K$ um complexo simplicial. O operador bordo $\partial_n \colon C_n(K) \to C_{n-1}(K)$ é
\begin{equation}\label{eq:bordo}
  \partial_n [v_0, \dots, v_n] = \sum_{i=0}^{n} (-1)^i [v_0, \dots, \hat{v}_i, \dots, v_n].
\end{equation}
\end{definition}

\begin{lemma}\label{lem:dd}
$\partial_{n-1} \circ \partial_n = 0$.
\end{lemma}

\begin{proof}
Segue de \eqref{eq:bordo}: cada face de codimensão dois aparece duas vezes com sinais opostos.
\end{proof}

\begin{theorem}
$H_n(K) = \ker \partial_n / \im \partial_{n+1}$ é um $\Z$-módulo finitamente gerado.
\end{theorem}

\[
\begin{tikzcd}
  C_{n+1} \arrow[r, "\partial_{n+1}"] \arrow[d] & C_n \arrow[r, "\partial_n"] \arrow[d] & C_{n-1} \arrow[d] \\
  D_{n+1} \arrow[r] & D_n \arrow[r] & D_{n-1}
\end{tikzcd}
\]
`;
const INTRO = String.raw`% !TEX root = ../main.tex
\section{Introdução}\label{sec:intro}

Estas notas seguem \cite{hatcher2002} e \cite{munkres1984}. Na \cref{sec:homologia}
definimos os grupos de homologia e calculamos a forma normal de Smith.
`;
const BIB = `@book{hatcher2002,
  author    = {Allen Hatcher},
  title     = {Algebraic Topology},
  publisher = {Cambridge University Press},
  year      = {2002}
}

@book{munkres1984,
  author    = {James R. Munkres},
  title     = {Elements of Algebraic Topology},
  publisher = {Addison-Wesley},
  year      = {1984}
}
`;

// ---- in-memory file system ------------------------------------------------------------------------------------

const files = new Map<string, string>();
const projectRoots = new Set<string>();

function seedProject(root: string): void {
  if (projectRoots.has(root)) return;
  projectRoots.add(root);
  files.set(`${root}/main.tex`, MAIN);
  files.set(`${root}/sections/homologia.tex`, HOMOLOGIA);
  files.set(`${root}/sections/introducao.tex`, INTRO);
  files.set(`${root}/refs.bib`, BIB);
  // A long file for typing-latency measurements (?big in the URL opens it).
  const big: string[] = ["% !TEX root = main.tex", "\\section{Long}"];
  for (let i = 0; big.length < 5000; i++) {
    big.push(`Paragraph ${i}: seja $f_${i} \\colon X \\to Y$ contínua; então \\ref{eq:bordo} vale e $\\sum_{k=0}^{n} a_k = ${i}$.`);
    if (i % 25 === 0) big.push(`\\subsection{Part ${i}}\\label{sub:${i}}`);
  }
  files.set(`${root}/sections/longo.tex`, big.join("\n") + "\n");
}

const rootOf = (path: string) => [...projectRoots].find((r) => path === r || path.startsWith(r + "/"));
const nameOf = (path: string) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;

function tree(root: string): FileNode[] {
  const top: FileNode[] = [];
  const dirs = new Map<string, FileNode>();
  const paths = [...files.keys()].filter((p) => p.startsWith(root + "/")).sort();
  for (const path of paths) {
    const parts = path.slice(root.length + 1).split("/");
    let list = top;
    let acc = root;
    for (let i = 0; i < parts.length - 1; i++) {
      acc += "/" + parts[i];
      let d = dirs.get(acc);
      if (!d) { d = { name: parts[i]!, path: acc, isDir: true, children: [] }; dirs.set(acc, d); list.push(d); }
      list = d.children!;
    }
    list.push({ name: parts[parts.length - 1]!, path, isDir: false });
  }
  const sort = (l: FileNode[]) => { l.sort((a, b) => (a.isDir === b.isDir ? a.name.localeCompare(b.name) : a.isDir ? -1 : 1)); l.forEach((n) => n.children && sort(n.children)); };
  sort(top);
  return top;
}

function info(root: string): ProjectInfo {
  seedProject(root);
  return { root, name: nameOf(root), files: tree(root), rootFile: `${root}/main.tex` };
}

// ---- project index ----------------------------------------------------------------------------------------------

function texFiles(root: string, overrides: Record<string, string>): [string, string][] {
  return [...files.keys()].filter((p) => p.startsWith(root + "/") && p.endsWith(".tex")).map((p) => [p, overrides[p] ?? files.get(p)!]);
}

function index(root: string, overrides: Record<string, string>): ProjectIndex {
  const outline: OutlineItem[] = [];
  const labels: Label[] = [];
  const citations = new Set<string>();
  let words = 0;
  const order = [`${root}/main.tex`, `${root}/sections/introducao.tex`, `${root}/sections/homologia.tex`];
  const all = texFiles(root, overrides).sort((a, b) => (order.indexOf(a[0]) + 1 || 99) - (order.indexOf(b[0]) + 1 || 99));
  for (const [path, text] of all) {
    if (path.endsWith("longo.tex")) continue;
    text.split("\n").forEach((line, i) => {
      const sec = /\\(part|chapter|section|subsection|subsubsection)\*?\{([^}]*)\}(?:\\label\{([^}]*)\})?/.exec(line);
      if (sec) outline.push({ level: ["part", "chapter", "section", "subsection", "subsubsection"].indexOf(sec[1]!), title: sec[2]!, path, line: i + 1, label: sec[3] });
      for (const m of line.matchAll(/\\label\{([^}]*)\}/g)) labels.push({ name: m[1]!, path, line: i + 1 });
      for (const m of line.matchAll(/\\cite\{([^}]*)\}/g)) m[1]!.split(",").forEach((k) => citations.add(k.trim()));
      if (!line.trimStart().startsWith("%")) words += line.replace(/\\[a-zA-Z]+|[{}$\\^_&]/g, " ").split(/\s+/).filter((w) => /\p{L}/u.test(w)).length;
    });
  }
  const bibKeys: BibEntry[] = [];
  BIB.split("\n").forEach((line, i) => {
    const m = /^@(\w+)\{([^,]+),/.exec(line);
    if (m) bibKeys.push({ key: m[2]!, type: m[1], path: `${root}/refs.bib`, line: i + 1 });
  });
  for (const b of bibKeys) {
    const block = BIB.slice(BIB.indexOf(b.key));
    b.title = /title\s*=\s*\{([^}]*)\}/.exec(block)?.[1];
    b.author = /author\s*=\s*\{([^}]*)\}/.exec(block)?.[1];
  }
  return {
    outline, labels, citations: [...citations], bibKeys, includes: ["sections/introducao", "sections/homologia"],
    rootFile: `${root}/main.tex`, warnings: [], stats: { words, pages: 2 },
  };
}

// ---- completion ------------------------------------------------------------------------------------------------

const COMMANDS: [string, string?][] = [
  ["section", "\\section{${1:title}}"], ["subsection", "\\subsection{${1:title}}"], ["label", "\\label{${1}}"],
  ["ref", "\\ref{${1}}"], ["eqref", "\\eqref{${1}}"], ["cref", "\\cref{${1}}"], ["cite", "\\cite{${1}}"],
  ["begin", "\\begin{${1:env}}\n\t$0\n\\end{${1:env}}"], ["emph", "\\emph{${1}}"], ["textbf", "\\textbf{${1}}"],
  ["partial"], ["sum", "\\sum_{${1}}^{${2}}"], ["frac", "\\frac{${1}}{${2}}"], ["mathbb", "\\mathbb{${1}}"],
  ["colon"], ["circ"], ["ker"], ["im"], ["dots"], ["hat", "\\hat{${1}}"], ["input", "\\input{${1}}"], ["includegraphics", "\\includegraphics{${1}}"],
];
const ENVS = ["equation", "align", "definition", "lemma", "theorem", "proof", "tikzcd", "itemize", "enumerate", "figure"];

function lineCol(text: string, line: number, column: number): number {
  let off = 0;
  for (let l = 1; l < line; l++) off = text.indexOf("\n", off) + 1;
  return off + column - 1;
}

function complete(root: string, text: string, line: number, column: number): CompletionResponse | null {
  const at = lineCol(text, line, column);
  const before = text.slice(Math.max(0, at - 80), at);
  const idx = index(root, {});
  let m = /\\(ref|eqref|cref|Cref|autoref|pageref)\{([^}]*)$/.exec(before);
  if (m) {
    const items: CompletionItem[] = idx.labels.filter((l) => l.name.startsWith(m![2]!)).map((l) => ({ label: l.name, kind: "label", detail: nameOf(l.path) + ":" + l.line, insertText: l.name }));
    return { from: at - m[2]!.length, to: at, prefix: m[2]!, context: "label", inMath: false, items };
  }
  m = /\\cite\w*\{([^}]*)$/.exec(before);
  if (m) {
    const items: CompletionItem[] = idx.bibKeys.filter((b) => b.key.startsWith(m![1]!)).map((b) => ({ label: b.key, kind: "citation", detail: b.title, insertText: b.key }));
    return { from: at - m[1]!.length, to: at, prefix: m[1]!, context: "citation", inMath: false, items };
  }
  m = /\\(begin|end)\{([a-zA-Z*]*)$/.exec(before);
  if (m) {
    const items: CompletionItem[] = ENVS.filter((e) => e.startsWith(m![2]!)).map((e) => ({ label: e, kind: "environment", insertText: e }));
    return { from: at - m[2]!.length, to: at, prefix: m[2]!, context: m[1] === "end" ? "endEnvironment" : "environment", inMath: false, items };
  }
  m = /\\([a-zA-Z]*)$/.exec(before);
  if (m) {
    const p = m[1]!;
    const items: CompletionItem[] = COMMANDS.filter(([c]) => c.startsWith(p)).map(([c, s]) => ({ label: "\\" + c, kind: s ? "snippet" : "command", insertText: s ?? "\\" + c }));
    if (!items.length) return null;
    return { from: at - p.length - 1, to: at, prefix: "\\" + p, context: "command", inMath: (before.split("$").length - 1) % 2 === 1, items };
  }
  return null;
}

// ---- a real, small PDF ---------------------------------------------------------------------------------------

type Run = { x: number; y: number; size: number; font: "R" | "B" | "I"; text: string };

/** Lines of the rendered paper, with the source line each came from (for the fake SyncTeX). */
const PAGE1: (Run & { src?: [string, number] })[] = [
  { x: 197, y: 680, size: 17, font: "R", text: "Notas de Homologia Simplicial", src: ["main.tex", 16] },
  { x: 255, y: 652, size: 12, font: "R", text: "Gabriel Ribeiro", src: ["main.tex", 17] },
  { x: 250, y: 630, size: 12, font: "R", text: "October 28, 2026", src: ["main.tex", 21] },
  { x: 133, y: 590, size: 14, font: "B", text: "1    Introdução", src: ["sections/introducao.tex", 2] },
  { x: 133, y: 566, size: 10.5, font: "R", text: "Estas notas seguem [1] e [2]. Na section 2 definimos os grupos de homologia", src: ["sections/introducao.tex", 4] },
  { x: 133, y: 552, size: 10.5, font: "R", text: "e calculamos a forma normal de Smith.", src: ["sections/introducao.tex", 5] },
  { x: 133, y: 518, size: 14, font: "B", text: "2    Homologia", src: ["sections/homologia.tex", 2] },
  { x: 133, y: 494, size: 10.5, font: "B", text: "Definição 2.1.", src: ["sections/homologia.tex", 4] },
  { x: 205, y: 494, size: 10.5, font: "R", text: "Seja K um complexo simplicial. O operador bordo \u2202n : Cn(K) \u2192", src: ["sections/homologia.tex", 5] },
  { x: 133, y: 480, size: 10.5, font: "R", text: "Cn\u22121(K) é", src: ["sections/homologia.tex", 5] },
  { x: 200, y: 452, size: 10.5, font: "I", text: "\u2202n[v0, . . . , vn] = \u03a3 (\u22121)i [v0, . . . , vi, . . . , vn].", src: ["sections/homologia.tex", 7] },
  { x: 465, y: 452, size: 10.5, font: "R", text: "(1)", src: ["sections/homologia.tex", 6] },
  { x: 133, y: 424, size: 10.5, font: "B", text: "Lema 2.2.", src: ["sections/homologia.tex", 11] },
  { x: 185, y: 424, size: 10.5, font: "I", text: "\u2202n\u22121 \u00b0 \u2202n = 0.", src: ["sections/homologia.tex", 12] },
  { x: 133, y: 400, size: 10.5, font: "I", text: "Proof.", src: ["sections/homologia.tex", 15] },
  { x: 165, y: 400, size: 10.5, font: "R", text: "Segue de (1): cada face de codimensão dois aparece duas vezes com", src: ["sections/homologia.tex", 16] },
  { x: 133, y: 386, size: 10.5, font: "R", text: "sinais opostos.", src: ["sections/homologia.tex", 16] },
  { x: 133, y: 362, size: 10.5, font: "B", text: "Teorema 2.3.", src: ["sections/homologia.tex", 19] },
  { x: 200, y: 362, size: 10.5, font: "I", text: "Hn(K) = ker \u2202n/ im \u2202n+1 é um Z-módulo finitamente ger-", src: ["sections/homologia.tex", 20] },
  { x: 133, y: 348, size: 10.5, font: "I", text: "ado.", src: ["sections/homologia.tex", 20] },
  { x: 255, y: 310, size: 10.5, font: "I", text: "Cn+1  \u2192  Cn  \u2192  Cn\u22121", src: ["sections/homologia.tex", 25] },
  { x: 255, y: 280, size: 10.5, font: "I", text: "Dn+1  \u2192  Dn  \u2192  Dn\u22121", src: ["sections/homologia.tex", 26] },
  { x: 302, y: 90, size: 10.5, font: "R", text: "1" },
];
const PAGE2: (Run & { src?: [string, number] })[] = [
  { x: 133, y: 700, size: 14, font: "B", text: "References", src: ["main.tex", 26] },
  { x: 133, y: 676, size: 10.5, font: "R", text: "[1]  Allen Hatcher. Algebraic Topology. Cambridge University Press, 2002.", src: ["refs.bib", 1] },
  { x: 133, y: 660, size: 10.5, font: "R", text: "[2]  James R. Munkres. Elements of Algebraic Topology. Addison-Wesley, 1984.", src: ["refs.bib", 8] },
  { x: 302, y: 90, size: 10.5, font: "R", text: "2" },
];
const PAGES = [PAGE1, PAGE2];

/** Characters outside WinAnsi are replaced with close Latin-1 look-alikes; the PDF uses the standard Times fonts. */
function winAnsi(s: string): string {
  const map: Record<string, string> = { "\u2202": "d", "\u2192": "->", "\u2212": "-", "\u03a3": "S", "\u00b0": "\u00b0" };
  let out = "";
  for (const ch of s) {
    const c = map[ch] ?? ch;
    for (const d of c) {
      const code = d.charCodeAt(0);
      if (d === "(" || d === ")" || d === "\\") out += "\\" + d;
      else if (code > 126) out += "\\" + (code & 0xff).toString(8).padStart(3, "0");
      else out += d;
    }
  }
  return out;
}

function buildPdf(): Uint8Array {
  const objs: string[] = [];
  const add = (body: string) => { objs.push(body); return objs.length; };
  const catalog = add("");
  const pagesId = add("");
  const fonts = {
    R: add("<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman /Encoding /WinAnsiEncoding >>"),
    B: add("<< /Type /Font /Subtype /Type1 /BaseFont /Times-Bold /Encoding /WinAnsiEncoding >>"),
    I: add("<< /Type /Font /Subtype /Type1 /BaseFont /Times-Italic /Encoding /WinAnsiEncoding >>"),
  };
  const kids: number[] = [];
  for (const runs of PAGES) {
    let content = "";
    for (const r of runs) content += `BT /F${r.font} ${r.size} Tf ${r.x} ${r.y} Td (${winAnsi(r.text)}) Tj ET\n`;
    if (runs === PAGE1) content += "0.4 w 465 386 6 6 re S\n"; // the proof's QED box
    const stream = add(`<< /Length ${content.length} >>\nstream\n${content}endstream`);
    kids.push(add(`<< /Type /Page /Parent ${pagesId} 0 R /MediaBox [0 0 612 792] /Contents ${stream} 0 R /Resources << /Font << /FR ${fonts.R} 0 R /FB ${fonts.B} 0 R /FI ${fonts.I} 0 R >> >> >>`));
  }
  objs[catalog - 1] = `<< /Type /Catalog /Pages ${pagesId} 0 R >>`;
  objs[pagesId - 1] = `<< /Type /Pages /Kids [${kids.map((k) => `${k} 0 R`).join(" ")}] /Count ${kids.length} >>`;
  let out = "%PDF-1.4\n%\u00e2\u00e3\u00cf\u00d3\n";
  const offsets: number[] = [];
  objs.forEach((body, i) => { offsets.push(out.length); out += `${i + 1} 0 obj\n${body}\nendobj\n`; });
  const xref = out.length;
  out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n` + offsets.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("");
  out += `trailer\n<< /Size ${objs.length + 1} /Root ${catalog} 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  const bytes = new Uint8Array(out.length);
  for (let i = 0; i < out.length; i++) bytes[i] = out.charCodeAt(i) & 0xff;
  return bytes;
}

let pdfBytes: Uint8Array | null = null;

function diagnostics(root: string): Diagnostic[] {
  const h = `${root}/sections/homologia.tex`;
  return [
    { severity: "info", message: "Overfull \\hbox (4.12pt too wide) in paragraph at lines 5--5", path: h, line: 5, source: "latex", detail: "[]\\OT1/cmr/m/n/10.95 Seja $K$ um complexo simplicial. O operador bordo" },
    { severity: "info", message: "Intersentence spacing (`\\@') should perhaps be used.", path: h, line: 16, column: 9, length: 1, source: "chktex" },
    { severity: "info", message: "You should put a space in front of parenthesis.", path: h, line: 25, column: 3, length: 1, source: "chktex" },
  ];
}

// ---- settings & recents ---------------------------------------------------------------------------------------

const STORE = "gabetex-mock-settings";
function loadSettings(): Settings {
  try {
    const raw = localStorage.getItem(STORE);
    if (raw) return JSON.parse(raw) as Settings;
  } catch { /* private window */ }
  const L = { thesis: "l-thesis", topology: "l-topology", papers: "l-papers", teaching: "l-teaching" };
  return {
    author: "Gabriel Ribeiro",
    labels: [
      { id: L.thesis, name: "Thesis", color: "purple" }, { id: L.topology, name: "Topology", color: "blue" },
      { id: L.papers, name: "Papers", color: "orange" }, { id: L.teaching, name: "Teaching", color: "green" },
    ],
    labelAssignments: {
      [`${DEMO_DIR}/Homologia`]: [L.thesis, L.topology], [`${DEMO_DIR}/Smith Normal Form`]: [L.topology, L.papers],
      [`${DEMO_DIR}/Masters Thesis`]: [L.thesis], [`${DEMO_DIR}/Persistent Homology Talk`]: [L.papers],
      [`${DEMO_DIR}/Linear Algebra Notes`]: [L.teaching], [`${DEMO_DIR}/Problem Sets`]: [L.teaching],
    },
  };
}
let settingsData = loadSettings();

const now = Date.now();
let recents: RecentProject[] = ["Homologia", "Smith Normal Form", "Masters Thesis", "Persistent Homology Talk", "Linear Algebra Notes", "Problem Sets"]
  .map((name, i) => ({ path: `${DEMO_DIR}/${name}`, name, lastOpened: now - i * 3_600_000 }));

// ---- SyncTeX ---------------------------------------------------------------------------------------------------

function forward(root: string, texPath: string, line: number): SyncBox[] {
  const rel = texPath.slice(root.length + 1);
  let best: { page: number; r: Run; d: number } | null = null;
  PAGES.forEach((runs, i) => runs.forEach((r) => {
    if (!r.src || r.src[0] !== rel) return;
    const d = Math.abs(r.src[1] - line);
    if (!best || d < best.d) best = { page: i + 1, r, d };
  }));
  if (!best) return [];
  const { page, r } = best as { page: number; r: Run };
  // PDF points, origin bottom-left (like the backend): the box starts just below the baseline.
  return [{ page, x: r.x, y: r.y - r.size * 0.25, width: Math.min(340, r.text.length * r.size * 0.45), height: r.size * 1.3 }];
}

function inverse(root: string, page: number, y: number): { path: string; line: number } | null {
  const runs = PAGES[page - 1];
  if (!runs) return null;
  let best: Run & { src?: [string, number] } | undefined;
  for (const r of runs) if (r.src && (!best || Math.abs(r.y - y) < Math.abs(best.y - y))) best = r;
  return best?.src ? { path: `${root}/${best.src[0]}`, line: best.src[1] } : null;
}

// ---- dispatch ---------------------------------------------------------------------------------------------------

type A = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));
const cancelled = new Set<string>();

async function handle(cmd: string, a: A): Promise<unknown> {
  switch (cmd) {
    case "open_project": return info(a.path);
    case "list_files": return tree(a.root);
    case "read_file": {
      const text = files.get(a.path);
      if (text === undefined) throw `No such file: ${a.path}`;
      return { text, encoding: "utf-8" };
    }
    case "write_file": files.set(a.path, a.text); return null;
    case "create_project": { const root = `${a.parentDir}/${a.name}`; return info(root); }
    case "project_index": return index(a.root, a.overrides ?? {});
    case "complete": return complete(a.root, a.text, a.line, a.column);
    case "completions": return complete(a.root, a.text, a.line, a.column)?.items ?? [];
    case "detect_tex": return { name: "TeX Live 2026", binDir: "/usr/local/texlive/2026/bin/x86_64-linux", latexmk: true, tlmgr: true, synctex: true };
    case "compile": {
      cancelled.delete(a.root);
      const started = performance.now();
      await delay(220);
      if (cancelled.has(a.root)) throw "Cancelled";
      const result: CompileResult = {
        ok: true, pdfPath: `${a.root}/main.pdf`, diagnostics: diagnostics(a.root), missingPackages: [],
        durationMs: Math.round(performance.now() - started), logPath: `${a.root}/main.log`,
      };
      return result;
    }
    case "cancel_compile": cancelled.add(a.root); return null;
    case "read_pdf": { pdfBytes ??= buildPdf(); return pdfBytes.slice().buffer; }
    case "synctex_forward": { const r = rootOf(a.texPath); return r ? forward(r, a.texPath, a.line) : []; }
    case "synctex_inverse": { const r = rootOf(a.pdfPath); return r ? inverse(r, a.page, a.y) : null; }
    case "install_packages": await delay(600); return { ok: true, output: `tlmgr install ${(a.packages as string[]).join(" ")}\n[1/1] done` };
    case "watch_project": case "unwatch_project": case "reveal_in_file_manager": return null;
    case "load_settings": return structuredClone(settingsData);
    case "save_settings":
      settingsData = a.settings as Settings;
      try { localStorage.setItem(STORE, JSON.stringify(settingsData)); } catch { /* ignore */ }
      return settingsData;
    case "recent_projects": return recents;
    case "add_recent_project": {
      recents = [{ path: a.path, name: a.name ?? nameOf(a.path), lastOpened: Date.now() }, ...recents.filter((r) => r.path !== a.path)];
      return recents;
    }
    case "set_project_label": recents = recents.map((r) => (r.path === a.path ? { ...r, label: a.label } : r)); return recents;
    // Plugins used by the shell.
    case "plugin:dialog|open": return a.options?.directory ? `${DEMO_DIR}/Homologia` : `${DEMO_DIR}/Homologia/main.tex`;
    case "plugin:dialog|message": case "plugin:dialog|ask": case "plugin:dialog|confirm": return true;
    case "plugin:path|resolve_directory": return "/home/demo";
    case "plugin:path|join": return (a.paths as string[]).join("/");
    case "plugin:path|dirname": return String(a.path).replace(/\/[^/]*$/, "");
    case "plugin:path|basename": return nameOf(String(a.path));
    case "plugin:window|title": return "GabeTeX";
    default:
      if (cmd.startsWith("plugin:webview|") || cmd.startsWith("plugin:window|")) return null;
      console.warn("[mock ipc] unhandled", cmd, a);
      return null;
  }
}

export function installMockBackend(): void {
  mockIPC((cmd, args) => handle(cmd, (args ?? {}) as A), { shouldMockEvents: true });
  const internals = (window as unknown as { __TAURI_INTERNALS__: Record<string, unknown> }).__TAURI_INTERNALS__;
  internals.metadata = { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } };
  for (const r of recents) seedProject(r.path);
}

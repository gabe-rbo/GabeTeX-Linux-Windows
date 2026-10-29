# GabeTeX for Linux and Windows — implementation plan (internal)

GabeTeX stays **native on macOS** (SwiftUI + AppKit, untouched). Linux and Windows get a companion build of the
same app, version **1.0.0**, that looks and behaves as close to the Mac app as possible while staying light and fast.

## Stack (decided)

- **Tauri 2** shell (Rust backend + the OS web view: WebView2 on Windows, WebKitGTK on Linux). No Electron,
  no bundled Chromium: installers stay ~5–10 MB, cold start well under a second.
- **Rust core** (`CrossPlatform/src-tauri/src/core/`): a faithful port of the parts of `TeXCore` and `TeXToolchain`
  the UI needs. The Swift sources are the spec; port behaviour, not just names. Unit tests via `cargo test`,
  reusing `Tests/Fixtures/TeXToolchain` logs where possible.
- **Frontend** (`CrossPlatform/src/`): Vite + TypeScript, **no UI framework** (plain DOM, tiny bundle).
  CodeMirror 6 for the editor, `@replit/codemirror-vim` for Vim mode, pdf.js for the PDF preview.
  CSS reproduces the macOS 26 Liquid Glass look (translucent sidebar, glass capsules/pills via
  `backdrop-filter`, SF-like system font stack, accent blue `#0A84FF`, Light/Dark via `prefers-color-scheme`).
- Localization: English + Brazilian Portuguese, following the OS language; strings come from
  `Resources/pt-BR.lproj/Localizable.strings` converted to `CrossPlatform/src/i18n/{en,pt-BR}.json`.
- Packaging: Linux `.AppImage` + `.deb`, Windows NSIS `.exe` (+ `.msi`). Release assets named
  `GabeTeX-linux-x86_64.AppImage`, `GabeTeX-linux-amd64.deb`, `GabeTeX-windows-x64-setup.exe`.
- CI: `.github/workflows/cross-platform.yml` (ubuntu-22.04 + windows-latest: `cargo test`, `npm run build`,
  `tauri build`; uploads installers; attaches them to the GitHub release on `v*` tags).

## Directory layout

```
CrossPlatform/
  package.json  vite.config.ts  tsconfig.json  index.html
  src/            main.ts, ipc.ts (typed wrappers), ui/*, editor/*, preview/*, i18n/*, styles/*
  src-tauri/      Cargo.toml, tauri.conf.json, icons/, src/{main.rs, lib.rs, commands.rs, core/*}
```

## IPC contract (frontend ⇄ Rust). Both sides must match exactly.

All paths are absolute strings. Lines/columns are **1-based**; columns are UTF-16 units (same as the Swift app
and as CodeMirror positions → convert at the edge). Rust types use `#[serde(rename_all = "camelCase")]`.

Commands (`invoke`):
| command | args | returns |
|---|---|---|
| `open_project` | `{ path }` | `ProjectInfo { root, name, files: FileNode[], rootFile?: string }` |
| `list_files` | `{ root }` | `FileNode[]` (tree; `{ name, path, isDir, children? }`, hidden + build dirs skipped) |
| `read_file` | `{ path }` | `{ text, encoding: "utf-8" \| "latin-1" }` |
| `write_file` | `{ path, text, encoding }` | `()` — atomic save |
| `create_project` | `{ parentDir, name, template }` | `ProjectInfo` (templates ported from TeXCore/Templates) |
| `project_index` | `{ root, overrides: {path: text}[] }` | `ProjectIndex { outline: OutlineItem[], labels: Label[], citations: string[], bibKeys: BibEntry[], includes: string[], rootFile? , warnings: Diagnostic[], stats: { words, ... } }` |
| `completions` | `{ root, path, text, line, column }` | `CompletionItem[] { label, kind, detail?, insertText (snippet syntax `${1:arg}`) }` |
| `detect_tex` | `{}` | `TeXDistribution { name, binDir, latexmk: bool, tlmgr: bool, synctex: bool } \| null` |
| `compile` | `{ root, rootFile, engine?: "pdflatex"\|"xelatex"\|"lualatex", overrides }` | `CompileResult { ok, pdfPath?, diagnostics: Diagnostic[], missingPackages: string[], durationMs, logPath? }` — compiles in a shadow workspace so unsaved buffers can be compiled while typing; cancels a still-running compile of the same project |
| `synctex_forward` | `{ pdfPath, texPath, line, column }` | `SyncBox[] { page, x, y, width, height }` (PDF points, origin bottom-left) |
| `synctex_inverse` | `{ pdfPath, page, x, y }` | `{ path, line, column? } \| null` |
| `install_packages` | `{ packages: string[] }` | `{ ok, output }` (tlmgr; elevate with pkexec on Linux, runas on Windows when needed) |
| `watch_project` / `unwatch_project` | `{ root }` | `()` — emits event `fs-changed { paths: string[] }` |
| `reveal_in_file_manager` | `{ path }` | `()` |
| `load_settings` / `save_settings` | — / `{ settings }` | `Settings` JSON (stored in the OS config dir) |
| `recent_projects` / `add_recent_project` / `set_project_label` | … | `RecentProject[] { path, name, lastOpened, label? }` |

`Diagnostic { severity: "error"\|"warning"\|"info", message, path, line, column?, source: "latex"\|"bibtex"\|"chktex"\|"reference"\|"structure" }`
`OutlineItem { level (part=-1, chapter=0, section=1, …), title, path, line, label? }`

## UI parity checklist (mirror the Mac app)

Welcome window (recent projects with color labels, New Project, Open Folder) · New-project sheet with templates ·
Project window: sidebar (file tree / outline tabs), editor tab pills, toolbar (compile button, engine, toggle PDF,
detach PDF), editor (LaTeX highlighting matching `EditorKit/Theme.swift`, gutter with line numbers + diagnostic marks,
inline diagnostics, completion popup, snippets with tab stops, Ctrl-click `\input`/`\ref` navigation, Vim mode),
PDF preview (pdf.js, keeps scroll/zoom on reload, zoom capsule, Ctrl-click → inverse SyncTeX, forward sync with a brief
highlight), live compile while typing (debounced), Issues panel, status bar capsules (word count, page, encoding,
compile state), missing-package banner, Quick Open (Ctrl+P), Command Palette (Ctrl+Shift+P) with editable shortcuts,
Settings window, Light/Dark, en/pt-BR. On Linux/Windows every ⌘ shortcut becomes Ctrl.

## Windows (port of the Mac scenes)

Every window runs the same frontend; its label says what it shows (`src/app/windows.ts`). Capabilities are scoped
per label (`src-tauri/capabilities/*.json`).

| label | shows | capability |
|---|---|---|
| `welcome` | Welcome, 700×450, not resizable (the window created at launch) | `default` |
| `project-<hash of path>` | one project (`?project=&file=`); several may be open; reused when the folder opens again | `default` |
| `pdf-<hash>` | the detached PDF of that project (`src/ui/pdfWindow.ts`, synced by `src/app/pdfLink.ts` events) | `pdf-window` |
| `present-<hash>` / `presenter-<hash>` | presentation: slides (controller) / presenter view on a second display (`src/present/`) | `presentation` |
| `main` | browser mock: single-window mode (in-window routing, PDF Only and in-window presentation as fallbacks) | `default` |

Native title “Project — file”; closing a project window asks about unsaved files (`onCloseRequested`), closes its
PDF and presentation windows and brings Welcome back when it was the last project window. Settings saved by one
window are broadcast (`gabetex://settings-changed`) and merged into the others.

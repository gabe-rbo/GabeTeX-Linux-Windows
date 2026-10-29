# GabeTeX for Linux and Windows

This is the Linux and Windows edition of GabeTeX. The macOS app stays fully native (SwiftUI/AppKit, in `Sources/`) and
is not part of this folder. See [PLAN.md](PLAN.md) for the stack, layout and IPC contract.

Stack: Tauri 2 (Rust) + Vite/TypeScript (no UI framework), CodeMirror 6, `@replit/codemirror-vim`, pdf.js.

## Features

Mirrors the Mac app (the Swift sources in `Sources/` are the reference for behaviour and look):

- **Windows like the Mac scenes:** a Welcome window (recent projects, color labels, filter and search, New Project with
  templates, Open Folder, Open File) and one window per project, titled “Project — file”. Closing a project with
  unsaved files asks first; closing the last project brings Welcome back.
- **Project sidebar:** file tree and outline across included files. The right-click menu matches the Mac's: New File,
  New Folder, Open, inline Rename (F2), Duplicate, Set as Main Document, Open in Default App, Reveal in File Manager,
  Copy Path and Move to Trash (the Mac's save, discard or cancel dialog when files have unsaved edits). Open tabs and
  the main document follow renames.
- **Editor (CodeMirror 6):** LaTeX highlighting with the Mac theme, tab pills, autocomplete for commands,
  environments, `\ref`/`\cite` keys, files and packages (with package badges, and Tab completion that also works
  offline from a built-in catalog), snippets with tab stops, Ctrl-click and Go to Definition on `\input`/`\ref`,
  a right-click menu (Show in PDF, Go to Definition, Toggle Comment, Cut/Copy/Paste/Select All), Vim mode, and
  UTF-8/Latin-1 detection with atomic saves.
- **Compiling:** detects TeX Live, MiKTeX and TinyTeX and runs `latexmk` (pdfLaTeX, XeLaTeX, LuaLaTeX) in a private
  shadow workspace, so unsaved text compiles while you type and your folder stays clean. Errors from the log, BibTeX
  and ChkTeX map to the right line of the right file. Live reference and citation warnings, a missing-package banner
  with `tlmgr` install, Build (Ctrl+B) exporting the PDF next to the main file, and Clean Auxiliary Files.
- **PDF preview (pdf.js):** keeps page and zoom across reloads, renders sharp at any zoom (tiles), supports rotation,
  clickable internal and external links, bundled CMaps and standard fonts, and two-way SyncTeX with a brief
  highlight. The PDF can be detached into its own window and docked again.
- **Beamer presentation mode** (Ctrl+Alt+Enter or the Present button): full-screen slides with the notes half split
  off, a presenter view on a second display, a go-to-slide overlay, and P to toggle the presenter view.
- Quick Open (Ctrl+P), Command Palette (Ctrl+Shift+P) with editable shortcuts, Issues panel, status bar with word and
  page count plus the Document Statistics popover, Settings window, Light and Dark, English and Brazilian Portuguese.

## Known gaps compared with the Mac app

- **No Touch Bar, no SwiftUI Liquid Glass.** The glass look is CSS (`backdrop-filter`); it is close but not identical,
  and window chrome comes from the OS.
- **Scroll bars stay visible.** WebKitGTK does not restyle custom scroll bars on hover or scroll, so the thin thumb
  is always shown instead of the Mac's overlay scroll bars.
- **Presentation on two displays is untested.** The slides plus presenter window path, Swap Displays and the state
  sync between the two windows have only run on a single display (Xvfb). On Windows, built-in laptop screens can't be
  told apart, so the presenter view goes to the display of the project window.
- **Windows is compiled, not run.** The Windows build is checked with `cargo check`/`clippy` for
  `x86_64-pc-windows-gnu` and built in CI, but the trash (`SHFileOperationW`), `open_url` and window paths have not
  been exercised on a real Windows machine.
- **Clean Auxiliary Files also deletes files in the project folder** (`.aux`, `.log`, `.synctex.gz` …, skipping
  `.git`, `node_modules` and similar). The Mac command only cancels the compile and drops the shadow workspace.
- **Biber:** works through `latexmk`, but the Mac's biber-specific diagnostics checks are not ported. `tlmgr` package
  search is not available (install by name only).
- When `latexmk` is missing, the Linux and Windows edition falls back to running the engine directly; the Mac app
  reports a tool error.
- Renaming a file with unsaved edits saves it first. Backend error messages that have no Mac equivalent are English.
- The AppImage is large (about 80 MB) because it carries WebKitGTK; the `.deb` uses the system's and is about 5 MB.
- Installers are not code-signed.

## Size and speed (Linux x86_64, measured on the release build)

| | |
|---|---|
| Release binary (`gabetex`, frontend embedded) | 7.6 MB |
| `.deb` / AppImage | 4.7 MB / 80 MB |
| First paint (JS 20.7 kB + CSS 8.1 kB gzip) | the editor (126 kB gzip) and pdf.js (156 kB gzip) load only when a project opens |
| Launch to painted Welcome window | about 1.7 s under Xvfb with software rendering (window mapped after 0.13 s) |

## Prerequisites

- Node.js 22+ and npm, Rust (stable) via [rustup](https://rustup.rs).
- A TeX distribution (TeX Live or MiKTeX) with `latexmk` for compiling documents.
- **Linux (Debian/Ubuntu):**
  `sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev patchelf build-essential libssl-dev`
- **Windows:** Visual Studio Build Tools (C++ workload) and the WebView2 runtime (preinstalled on Windows 11).

## Develop and build

```sh
cd CrossPlatform
npm install
npm run tauri dev        # run the app with hot reload
npm run build            # type-check + production frontend build
npm test                 # frontend tests
(cd src-tauri && cargo test)   # unit tests + end-to-end compile tests (tests/e2e.rs; skipped without TeX)
npm run tauri build      # installers: AppImage + deb (Linux), NSIS exe + msi (Windows)
```

Bundles land in `src-tauri/target/release/bundle/`. CI (`.github/workflows/cross-platform.yml`) builds both platforms and
attaches `GabeTeX-linux-x86_64.AppImage`, `GabeTeX-linux-amd64.deb`, `GabeTeX-windows-x64-setup.exe` and
`GabeTeX-windows-x64.msi` to the release on `v*` tags.

## Layout

- `src/` frontend; `src/ipc.ts` holds typed wrappers for every Rust command.
- `src-tauri/src/commands.rs` the command layer; `src-tauri/src/core/` the Rust port of the TeX core.

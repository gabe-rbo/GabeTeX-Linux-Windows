# GabeTeX for Linux & Windows

A light, fast LaTeX editor for Linux and Windows, part of the [GabeTeX](https://github.com/gabe-rbo/GabeTeX) project.
It mirrors the look and behaviour of the native macOS app, [GabeTeX-macOS](https://github.com/gabe-rbo/GabeTeX-macOS).

[README em português](README.pt-BR.md)

> **Status: in development, no release yet.** The Rust core (project scanner and index, completion, TeX toolchain,
> SyncTeX, file watcher, templates, settings) is ported and tested, and the Tauri UI mirroring the Mac app is
> implemented on top of it. It was exercised on Linux (including under Xvfb); the Windows build is compiled and built
> in CI but has not been run on a real Windows machine. See [Known gaps](#known-gaps).

## Links

- [GabeTeX](https://github.com/gabe-rbo/GabeTeX): the general project
- [GabeTeX-macOS](https://github.com/gabe-rbo/GabeTeX-macOS): the native macOS version (SwiftUI + AppKit)
- [docs-dev/PLAN.md](docs-dev/PLAN.md): stack, layout and IPC contract (internal planning note)

## Stack

Tauri 2 (Rust) + Vite/TypeScript (no UI framework), CodeMirror 6, `@replit/codemirror-vim`, pdf.js. One codebase
builds for both systems; the OS web view is WebKitGTK on Linux and WebView2 on Windows (no bundled Chromium).

## Features

- **Windows like the Mac scenes:** a Welcome window (recent projects, color labels, filter and search, New Project
  with templates, Open Folder, Open File) and one window per project. Closing a project with unsaved files asks first.
- **Project sidebar:** file tree and outline across included files, with the same context menu as the Mac (New File,
  New Folder, Rename, Duplicate, Set as Main Document, Reveal in File Manager, Move to Trash...).
- **Editor (CodeMirror 6):** LaTeX highlighting, tab pills, autocomplete for commands, environments, `\ref`/`\cite`
  keys, files and packages (also offline, from a built-in catalog), snippets with tab stops, Ctrl-click and Go to
  Definition on `\input`/`\ref`, Vim mode, UTF-8/Latin-1 detection and atomic saves.
- **Compiling:** detects TeX Live, MiKTeX and TinyTeX and runs `latexmk` (pdfLaTeX, XeLaTeX, LuaLaTeX) in a private
  shadow workspace, so unsaved text compiles while you type and your folder stays clean. Errors from the log, BibTeX
  and ChkTeX map to the right line of the right file; missing-package banner with `tlmgr` install.
- **PDF preview (pdf.js):** keeps page and zoom across reloads, sharp at any zoom, clickable links, two-way SyncTeX,
  and the PDF can be detached into its own window.
- **Beamer presentation mode** with a presenter view, Quick Open (Ctrl+P), Command Palette (Ctrl+Shift+P), Issues
  panel, status bar with word and page count, Settings, Light and Dark, English and Brazilian Portuguese.

## Requirements

- Node.js 22+ and npm, and Rust (stable) via [rustup](https://rustup.rs).
- A TeX distribution (TeX Live or MiKTeX) with `latexmk`, to compile documents.
- **Linux (Debian/Ubuntu):**
  `sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev patchelf build-essential libssl-dev`
- **Windows:** Visual Studio Build Tools (C++ workload) and the WebView2 runtime (preinstalled on Windows 11).

## Run and test

```sh
npm install
npm run tauri dev              # run the app with hot reload
npm run build                  # type-check + production frontend build
npm test                       # frontend tests (vitest)
(cd src-tauri && cargo test)   # Rust unit tests + end-to-end compile tests (tests/e2e.rs, skipped without TeX)
```

## Packages

`npm run tauri build` produces, in `src-tauri/target/release/bundle/`:

| System | Packages |
|---|---|
| Linux | AppImage, `.deb` |
| Windows | NSIS `.exe` installer, `.msi` |

CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) builds both systems on every push and pull request and, on
`v*` tags, attaches `GabeTeX-linux-x86_64.AppImage`, `GabeTeX-linux-amd64.deb`, `GabeTeX-windows-x64-setup.exe` and
`GabeTeX-windows-x64.msi` to the GitHub release. Installers are not code-signed.

## Known gaps

- No Touch Bar and no SwiftUI Liquid Glass: the glass look is CSS (`backdrop-filter`), close but not identical.
- Scroll bars stay visible (WebKitGTK does not support the Mac's overlay behaviour).
- Presentation on two displays is untested (only run on a single display under Xvfb).
- Windows is compiled and built in CI, not yet run on a real machine (trash, `open_url` and window paths).
- Clean Auxiliary Files also deletes `.aux`, `.log`, `.synctex.gz`... in the project folder.
- Biber works through `latexmk`, but the Mac's biber-specific diagnostics are not ported; `tlmgr` package search is
  not available (install by name only).
- The AppImage is large (about 80 MB, it carries WebKitGTK); the `.deb` uses the system's and is about 5 MB.

## Layout

- `src/`: frontend; `src/ipc.ts` holds typed wrappers for every Rust command.
- `src-tauri/src/commands.rs`: command layer; `src-tauri/src/core/`: the Rust port of the TeX core.
- `scripts/`: Vite plugin that serves the pdf.js assets. `docs-dev/`: internal notes.

## Contributing, conduct and security

See [CONTRIBUTING.md](CONTRIBUTING.md), [INSTRUCTIONS.md](INSTRUCTIONS.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
and [SECURITY.md](SECURITY.md).

## License

MIT, see [LICENSE](LICENSE).

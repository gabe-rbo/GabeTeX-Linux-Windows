# GabeTeX for Linux and Windows

This is the Linux and Windows edition of GabeTeX. The macOS app stays fully native (SwiftUI/AppKit, in `Sources/`) and
is not part of this folder. See [PLAN.md](PLAN.md) for the stack, layout and IPC contract.

Stack: Tauri 2 (Rust) + Vite/TypeScript (no UI framework), CodeMirror 6, `@replit/codemirror-vim`, pdf.js.

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
(cd src-tauri && cargo test)
npm run tauri build      # installers: AppImage + deb (Linux), NSIS exe + msi (Windows)
```

Bundles land in `src-tauri/target/release/bundle/`. CI (`.github/workflows/cross-platform.yml`) builds both platforms and
attaches `GabeTeX-linux-x86_64.AppImage`, `GabeTeX-linux-amd64.deb`, `GabeTeX-windows-x64-setup.exe` and
`GabeTeX-windows-x64.msi` to the release on `v*` tags.

## Layout

- `src/` frontend; `src/ipc.ts` holds typed wrappers for every Rust command.
- `src-tauri/src/commands.rs` the command layer; `src-tauri/src/core/` the Rust port of the TeX core.

# Developer Instructions: GabeTeX-Linux-Windows

This repository is a submodule of the umbrella repository [gabe-rbo/GabeTeX](https://github.com/gabe-rbo/GabeTeX).
One Tauri 2 app serves both Linux (WebKitGTK) and Windows (WebView2). The macOS app lives in
[GabeTeX-macOS](https://github.com/gabe-rbo/GabeTeX-macOS) and is the reference for behaviour and look.

## Branches
- `main`: always in a buildable state. Changes arrive only through pull requests.
- `feature/<short-name>`, `fix/<short-name>`, `chore/<short-name>`. Rebase on `main` and keep each PR focused.

## Where to change things
- `src/`: frontend (plain TypeScript DOM, no framework). `src/ui/` UI, `src/editor/` CodeMirror, `src/preview/` pdf.js,
  `src/present/` presentation mode, `src/i18n/{en,pt-BR}.json` strings, `src/styles/` CSS.
- `src/ipc.ts`: typed wrappers for every Rust command. It must match `src-tauri/src/commands.rs` and the contract in
  [docs-dev/PLAN.md](docs-dev/PLAN.md); change both sides together (camelCase over IPC, 1-based lines).
- `src-tauri/src/core/`: Rust port of the TeX core (scanner, index, completion, toolchain, SyncTeX, watcher,
  templates, settings). Port behaviour from the Swift sources in GabeTeX-macOS, not just names.
- `src-tauri/capabilities/*.json`: per-window permissions. A new window label needs a capability entry.
- `src-tauri/tauri.conf.json`: window, bundle targets (appimage, deb, nsis, msi) and version.
- `scripts/pdfjs-assets.mjs`: Vite plugin that copies pdf.js CMaps and fonts into `public/pdfjs/` (git-ignored).

## Build and run
```sh
npm install
npm run tauri dev        # hot-reload app (Vite on port 1420)
npm run build            # tsc --noEmit + vite build
npm run tauri build      # installers in src-tauri/target/release/bundle/
```
System dependencies are listed in [README.md](README.md#requirements).

## Tests
```sh
npm test                       # vitest
(cd src-tauri && cargo test)   # unit tests + tests/e2e.rs (real compile; skipped without TeX/latexmk)
```

Test fixtures (TeX logs and sample projects) live in `src-tauri/tests/fixtures/`, copied from the macOS repository.

## Packaging and release
CI builds on `ubuntu-22.04` and `windows-latest`. Pushing a `v*` tag attaches the AppImage, `.deb`, NSIS `.exe` and
`.msi` to the release. Keep the version in `package.json` and `src-tauri/tauri.conf.json` (and `Cargo.toml`) equal.
Windows code paths cannot be run on Linux: compile-check with `cargo check --target x86_64-pc-windows-gnu`.

## Before opening a pull request
- [ ] CI passes (`npm run build`, `npm test`, `cargo test`)
- [ ] `CHANGELOG.md` updated under `Unreleased`
- [ ] `README.md` and `README.pt-BR.md` updated if behavior or requirements change

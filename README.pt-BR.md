# GabeTeX para Linux e Windows

Um editor LaTeX leve e rápido para Linux e Windows, parte do projeto [GabeTeX](https://github.com/gabe-rbo/GabeTeX).
Espelha a aparência e o comportamento do app nativo para macOS, o [GabeTeX-macOS](https://github.com/gabe-rbo/GabeTeX-macOS).

[README in English](README.md)

> **Status: em desenvolvimento, sem release ainda.** O núcleo em Rust (scanner e índice do projeto, autocompletar,
> toolchain TeX, SyncTeX, observador de arquivos, templates, configurações) está portado e testado, e a interface
> Tauri que espelha o app do Mac está implementada sobre ele. Foi exercitada no Linux (inclusive sob Xvfb); a versão
> Windows é compilada e gerada no CI, mas ainda não foi executada em uma máquina Windows real.
> Veja as [Limitações conhecidas](#limitações-conhecidas).

## Links

- [GabeTeX](https://github.com/gabe-rbo/GabeTeX): projeto geral
- [GabeTeX-macOS](https://github.com/gabe-rbo/GabeTeX-macOS): versão nativa para macOS (SwiftUI + AppKit)
- [docs-dev/PLAN.md](docs-dev/PLAN.md): stack, estrutura e contrato de IPC (nota interna de planejamento, em inglês)

## Stack

Tauri 2 (Rust) + Vite/TypeScript (sem framework de UI), CodeMirror 6, `@replit/codemirror-vim`, pdf.js. Uma única base
de código gera os dois sistemas; a webview é a do SO (WebKitGTK no Linux, WebView2 no Windows), sem Chromium embutido.

## Recursos

- **Janelas como as cenas do Mac:** janela de Boas-vindas (projetos recentes, rótulos coloridos, filtro e busca, Novo
  Projeto com templates, Abrir Pasta, Abrir Arquivo) e uma janela por projeto. Fechar um projeto com arquivos não
  salvos pergunta antes.
- **Barra lateral:** árvore de arquivos e estrutura (outline) entre arquivos incluídos, com o mesmo menu de contexto do
  Mac (Novo Arquivo, Nova Pasta, Renomear, Duplicar, Definir como Documento Principal, Mostrar no Gerenciador de
  Arquivos, Mover para a Lixeira...).
- **Editor (CodeMirror 6):** realce LaTeX, abas em pílula, autocompletar de comandos, ambientes, chaves de
  `\ref`/`\cite`, arquivos e pacotes (também offline, por um catálogo embutido), snippets com tab stops, Ctrl+clique e
  Ir para Definição em `\input`/`\ref`, modo Vim, detecção UTF-8/Latin-1 e salvamento atômico.
- **Compilação:** detecta TeX Live, MiKTeX e TinyTeX e roda `latexmk` (pdfLaTeX, XeLaTeX, LuaLaTeX) em um workspace
  sombra privado, então o texto não salvo compila enquanto você digita e sua pasta fica limpa. Erros do log, BibTeX e
  ChkTeX apontam a linha certa do arquivo certo; faixa de pacote ausente com instalação via `tlmgr`.
- **Pré-visualização do PDF (pdf.js):** mantém página e zoom ao recarregar, nítida em qualquer zoom, links clicáveis,
  SyncTeX nos dois sentidos, e o PDF pode ser destacado em outra janela.
- **Modo apresentação Beamer** com visão do apresentador, Abrir Rápido (Ctrl+P), Paleta de Comandos (Ctrl+Shift+P),
  painel de Problemas, barra de status com contagem de palavras e páginas, Ajustes, claro e escuro, inglês e português
  do Brasil.

## Requisitos

- Node.js 22+ e npm, e Rust (stable) via [rustup](https://rustup.rs).
- Uma distribuição TeX (TeX Live ou MiKTeX) com `latexmk`, para compilar documentos.
- **Linux (Debian/Ubuntu):**
  `sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev patchelf build-essential libssl-dev`
- **Windows:** Visual Studio Build Tools (carga de trabalho C++) e o runtime WebView2 (já vem no Windows 11).

## Rodar e testar

```sh
npm install
npm run tauri dev              # roda o app com hot reload
npm run build                  # checagem de tipos + build de produção do frontend
npm test                       # testes do frontend (vitest)
(cd src-tauri && cargo test)   # testes unitários em Rust + testes e2e de compilação (tests/e2e.rs, ignorados sem TeX)
```

## Pacotes

`npm run tauri build` gera, em `src-tauri/target/release/bundle/`:

| Sistema | Pacotes |
|---|---|
| Linux | AppImage, `.deb` |
| Windows | instalador NSIS `.exe`, `.msi` |

O CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) compila os dois sistemas a cada push e pull request e, em
tags `v*`, anexa `GabeTeX-linux-x86_64.AppImage`, `GabeTeX-linux-amd64.deb`, `GabeTeX-windows-x64-setup.exe` e
`GabeTeX-windows-x64.msi` ao release no GitHub. Os instaladores não são assinados.

## Limitações conhecidas

- Sem Touch Bar e sem o Liquid Glass do SwiftUI: o efeito de vidro é CSS (`backdrop-filter`), próximo mas não idêntico.
- As barras de rolagem ficam sempre visíveis (o WebKitGTK não suporta o comportamento overlay do Mac).
- Apresentação com dois monitores não testada (só rodou em um monitor, sob Xvfb).
- O Windows é compilado e gerado no CI, mas ainda não foi executado em máquina real (lixeira, `open_url`, janelas).
- Limpar Arquivos Auxiliares também apaga `.aux`, `.log`, `.synctex.gz`... na pasta do projeto.
- Biber funciona via `latexmk`, mas os diagnósticos específicos de biber do Mac não foram portados; a busca de pacotes
  do `tlmgr` não existe (instalação só por nome).
- O AppImage é grande (cerca de 80 MB, leva o WebKitGTK); o `.deb` usa o do sistema e tem cerca de 5 MB.

## Estrutura

- `src/`: frontend; `src/ipc.ts` tem wrappers tipados para cada comando Rust.
- `src-tauri/src/commands.rs`: camada de comandos; `src-tauri/src/core/`: port em Rust do núcleo TeX.
- `scripts/`: plugin do Vite que serve os assets do pdf.js. `docs-dev/`: notas internas.

## Contribuir, conduta e segurança

Veja [CONTRIBUTING.md](CONTRIBUTING.md), [INSTRUCTIONS.md](INSTRUCTIONS.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) e
[SECURITY.md](SECURITY.md).

## Licença

MIT, veja [LICENSE](LICENSE).

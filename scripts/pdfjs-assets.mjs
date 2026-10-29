// Vite plugin: copies the pdf.js data files (CJK CMaps, standard fonts, ICC profiles, wasm decoders) from
// `pdfjs-dist` into `public/pdfjs/` (git-ignored). They are fetched on demand by the pdf.js worker, never bundled.
import { cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";

export default function pdfjsAssets() {
  return {
    name: "gabetex-pdfjs-assets",
    buildStart() {
      const from = "node_modules/pdfjs-dist";
      const to = "public/pdfjs";
      const version = JSON.parse(readFileSync(`${from}/package.json`, "utf8")).version;
      const stamp = `${to}/.version`;
      if (existsSync(stamp) && readFileSync(stamp, "utf8") === version) return;
      for (const dir of ["cmaps", "standard_fonts", "iccs", "wasm"]) {
        mkdirSync(`${to}/${dir}`, { recursive: true });
        cpSync(`${from}/${dir}`, `${to}/${dir}`, { recursive: true });
      }
      writeFileSync(stamp, version);
    },
  };
}

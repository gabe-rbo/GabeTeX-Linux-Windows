// Lazy pdf.js loader. The library (~520 kB) and its worker (~1.3 MB, a separate asset) load on the first
// `load()`, never with the main bundle. The legacy build is used on purpose: the modern one relies on very new
// JavaScript (`Math.sumPrecise`, `Uint8Array.fromBase64`, …) that WebKitGTK on LTS distributions may lack.
import type * as PDFJS from "pdfjs-dist/legacy/build/pdf.mjs";
import workerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";

export type PdfLib = typeof PDFJS;
export type PDFDocumentProxy = PDFJS.PDFDocumentProxy;
export type PDFPageProxy = PDFJS.PDFPageProxy;
export type PageViewport = PDFJS.PageViewport;
export type RenderTask = PDFJS.RenderTask;
export type TextLayer = PDFJS.TextLayer;
export type TextContent = Awaited<ReturnType<PDFPageProxy["getTextContent"]>>;

let lib: Promise<{ lib: PdfLib; worker: PDFJS.PDFWorker }> | null = null;

/** Loads pdf.js once and starts one shared worker, reused by every document (no worker start-up per build). */
export function loadPdfJs(): Promise<{ lib: PdfLib; worker: PDFJS.PDFWorker }> {
  lib ??= import("pdfjs-dist/legacy/build/pdf.mjs").then((mod) => {
    mod.GlobalWorkerOptions.workerSrc = workerUrl;
    return { lib: mod, worker: new mod.PDFWorker() };
  }).catch((error: unknown) => {
    lib = null; // allow a retry
    throw error;
  });
  return lib;
}

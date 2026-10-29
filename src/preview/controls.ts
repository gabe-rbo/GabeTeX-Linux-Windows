// Floating Liquid Glass capsule of the preview (PDFPreviewControls): zoom −, percentage (click = fit width),
// zoom +, separator, "3 / 12". And the empty state (PDFPreviewPlaceholder).
import { t } from "../i18n";

const SVG_NS = "http://www.w3.org/2000/svg";

function svg(d: string, size: number, strokeWidth: number): SVGSVGElement {
  const el = document.createElementNS(SVG_NS, "svg");
  el.setAttribute("viewBox", "0 0 24 24");
  el.setAttribute("width", String(size));
  el.setAttribute("height", String(size));
  el.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(SVG_NS, "path");
  path.setAttribute("d", d);
  path.setAttribute("fill", "none");
  path.setAttribute("stroke", "currentColor");
  path.setAttribute("stroke-width", String(strokeWidth));
  path.setAttribute("stroke-linecap", "round");
  path.setAttribute("stroke-linejoin", "round");
  el.append(path);
  return el;
}

function button(cls: string, label: string, onClick: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.className = `pv-btn ${cls}`;
  b.title = label;
  b.setAttribute("aria-label", label);
  b.addEventListener("click", onClick);
  return b;
}

export interface ControlsActions { zoomIn(): void; zoomOut(): void; fit(): void }

export class PreviewControls {
  readonly el = document.createElement("div");
  private readonly out: HTMLButtonElement;
  private readonly inn: HTMLButtonElement;
  private readonly zoom: HTMLButtonElement;
  private readonly pages = document.createElement("span");
  private last = "";

  constructor(actions: ControlsActions) {
    this.el.className = "pv-controls";
    this.el.setAttribute("role", "group");
    this.el.setAttribute("aria-label", t("PDF zoom and pages"));
    // The capsule never takes focus away from the editor (like the Mac buttons).
    this.el.addEventListener("mousedown", (e) => e.preventDefault());

    this.out = button("pv-icon", t("Zoom Out"), actions.zoomOut);
    this.out.append(svg("M6 12h12", 14, 2.4));
    this.zoom = button("pv-zoom", t("Fit Width"), actions.fit);
    this.zoom.setAttribute("aria-label", t("Zoom"));
    this.inn = button("pv-icon", t("Zoom In"), actions.zoomIn);
    this.inn.append(svg("M12 6v12M6 12h12", 14, 2.4));
    const sep = document.createElement("span");
    sep.className = "pv-sep";
    sep.setAttribute("aria-hidden", "true");
    this.pages.className = "pv-pages";
    this.el.append(this.out, this.zoom, this.inn, sep, this.pages);
  }

  /** Cheap: touches the DOM only when something visible changed. */
  update(scale: number, page: number, total: number, canIn: boolean, canOut: boolean, fits: boolean): void {
    const percent = Math.round(scale * 100);
    const key = `${percent}|${page}|${total}|${canIn}|${canOut}|${fits}`;
    if (key === this.last) return;
    this.last = key;
    this.zoom.textContent = `${percent}%`;
    this.zoom.setAttribute("aria-valuetext", fits ? `${percent}%, ${t("Fit Width")}` : `${percent}%`);
    this.inn.disabled = !canIn;
    this.out.disabled = !canOut;
    this.pages.textContent = total > 0 ? `${page} / ${total}` : "– / –";
    this.pages.setAttribute("aria-label", total > 0 ? t("Page %lld of %lld", page, total) : t("No pages"));
  }
}

/** Empty state shown until the first PDF exists. */
export function createPlaceholder(): HTMLElement {
  const el = document.createElement("div");
  el.className = "pv-placeholder";
  const icon = svg(
    "M6.5 2.8h7.8l4.2 4.2v12.5a1.7 1.7 0 0 1-1.7 1.7H6.5a1.7 1.7 0 0 1-1.7-1.7V4.5a1.7 1.7 0 0 1 1.7-1.7ZM14 3v4.3h4.3M8.3 11.5h7.4M8.3 14.5h7.4M8.3 17.5h4.6",
    44, 1.1);
  const title = document.createElement("div");
  title.className = "pv-placeholder-title";
  title.textContent = t("No PDF yet");
  const hint = document.createElement("div");
  hint.className = "pv-placeholder-hint";
  hint.textContent = t("Press ⌘B to compile");
  el.append(icon, title, hint);
  return el;
}

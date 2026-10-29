// New Label / Rename Label sheet (port of LabelEditorSheet.swift).
import { t } from "../i18n";
import * as labels from "../app/labels";
import type { LabelColor } from "../app/settings";
import { h } from "./dom";
import { showSheet } from "./overlay";

/** Opens the editor. `editId` edits an existing label; otherwise creates one (assigned to `assignTo`). */
export function openLabelEditor(opts: { editId?: string; assignTo?: string; onDone?: () => void }): void {
  const existing = opts.editId ? labels.allLabels().find((l) => l.id === opts.editId) : undefined;
  let color: LabelColor = existing?.color ?? labels.nextColor();
  const input = h("input", { class: "field", value: existing?.name ?? "", attrs: { type: "text", placeholder: t("Label name"), "aria-label": t("Label name"), maxlength: labels.MAX_NAME, autofocus: true } });
  const taken = h("div", { class: "prompt-error" });
  const commitBtn = h("button", { class: "btn prominent", text: existing ? t("Save") : t("Create") });
  const swatches = h("div", { class: "swatches", attrs: { role: "radiogroup" } });
  const renderSwatches = () => {
    swatches.textContent = "";
    for (const c of labels.LABEL_COLORS) {
      swatches.appendChild(h("button", {
        class: "swatch" + (c === color ? " selected" : ""), title: labels.colorTitle(c),
        style: { background: labels.labelCSS(c) },
        attrs: { role: "radio", "aria-checked": String(c === color), "aria-label": labels.colorTitle(c) },
        on: { click: () => { color = c; renderSwatches(); } },
      }));
    }
  };
  renderSwatches();
  const validate = () => {
    const ok = labels.isNameAvailable(input.value, existing?.id);
    commitBtn.disabled = !ok;
    taken.textContent = !ok && labels.normalizedName(input.value) ? t("A label with this name already exists.") : "";
    return ok;
  };
  const commit = () => {
    if (!validate()) return;
    if (existing) labels.editLabel(existing.id, input.value, color);
    else labels.createLabel(input.value, color, opts.assignTo);
    sheet.close();
    opts.onDone?.();
  };
  input.addEventListener("input", validate);
  input.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); commit(); } });
  commitBtn.addEventListener("click", commit);
  validate();
  const sheet = showSheet([
    h("div", { class: "sheet-title", text: existing ? t("Rename Label") : t("New Label") }),
    input, swatches, taken,
    h("div", { class: "alert-buttons row end" }, h("button", { class: "btn glass", text: t("Cancel"), on: { click: () => sheet.close() } }), commitBtn),
  ], { class: "label-editor", label: t("New Label") });
}

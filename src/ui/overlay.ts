// Window-level overlays: sheets (modal glass cards), alerts, name prompts and popup / context menus.
import { t } from "../i18n";
import { h, type Child } from "./dom";
import { icon } from "./icons";

let sheetDepth = 0;
/** True while a modal sheet is open (global shortcuts pause). */
export const isModalOpen = (): boolean => sheetDepth > 0;

export interface Sheet {
  card: HTMLElement;
  close(): void;
  closed: Promise<void>;
}

/**
 * Presents `content` in a modal glass card over a dimmed window. Escape and a click on the backdrop call `onCancel`
 * (default: close). Focus is kept inside the card.
 */
export function showSheet(content: Child, opts: { class?: string; onCancel?: () => void; label?: string; dismissOnBackdrop?: boolean } = {}): Sheet {
  const previous = document.activeElement as HTMLElement | null;
  const card = h("div", { class: "sheet glass-panel " + (opts.class ?? ""), attrs: { role: "dialog", "aria-modal": "true", "aria-label": opts.label } }, content);
  const backdrop = h("div", { class: "sheet-backdrop" }, card);
  let resolve!: () => void;
  const closed = new Promise<void>((r) => (resolve = r));
  let open = true;
  const close = () => {
    if (!open) return;
    open = false;
    sheetDepth--;
    backdrop.classList.add("closing");
    document.removeEventListener("keydown", onKey, true);
    const done = () => { backdrop.remove(); resolve(); };
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) done();
    else setTimeout(done, 140);
    previous?.focus?.();
  };
  const cancel = () => (opts.onCancel ? opts.onCancel() : close());
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); cancel(); }
    else if (e.key === "Tab") trapFocus(card, e);
  };
  backdrop.addEventListener("mousedown", (e) => { if (e.target === backdrop && opts.dismissOnBackdrop !== false) cancel(); });
  document.addEventListener("keydown", onKey, true);
  document.body.appendChild(backdrop);
  sheetDepth++;
  requestAnimationFrame(() => {
    const first = card.querySelector<HTMLElement>("[autofocus], input, select, textarea, button.prominent, button");
    first?.focus();
  });
  return { card, close, closed };
}

function trapFocus(root: HTMLElement, e: KeyboardEvent): void {
  const items = [...root.querySelectorAll<HTMLElement>("button, input, select, textarea, [tabindex]:not([tabindex='-1'])")].filter((el) => !el.hasAttribute("disabled") && el.offsetParent !== null);
  if (!items.length) return;
  const first = items[0]!;
  const last = items[items.length - 1]!;
  if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
  else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
}

export interface AlertButton { title: string; role?: "cancel" | "destructive" | "default"; value: string }

/** A macOS-style alert: icon, bold title, message, stacked buttons. Resolves with the chosen button's value. */
export function alertDialog(opts: { title: string; message?: string; buttons?: AlertButton[]; icon?: "warning" | "info" | "app" }): Promise<string> {
  const buttons = opts.buttons ?? [{ title: t("OK"), role: "default", value: "ok" }];
  const cancelValue = buttons.find((b) => b.role === "cancel")?.value ?? buttons[buttons.length - 1]?.value ?? "cancel";
  return new Promise((resolve) => {
    let sheet: Sheet;
    const choose = (v: string) => { sheet.close(); resolve(v); };
    const row = h("div", { class: "alert-buttons" + (buttons.length > 2 ? " stacked" : "") },
      buttons.map((b) => h("button", {
        class: "btn " + (b.role === "default" ? "prominent" : b.role === "destructive" ? "destructive" : "glass"),
        text: b.title, on: { click: () => choose(b.value) },
      })));
    sheet = showSheet([
      h("div", { class: "alert-icon" }, icon(opts.icon === "info" ? "info" : "warningFill", 34)),
      h("div", { class: "alert-title", text: opts.title }),
      opts.message ? h("div", { class: "alert-message", text: opts.message }) : null,
      row,
    ], { class: "alert", onCancel: () => choose(cancelValue), label: opts.title });
    // Return activates the default button.
    sheet.card.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !(e.target instanceof HTMLButtonElement)) {
        const def = buttons.find((b) => b.role === "default");
        if (def) { e.preventDefault(); choose(def.value); }
      }
    });
  });
}

/** A one-field dialog (“New File”, “Go to Line”). Resolves with the text, or null when cancelled. */
export function promptDialog(opts: { title: string; message?: string; label: string; initial?: string; confirm: string; selectStem?: boolean; validate?: (v: string) => string | null }): Promise<string | null> {
  return new Promise((resolve) => {
    let sheet: Sheet;
    const input = h("input", { class: "field", value: opts.initial ?? "", attrs: { type: "text", "aria-label": opts.label, placeholder: opts.label, spellcheck: "false", autofocus: true } });
    const error = h("div", { class: "prompt-error" });
    const ok = h("button", { class: "btn prominent", text: opts.confirm });
    const finish = (v: string | null) => { sheet.close(); resolve(v); };
    const submit = () => {
      const problem = opts.validate?.(input.value) ?? null;
      if (problem) { error.textContent = problem; return; }
      finish(input.value);
    };
    ok.addEventListener("click", submit);
    input.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); submit(); } });
    sheet = showSheet([
      h("div", { class: "alert-title", text: opts.title }),
      opts.message ? h("div", { class: "alert-message", text: opts.message }) : null,
      input, error,
      h("div", { class: "alert-buttons row" }, h("button", { class: "btn glass", text: t("Cancel"), on: { click: () => finish(null) } }), ok),
    ], { class: "alert prompt", onCancel: () => finish(null), label: opts.title });
    requestAnimationFrame(() => {
      input.focus();
      const dot = input.value.lastIndexOf(".");
      if (opts.selectStem && dot > 0) input.setSelectionRange(0, dot);
      else input.select();
    });
  });
}

// ---- menus ----------------------------------------------------------------------------------------------------

export type MenuItem =
  | { kind?: "item"; title: string; icon?: string; checked?: boolean; disabled?: boolean; destructive?: boolean; shortcut?: string; dot?: string; action: () => void }
  | { kind: "separator" }
  | { kind: "header"; title: string };

let openMenu: { el: HTMLElement; close: () => void } | null = null;

/** Closes the open popup menu, if any. */
export function closeMenu(): void {
  openMenu?.close();
}

/** A glass popup menu at a point (context menu) or under an anchor element. Arrow keys, Return and Escape work. */
export function popupMenu(items: MenuItem[], at: { x: number; y: number } | HTMLElement, opts: { align?: "start" | "end"; minWidth?: number } = {}): void {
  closeMenu();
  const el = h("div", { class: "menu glass-panel", attrs: { role: "menu" } });
  if (opts.minWidth) el.style.minWidth = `${opts.minWidth}px`;
  const buttons: HTMLButtonElement[] = [];
  for (const item of items) {
    if (item.kind === "separator") { el.appendChild(h("div", { class: "menu-sep", attrs: { role: "separator" } })); continue; }
    if (item.kind === "header") { el.appendChild(h("div", { class: "menu-header", text: item.title })); continue; }
    const b = h("button", {
      class: "menu-item" + (item.destructive ? " destructive" : ""), disabled: !!item.disabled,
      attrs: { role: item.checked !== undefined ? "menuitemcheckbox" : "menuitem", "aria-checked": item.checked === undefined ? undefined : String(item.checked) },
    },
    h("span", { class: "menu-check", text: item.checked ? "✓" : "" }),
    item.dot ? h("span", { class: "label-dot", style: { background: item.dot } }) : item.icon ? icon(item.icon, 14, "menu-icon") : null,
    h("span", { class: "menu-title", text: item.title }),
    item.shortcut ? h("span", { class: "menu-shortcut", text: item.shortcut }) : null);
    b.addEventListener("click", () => { close(); item.action(); });
    b.addEventListener("mouseenter", () => b.focus({ preventScroll: true }));
    buttons.push(b);
    el.appendChild(b);
  }
  document.body.appendChild(el);
  // Position: below the anchor or at the point, kept inside the window.
  const r = el.getBoundingClientRect();
  let x: number, y: number;
  if (at instanceof HTMLElement) {
    const a = at.getBoundingClientRect();
    x = opts.align === "end" ? a.right - r.width : a.left;
    y = a.bottom + 6;
    if (y + r.height > innerHeight - 8) y = a.top - r.height - 6;
  } else {
    x = at.x;
    y = at.y;
    if (y + r.height > innerHeight - 8) y = Math.max(8, at.y - r.height);
  }
  x = Math.min(Math.max(8, x), innerWidth - r.width - 8);
  y = Math.min(Math.max(8, y), innerHeight - r.height - 8);
  el.style.transform = `translate(${Math.round(x)}px, ${Math.round(y)}px)`;
  requestAnimationFrame(() => el.classList.add("open"));

  const onDown = (e: MouseEvent) => { if (!el.contains(e.target as Node)) close(); };
  const onKey = (e: KeyboardEvent) => {
    const enabled = buttons.filter((b) => !b.disabled);
    const i = enabled.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); }
    else if (e.key === "ArrowDown") { e.preventDefault(); enabled[(i + 1) % enabled.length]?.focus(); }
    else if (e.key === "ArrowUp") { e.preventDefault(); enabled[(i - 1 + enabled.length) % enabled.length]?.focus(); }
  };
  const close = () => {
    if (openMenu?.el !== el) return;
    openMenu = null;
    el.remove();
    document.removeEventListener("mousedown", onDown, true);
    document.removeEventListener("keydown", onKey, true);
    window.removeEventListener("blur", close);
    window.removeEventListener("resize", close);
  };
  openMenu = { el, close };
  setTimeout(() => {
    document.addEventListener("mousedown", onDown, true);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
  });
  document.addEventListener("keydown", onKey, true);
  if (!(at instanceof HTMLElement)) return;
  if (document.activeElement === at) buttons.find((b) => !b.disabled)?.focus({ preventScroll: true });
}

// ---- toast ----------------------------------------------------------------------------------------------------

/** A short status message floating at the bottom of the window (Mac `flash`). */
export function toast(message: string, ms = 2200): void {
  const el = h("div", { class: "toast glass-capsule", text: message, attrs: { role: "status" } });
  document.body.appendChild(el);
  requestAnimationFrame(() => el.classList.add("open"));
  setTimeout(() => { el.classList.remove("open"); setTimeout(() => el.remove(), 250); }, ms);
}

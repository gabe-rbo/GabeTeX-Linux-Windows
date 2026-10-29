// Keyboard shortcuts (port of ShortcutStore / ShortcutMap): registry defaults plus the user's changes from
// Settings › Shortcuts, persisted in settings.shortcuts as { commandId: "Ctrl+Shift+P" | "" }.
// On Linux and Windows every ⌘ of the Mac app becomes Ctrl, ⌥ becomes Alt.
import { settings } from "./settings";

/** Canonical combo string: modifiers in the order Ctrl, Alt, Shift, Meta, then the key ("Ctrl+Shift+P"). */
export type Combo = string;

const CODE_KEYS: Record<string, string> = {
  Slash: "/", Backslash: "\\", BracketLeft: "[", BracketRight: "]", Quote: "'", Period: ".", Comma: ",",
  Minus: "-", Equal: "=", Semicolon: ";", Backquote: "`", Space: "Space", Enter: "Enter", NumpadEnter: "Enter",
  Tab: "Tab", Escape: "Esc", Backspace: "Backspace", Delete: "Delete", ArrowUp: "Up", ArrowDown: "Down",
  ArrowLeft: "Left", ArrowRight: "Right", Home: "Home", End: "End", PageUp: "PageUp", PageDown: "PageDown",
};

const MODIFIER_KEYS = new Set(["Control", "Shift", "Alt", "Meta", "AltGraph", "OS"]);

/** The key part of an event, layout-independent for letters and digits; null for a bare modifier. */
export function eventKey(e: KeyboardEvent): string | null {
  if (MODIFIER_KEYS.has(e.key)) return null;
  const code = e.code;
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit\d$/.test(code)) return code.slice(5);
  if (/^Numpad\d$/.test(code)) return code.slice(6);
  if (/^F\d{1,2}$/.test(code)) return code;
  const mapped = CODE_KEYS[code];
  if (mapped) return mapped;
  if (e.key.length === 1) return e.key.toUpperCase();
  return e.key || null;
}

/** The combo an event represents, or null for a bare modifier press. */
export function comboFromEvent(e: KeyboardEvent): Combo | null {
  const key = eventKey(e);
  if (!key) return null;
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Meta");
  parts.push(key);
  return parts.join("+");
}

/** True when the combo has a command modifier (shortcuts without one would steal typing). */
export function hasCommandModifier(combo: Combo): boolean {
  return /(^|\+)(Ctrl|Alt|Meta)\+/.test(combo) || /^F\d+$/.test(combo.split("+").pop() ?? "");
}

export interface ShortcutDefinition { id: string; defaultCombo?: Combo; customizable: boolean }

type Outcome = { ok: true } | { ok: false; reason: "reserved" | "conflict" | "noModifier"; conflicts: string[] };

class ShortcutStore {
  private defs = new Map<string, ShortcutDefinition>();
  private listeners = new Set<() => void>();
  private byCombo = new Map<Combo, string[]>();

  register(defs: ShortcutDefinition[]): void {
    for (const d of defs) this.defs.set(d.id, d);
    this.reindex();
  }

  /** The current combo for a command (undefined = none). */
  combo(id: string): Combo | undefined {
    const custom = settings.get("shortcuts")[id];
    if (custom !== undefined) return custom || undefined;
    return this.defs.get(id)?.defaultCombo;
  }

  defaultCombo(id: string): Combo | undefined {
    return this.defs.get(id)?.defaultCombo;
  }

  isCustomized(id: string): boolean {
    const custom = settings.get("shortcuts")[id];
    return custom !== undefined && (custom || undefined) !== this.defs.get(id)?.defaultCombo;
  }

  isCustomizable(id: string): boolean {
    return this.defs.get(id)?.customizable ?? false;
  }

  get hasCustomizations(): boolean {
    return [...this.defs.keys()].some((id) => this.isCustomized(id));
  }

  /** Commands bound to `combo` (first = the one that runs). */
  commands(combo: Combo): string[] {
    return this.byCombo.get(combo) ?? [];
  }

  check(combo: Combo | undefined, id: string): Outcome {
    if (!combo) return { ok: true };
    if (!hasCommandModifier(combo)) return { ok: false, reason: "noModifier", conflicts: [] };
    const others = this.commands(combo).filter((c) => c !== id);
    if (others.some((c) => !this.isCustomizable(c))) return { ok: false, reason: "reserved", conflicts: others };
    if (others.length) return { ok: false, reason: "conflict", conflicts: others };
    return { ok: true };
  }

  /** Assigns (or removes, with undefined) a shortcut. With `replacing`, conflicting commands lose theirs. */
  assign(id: string, combo: Combo | undefined, replacing = false): Outcome {
    const outcome = this.check(combo, id);
    if (!outcome.ok && !(replacing && outcome.reason === "conflict")) return outcome;
    settings.update("shortcuts", (map) => {
      if (!outcome.ok) for (const other of outcome.conflicts) map[other] = "";
      if ((combo ?? undefined) === this.defs.get(id)?.defaultCombo) delete map[id];
      else map[id] = combo ?? "";
    });
    this.changed();
    return { ok: true };
  }

  restoreDefault(id: string): Outcome {
    const def = this.defs.get(id)?.defaultCombo;
    const outcome = this.check(def, id);
    if (!outcome.ok) return outcome;
    settings.update("shortcuts", (map) => { delete map[id]; });
    this.changed();
    return { ok: true };
  }

  restoreAll(): void {
    settings.set("shortcuts", {});
    this.changed();
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private changed(): void {
    this.reindex();
    for (const l of this.listeners) l();
  }

  private reindex(): void {
    this.byCombo.clear();
    for (const id of this.defs.keys()) {
      const c = this.combo(id);
      if (!c) continue;
      const list = this.byCombo.get(c);
      if (list) list.push(id);
      else this.byCombo.set(c, [id]);
    }
  }
}

export const shortcuts = new ShortcutStore();

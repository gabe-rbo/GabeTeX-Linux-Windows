// Project labels (port of ProjectLabels.swift): named, coloured labels for the Welcome window's recent projects.
// Names and assignments live in the settings JSON; the backend keeps the first label's colour per project so
// labeled projects are never trimmed from the recents list.
import * as ipc from "../ipc";
import { t } from "../i18n";
import { settings, type LabelColor, type LabelDef } from "./settings";

export const LABEL_COLORS: LabelColor[] = ["red", "orange", "yellow", "green", "blue", "purple", "gray"];
export const COLOR_NAMES: Record<LabelColor, string> = {
  red: "Red", orange: "Orange", yellow: "Yellow", green: "Green", blue: "Blue", purple: "Purple", gray: "Gray",
};
/** CSS colour of a label (system colours, adjusted for Dark Mode by CSS variables). */
export const labelCSS = (c: LabelColor): string => `var(--label-${c})`;

export const MAX_NAME = 32;

const key = (path: string) => path.replace(/[\\/]+$/, "");

export function normalizedName(raw: string): string {
  return raw.split(/\s+/).filter(Boolean).join(" ").slice(0, MAX_NAME);
}

export const allLabels = (): LabelDef[] => settings.get("labels");

export function labelsFor(path: string): LabelDef[] {
  const ids = settings.get("labelAssignments")[key(path)] ?? [];
  return ids.map((id) => allLabels().find((l) => l.id === id)).filter((l): l is LabelDef => !!l);
}

export function isNameAvailable(name: string, excluding?: string): boolean {
  const n = normalizedName(name);
  if (!n) return false;
  const same = (a: string, b: string) => a.localeCompare(b, undefined, { sensitivity: "base" }) === 0;
  return !allLabels().some((l) => l.id !== excluding && same(l.name, n));
}

/** A colour no label uses yet (else the palette cycles). */
export function nextColor(): LabelColor {
  const used = new Set(allLabels().map((l) => l.color));
  return LABEL_COLORS.find((c) => !used.has(c)) ?? LABEL_COLORS[allLabels().length % LABEL_COLORS.length]!;
}

export function createLabel(name: string, color: LabelColor, assignTo?: string): LabelDef | null {
  const n = normalizedName(name);
  if (!isNameAvailable(n)) return null;
  const label: LabelDef = { id: newId(), name: n, color };
  settings.update("labels", (list) => list.push(label));
  if (assignTo) toggle(label.id, assignTo, true);
  return label;
}

export function editLabel(id: string, name: string, color: LabelColor): boolean {
  const n = normalizedName(name);
  if (!isNameAvailable(n, id)) return false;
  settings.update("labels", (list) => {
    const l = list.find((x) => x.id === id);
    if (l) { l.name = n; l.color = color; }
  });
  for (const path of Object.keys(settings.get("labelAssignments"))) void syncBackend(path);
  return true;
}

export function deleteLabel(id: string): void {
  settings.update("labels", (list) => { const i = list.findIndex((l) => l.id === id); if (i >= 0) list.splice(i, 1); });
  const touched: string[] = [];
  settings.update("labelAssignments", (map) => {
    for (const [p, ids] of Object.entries(map)) {
      if (!ids.includes(id)) continue;
      const rest = ids.filter((x) => x !== id);
      if (rest.length) map[p] = rest; else delete map[p];
      touched.push(p);
    }
  });
  for (const p of touched) void syncBackend(p);
}

export function isAssigned(id: string, path: string): boolean {
  return (settings.get("labelAssignments")[key(path)] ?? []).includes(id);
}

/** Assigns or removes a label (flips it when `on` is omitted). */
export function toggle(id: string, path: string, on?: boolean): void {
  const k = key(path);
  settings.update("labelAssignments", (map) => {
    const ids = map[k] ?? [];
    const has = ids.includes(id);
    const want = on ?? !has;
    if (want && !has) map[k] = [...ids, id];
    else if (!want && has) {
      const rest = ids.filter((x) => x !== id);
      if (rest.length) map[k] = rest; else delete map[k];
    }
  });
  void syncBackend(k);
}

async function syncBackend(path: string): Promise<void> {
  const first = labelsFor(path)[0];
  try {
    await ipc.setProjectLabel(path, first?.color);
  } catch {
    /* recents not essential */
  }
}

function newId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
  }
}

export const colorTitle = (c: LabelColor): string => t(COLOR_NAMES[c]);

// App-wide preferences (port of AppSettings.swift), persisted through `load_settings` / `save_settings` as one JSON
// object in the OS config dir. Views subscribe to the keys they show, so a change re-renders only those.
import * as ipc from "../ipc";
import type { Engine } from "../ipc";

export type SyncTrigger = "doubleClick" | "commandClick";

export interface LabelDef { id: string; name: string; color: LabelColor }
export type LabelColor = "red" | "orange" | "yellow" | "green" | "blue" | "purple" | "gray";

/** Per-project memory (engine override, main-file override, open tabs). */
export interface ProjectMemory { engine?: Engine; mainFile?: string; tabs?: string[]; current?: string; sidebar?: "files" | "outline" }

export interface AppSettingsData {
  // General
  defaultEngine: Engine;
  liveCompile: boolean;
  /** Seconds of inactivity before a live compile (0.5 … 5). */
  liveDelay: number;
  compileOnSave: boolean;
  syncAfterBuild: boolean;
  pdfSyncTrigger: SyncTrigger;
  followCursorInPDF: boolean;
  // Editor
  fontSize: number;
  lineHeight: number;
  showLineNumbers: boolean;
  wrapLines: boolean;
  vimMode: boolean;
  indentWidth: number;
  spellChecking: boolean;
  // Window layout
  showPreview: boolean;
  showIssues: boolean;
  sidebarVisible: boolean;
  sidebarWidth: number;
  /** Editor share of the editor | PDF split (0…1). */
  splitRatio: number;
  issuesHeight: number;
  // Shortcuts: command id → combo string ("Ctrl+Shift+P") or "" for none. Only customizations are stored.
  shortcuts: Record<string, string>;
  // New project
  author: string;
  newProjectLocation: string;
  // Labels (named, colored; the backend stores the first label's colour so labeled projects stay in recents)
  labels: LabelDef[];
  labelAssignments: Record<string, string[]>;
  /** Recents hidden by “Clear” (path → lastOpened at the time). */
  hiddenRecents: Record<string, number>;
  projects: Record<string, ProjectMemory>;
}

export const DEFAULTS: AppSettingsData = {
  defaultEngine: "pdflatex",
  liveCompile: true,
  liveDelay: 1.0,
  compileOnSave: true,
  syncAfterBuild: false,
  pdfSyncTrigger: "doubleClick",
  followCursorInPDF: false,
  fontSize: 13,
  lineHeight: 1.25,
  showLineNumbers: true,
  wrapLines: true,
  vimMode: false,
  indentWidth: 2,
  spellChecking: false,
  showPreview: true,
  showIssues: false,
  sidebarVisible: true,
  sidebarWidth: 260,
  splitRatio: 0.52,
  issuesHeight: 230,
  shortcuts: {},
  author: "",
  newProjectLocation: "",
  labels: [],
  labelAssignments: {},
  hiddenRecents: {},
  projects: {},
};

type Key = keyof AppSettingsData;
type Listener = (key: Key) => void;

class SettingsStore {
  private data: AppSettingsData = structuredClone(DEFAULTS);
  private listeners = new Set<Listener>();
  private saveTimer = 0;

  async load(): Promise<void> {
    try {
      const raw = await ipc.loadSettings();
      this.data = sanitize({ ...structuredClone(DEFAULTS), ...(raw as Partial<AppSettingsData>) });
    } catch {
      this.data = structuredClone(DEFAULTS);
    }
  }

  get<K extends Key>(key: K): AppSettingsData[K] {
    return this.data[key];
  }

  get all(): Readonly<AppSettingsData> {
    return this.data;
  }

  set<K extends Key>(key: K, value: AppSettingsData[K]): void {
    if (this.data[key] === value && typeof value !== "object") return;
    this.data[key] = value;
    this.data = sanitize(this.data);
    this.scheduleSave();
    for (const l of this.listeners) l(key);
  }

  /** Mutates an object-valued setting in place and notifies. */
  update<K extends Key>(key: K, fn: (value: AppSettingsData[K]) => void): void {
    fn(this.data[key]);
    this.scheduleSave();
    for (const l of this.listeners) l(key);
  }

  project(root: string): ProjectMemory {
    return (this.data.projects[root] ??= {});
  }

  updateProject(root: string, fn: (m: ProjectMemory) => void): void {
    fn(this.project(root));
    this.scheduleSave();
  }

  /** Subscribes to changes; `keys` limits the callback to those keys. Returns the unsubscribe function. */
  subscribe(fn: Listener, keys?: Key[]): () => void {
    const wrapped: Listener = keys ? (k) => { if (keys.includes(k)) fn(k); } : fn;
    this.listeners.add(wrapped);
    return () => this.listeners.delete(wrapped);
  }

  private scheduleSave(): void {
    clearTimeout(this.saveTimer);
    this.saveTimer = window.setTimeout(() => void this.flush(), 300);
  }

  async flush(): Promise<void> {
    clearTimeout(this.saveTimer);
    try {
      await ipc.saveSettings(this.data as unknown as ipc.Settings);
    } catch (e) {
      console.warn("Could not save settings", e);
    }
  }
}

const clamp = (v: unknown, lo: number, hi: number, fallback: number) =>
  typeof v === "number" && Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : fallback;

function sanitize(d: AppSettingsData): AppSettingsData {
  const engines: Engine[] = ["pdflatex", "xelatex", "lualatex"];
  if (!engines.includes(d.defaultEngine)) d.defaultEngine = "pdflatex";
  d.liveDelay = clamp(d.liveDelay, 0.5, 5, 1);
  d.fontSize = clamp(d.fontSize, 8, 40, 13);
  d.lineHeight = clamp(d.lineHeight, 1, 2.2, 1.25);
  d.indentWidth = Math.round(clamp(d.indentWidth, 1, 8, 2));
  d.sidebarWidth = clamp(d.sidebarWidth, 180, 440, 260);
  d.splitRatio = clamp(d.splitRatio, 0.15, 0.85, 0.52);
  d.issuesHeight = clamp(d.issuesHeight, 110, 600, 230);
  if (d.pdfSyncTrigger !== "commandClick") d.pdfSyncTrigger = "doubleClick";
  for (const k of ["shortcuts", "labelAssignments", "hiddenRecents", "projects"] as const) {
    if (!d[k] || typeof d[k] !== "object" || Array.isArray(d[k])) (d as unknown as Record<string, unknown>)[k] = {};
  }
  if (!Array.isArray(d.labels)) d.labels = [];
  return d;
}

/** The single settings store. */
export const settings = new SettingsStore();

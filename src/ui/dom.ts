// Tiny DOM helpers (no framework). `h` builds elements; everything else is plain DOM.

export type Child = Node | string | number | null | undefined | false | Child[];

export interface Props {
  class?: string;
  text?: string;
  title?: string;
  html?: string;
  style?: Partial<CSSStyleDeclaration> | string;
  attrs?: Record<string, string | number | boolean | undefined | null>;
  data?: Record<string, string>;
  on?: { [K in keyof HTMLElementEventMap]?: (ev: HTMLElementEventMap[K]) => void };
  [key: string]: unknown;
}

const RESERVED = new Set(["class", "text", "title", "html", "style", "attrs", "data", "on"]);

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, props?: Props | null, ...children: Child[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (props) {
    if (props.class) el.className = props.class;
    if (props.title) el.title = props.title;
    if (props.text !== undefined) el.textContent = props.text;
    if (props.html !== undefined) el.innerHTML = props.html;
    if (typeof props.style === "string") el.style.cssText = props.style;
    else if (props.style) Object.assign(el.style, props.style);
    if (props.attrs) {
      for (const [k, v] of Object.entries(props.attrs)) {
        if (v === undefined || v === null || v === false) continue;
        el.setAttribute(k, v === true ? "" : String(v));
      }
    }
    if (props.data) for (const [k, v] of Object.entries(props.data)) el.dataset[k] = v;
    if (props.on) {
      for (const [type, fn] of Object.entries(props.on)) {
        if (fn) el.addEventListener(type, fn as EventListener);
      }
    }
    for (const [k, v] of Object.entries(props)) {
      if (RESERVED.has(k) || v === undefined) continue;
      (el as unknown as Record<string, unknown>)[k] = v;
    }
  }
  append(el, children);
  return el;
}

export function append(parent: Node, children: Child[]): void {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    if (Array.isArray(c)) append(parent, c);
    else parent.appendChild(typeof c === "string" || typeof c === "number" ? document.createTextNode(String(c)) : c);
  }
}

/** Replaces all children of `el`. */
export function replaceChildren(el: Element, ...children: Child[]): void {
  el.textContent = "";
  append(el, children);
}

/** Sets text only when it changed (avoids needless layout work). */
export function setText(el: Element, text: string): void {
  if (el.textContent !== text) el.textContent = text;
}

/** Toggles a class only when needed. */
export function toggleClass(el: Element, name: string, on: boolean): void {
  if (el.classList.contains(name) !== on) el.classList.toggle(name, on);
}

/** Coalesces calls to one per animation frame. */
export function rafThrottle(fn: () => void): () => void {
  let pending = false;
  return () => {
    if (pending) return;
    pending = true;
    requestAnimationFrame(() => {
      pending = false;
      fn();
    });
  };
}

export const reducedMotion = (): boolean => matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Base name of a path (handles / and \). */
export function basename(path: string): string {
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return i >= 0 ? path.slice(i + 1) : path;
}

export function dirname(path: string): string {
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return i > 0 ? path.slice(0, i) : path;
}

export function extname(path: string): string {
  const b = basename(path);
  const i = b.lastIndexOf(".");
  return i > 0 ? b.slice(i + 1).toLowerCase() : "";
}

/** `path` relative to `root` with `/` separators, or the path itself when outside. */
export function relativePath(path: string, root: string): string {
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");
  const p = norm(path);
  const r = norm(root);
  return p.startsWith(r + "/") ? p.slice(r.length + 1) : p;
}

/** Joins a directory and a relative name with the directory's separator. */
export function joinPath(dir: string, name: string): string {
  const sep = dir.includes("\\") && !dir.includes("/") ? "\\" : "/";
  return dir.replace(/[\\/]+$/, "") + sep + name;
}

/** Home-relative display (`~/Documents`) when the home directory is known. */
export function abbreviateHome(path: string, home?: string): string {
  if (home && path.startsWith(home)) return "~" + path.slice(home.length);
  return path.replace(/^\/home\/[^/]+/, "~").replace(/^\/Users\/[^/]+/, "~");
}

/** Wraps a path in left-to-right marks, so `direction: rtl` (ellipsis at the start) keeps its slashes in place. */
export const ltr = (text: string): string => `\u200E${text}\u200E`;

export const isMac = /Mac/.test(navigator.platform);

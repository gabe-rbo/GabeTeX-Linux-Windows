// Localization. Keys are the Mac app's English source strings (Localizable.strings), so the Brazilian Portuguese
// table converted from the Mac app hits directly. English needs no table: the key is the text.
// Placeholders follow Apple's format: `%@`, `%lld`, `%d`, and positional `%1$@`.

type Table = Record<string, string>;

let table: Table | null = null;
let lang = "en";

/** Picks the language from `navigator.languages` and loads its table (lazy: English loads nothing). */
export async function initI18n(): Promise<void> {
  const list = navigator.languages?.length ? navigator.languages : [navigator.language || "en"];
  for (const raw of list) {
    const l = raw.toLowerCase();
    if (l.startsWith("pt")) { lang = "pt-BR"; break; }
    if (l.startsWith("en")) break;
  }
  if (lang === "pt-BR") {
    try {
      table = (await import("./pt-BR.json")).default as Table;
    } catch {
      table = null;
      lang = "en";
    }
  }
  document.documentElement.lang = lang;
}

/** The active language tag ("en" or "pt-BR"). */
export const language = (): string => lang;

/** Translates `key` and fills its placeholders with `args` in order (or by `%n$` position). */
export function t(key: string, ...args: Array<string | number>): string {
  const extra = EXTRA[key];
  const text = platformize(extra ? (lang === "pt-BR" ? extra : key) : table?.[key] ?? key);
  if (args.length === 0 && text.indexOf("%") < 0) return text;
  let next = 0;
  return text.replace(/%(?:(\d+)\$)?(@|lld|ld|d|%)/g, (_m, pos: string | undefined, spec: string) => {
    if (spec === "%") return "%";
    const index = pos ? Number(pos) - 1 : next++;
    const value = args[index];
    if (value === undefined) return "";
    return typeof value === "number" ? formatInt(value) : value;
  });
}

/**
 * Strings that differ from the Mac app (Linux/Windows wording) with their pt-BR translation. English is the key.
 */
const EXTRA: Record<string, string> = {
  "Cut": "Recortar",
  "Copy": "Copiar",
  "Paste": "Colar",
  "Select All": "Selecionar Tudo",
  "Go to Definition": "Ir para a Definição",
  "Open in Default App": "Abrir no Aplicativo Padrão",
  "Reveal in File Manager": "Mostrar no Gerenciador de Arquivos",
  "Show PDF in File Manager": "Mostrar PDF no Gerenciador de Arquivos",
  "Install TeX Live or MiKTeX to compile documents.": "Instale o TeX Live ou o MiKTeX para compilar documentos.",
  "Get TeX Live": "Obter TeX Live",
  "Edit Label…": "Editar Rótulo…",
  "PDF Only": "Somente PDF",
  "Show the editor again": "Mostrar o editor novamente",
  "Maximize the PDF preview": "Maximizar a pré-visualização do PDF",
  "Folder": "Pasta",
  "The PDF was updated": "O PDF foi atualizado",
  "Saved": "Salvo",
  "Could not reveal the file": "Não foi possível mostrar o arquivo",
  "Language follows the system.": "O idioma acompanha o sistema.",
  "Appearance follows the system (Light or Dark).": "A aparência acompanha o sistema (Claro ou Escuro).",
  "“%@” was reloaded": "“%@” foi recarregado",
  "Installed %@": "%@ instalado",
  "Compile the document first.": "Compile o documento primeiro.",
  "This position was not found in the PDF.": "Esta posição não foi encontrada no PDF.",
  "This position was not found in the source.": "Esta posição não foi encontrada no código-fonte.",
  "Hide Sidebar": "Ocultar Barra Lateral",
  "Show Sidebar": "Mostrar Barra Lateral",
  "Close": "Fechar",
};

const MOD_NAMES: Record<string, string> = { "⌃": "Ctrl", "⌥": "Alt", "⇧": "Shift", "⌘": "Ctrl" };
const MOD_ORDER = ["Ctrl", "Alt", "Shift"];
/** Mac shortcut glyphs in prose (“(⇧⌘M)”, “⌘-click”) become Linux/Windows spelling (“(Ctrl+Shift+M)”, “Ctrl-click”). */
export function platformize(text: string): string {
  if (!/[⌃⌥⇧⌘]/.test(text)) return text;
  return text.replace(/([⌃⌥⇧⌘]+)(-click|[^\s)]+)?/g, (_m, mods: string, key: string | undefined) => {
    const names = [...new Set([...mods].map((c) => MOD_NAMES[c] ?? c))].sort((a, b) => MOD_ORDER.indexOf(a) - MOD_ORDER.indexOf(b));
    if (!key) return names.join("+");
    if (key.startsWith("-")) return names.join("+") + key;
    return [...names, keyName(key)].join("+");
  });
}
const KEY_NAMES: Record<string, string> = { "↩": "Enter", "⌫": "Backspace", "⎋": "Esc", "⇥": "Tab", "↑": "Up", "↓": "Down", "←": "Left", "→": "Right" };
const keyName = (k: string): string => KEY_NAMES[k] ?? (k.length === 1 ? k.toUpperCase() : k);

const intFormat = new Map<string, Intl.NumberFormat>();
/** Integer with the locale's grouping (1,234 / 1.234). */
export function formatInt(n: number): string {
  let f = intFormat.get(lang);
  if (!f) { f = new Intl.NumberFormat(lang, { maximumFractionDigits: 0 }); intFormat.set(lang, f); }
  return f.format(n);
}

/** A decimal with exactly `digits` fraction digits (0,2 / 0.2). */
export function formatDecimal(n: number, digits = 1): string {
  return new Intl.NumberFormat(lang, { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(n);
}

/** “a”, “a and b”, “a, b, and c” in the active language. */
export function formatList(items: string[]): string {
  try {
    return new Intl.ListFormat(lang, { style: "long", type: "conjunction" }).format(items);
  } catch {
    return items.join(", ");
  }
}

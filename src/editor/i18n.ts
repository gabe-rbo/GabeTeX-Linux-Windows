// Tiny translator for the editor's few UI strings. Keys are the Mac app's English source strings, so the
// shared `src/i18n/*.json` tables hit. Follows `navigator.language` (pt → Brazilian Portuguese).
import ptBR from "../i18n/pt-BR.json";

const table: Record<string, string> | null =
  typeof navigator !== "undefined" && /^pt\b/i.test(navigator.language || "")
    ? (ptBR as Record<string, string>)
    : null;

export const isPortuguese = table !== null;

/** Translates `key` and substitutes `%@`, `%lld`, `%d` (and positional `%1$@`) with `args`. */
export function t(key: string, ...args: (string | number)[]): string {
  const s = table?.[key] ?? key;
  if (args.length === 0) return s;
  let next = 0;
  return s.replace(/%(?:(\d+)\$)?(?:@|lld|ld|d)/g, (_m, pos: string | undefined) => {
    const index = pos ? Number(pos) - 1 : next++;
    return String(args[index] ?? "");
  });
}

/** CodeMirror phrases (search panel) in Brazilian Portuguese. */
export const cmPhrasesPT: Record<string, string> = {
  Find: "Buscar",
  Replace: "Substituir",
  next: "próximo",
  previous: "anterior",
  all: "todos",
  "match case": "maiúsc./minúsc.",
  regexp: "expressão regular",
  "by word": "palavra inteira",
  replace: "substituir",
  "replace all": "substituir todos",
  close: "fechar",
  "current match": "ocorrência atual",
  "on line": "na linha",
  "replaced $ matches": "$ ocorrências substituídas",
  "replaced match on line $": "ocorrência substituída na linha $",
  "Go to line": "Ir para a linha",
  go: "ir",
};

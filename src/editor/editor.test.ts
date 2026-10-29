import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { ensureSyntaxTree } from "@codemirror/language";
import { latexLanguage } from "./latex";
import { expandSnippet, parseSnippet } from "./snippet";
import { commandPrefixStart, enclosingArgument, environmentOpenedAtEndOf, isInCommandArgument } from "./scan";
import { isMathAt } from "./editing";
import { diagnosticRange } from "./diagnostics";
import { includeTargetAt, labelKeyAt } from "./mouse";

function tokens(doc: string): [string, string][] {
  const state = EditorState.create({ doc, extensions: latexLanguage });
  const tree = ensureSyntaxTree(state, doc.length, 5000)!;
  const out: [string, string][] = [];
  tree.iterate({
    enter(n) {
      if (n.type.isTop) return;
      const kind = n.name === "mathOpen" || n.name === "mathClose" ? "mathDelimiter" : n.name;
      out.push([kind, doc.slice(n.from, n.to)]);
    },
  });
  return out;
}

describe("latex tokenizer", () => {
  it("classifies commands, structure, environments and references", () => {
    const t = tokens("\\section{Intro}\\label{sec:a} \\foo");
    expect(t[0]).toEqual(["structureCommand", "\\section"]);
    expect(t).toContainEqual(["referenceArgument", "sec:a"]);
    expect(t).toContainEqual(["command", "\\label"]);
    expect(t[t.length - 1]).toEqual(["command", "\\foo"]);
  });

  it("handles \\begin{env} and math environments across lines", () => {
    const t = tokens("\\begin{equation}\na+\\alpha\n\\end{equation} x");
    expect(t[0]).toEqual(["mathDelimiter", "\\begin"]);
    expect(t).toContainEqual(["environmentName", "equation"]);
    expect(t).toContainEqual(["math", "a+"]);
    expect(t).toContainEqual(["mathCommand", "\\alpha"]);
    expect(t.filter((x) => x[0] === "mathDelimiter").map((x) => x[1])).toEqual(["\\begin", "\\end"]);
  });

  it("tracks inline and display math, comments and magic comments", () => {
    const t = tokens("% !TEX root = main.tex\n$x^2$ and \\[ \\frac{a}{b} \\] % note");
    expect(t[0]).toEqual(["magicComment", "% !TEX root = main.tex"]);
    expect(t).toContainEqual(["specialCharacter", "^"]);
    expect(t).toContainEqual(["mathCommand", "\\frac"]);
    expect(t[t.length - 1]).toEqual(["comment", "% note"]);
    expect(t.some(([k, s]) => k === "math" && s.includes("and"))).toBe(false);
  });

  it("reference arguments after optional brackets, and verbatim", () => {
    const t = tokens("\\usepackage[utf8]{inputenc}\n\\verb|\\x{| \\url{a%b}\n\\begin{verbatim}\n\\foo $\n\\end{verbatim}");
    expect(t).toContainEqual(["bracket", "["]);
    expect(t).toContainEqual(["referenceArgument", "inputenc"]);
    expect(t).toContainEqual(["verbatim", "|\\x{|"]);
    expect(t).toContainEqual(["verbatim", "a%b"]);
    expect(t).toContainEqual(["verbatim", "\\foo $"]);
    expect(t[t.length - 1]).toEqual(["brace", "}"]);
  });

  it("a blank line ends inline math", () => {
    const t = tokens("$a\n\nb");
    expect(t.some(([k, s]) => k === "math" && s === "b")).toBe(false);
  });
});

describe("snippets", () => {
  it("parses stops, defaults with braces, mirrors and final", () => {
    const s = parseSnippet("\\begin{${1:itemize}}\n\t\\item ${2:\\textbf{x}}$0\n\\end{$1}");
    expect(s.text).toBe("\\begin{itemize}\n\t\\item \\textbf{x}\n\\end{itemize}");
    expect(s.stops.map((r) => s.text.slice(r.from, r.to))).toEqual(["itemize", "\\textbf{x}"]);
    expect(s.mirrors[0]!.map((r) => s.text.slice(r.from, r.to))).toEqual(["itemize"]);
    expect(s.final).toBe(s.text.indexOf("\n\\end"));
    expect(parseSnippet("\\$ $x").text).toBe("\\$ $x");
  });
  it("expands indentation", () => {
    const e = expandSnippet(parseSnippet("a{\n\t${1:b}\n}"), "  ", "  ");
    expect(e.text).toBe("a{\n    b\n  }");
    expect(e.text.slice(e.stops[0]!.from, e.stops[0]!.to)).toBe("b");
  });
});

describe("scanning", () => {
  it("environment at end of head", () => {
    expect(environmentOpenedAtEndOf("  \\begin{tabular}{cc}")).toBe("tabular");
    expect(environmentOpenedAtEndOf("% \\begin{x}")).toBeNull();
  });
  it("completion triggers", () => {
    expect(commandPrefixStart("a \\sec", 6)).toBe(2);
    expect(commandPrefixStart("a \\", 3)).toBe(-1);
    expect(isInCommandArgument("\\ref{eq:", 8)).toBe(true);
    expect(isInCommandArgument("{eq:", 4)).toBe(false);
    expect(enclosingArgument("\\cref[x]{a,b}", 11)?.command).toBe("cref");
  });
  it("math detection", () => {
    const doc = "text $a + b$ more";
    const state = EditorState.create({ doc, extensions: latexLanguage });
    expect(isMathAt(state, doc.indexOf("+"))).toBe(true);
    expect(isMathAt(state, doc.indexOf("more"))).toBe(false);
  });
  it("diagnostic ranges and navigation targets", () => {
    const state = EditorState.create({ doc: "x\n  \\foo bar  \n\\input{chap/one} \\ref{a, b}" });
    expect(diagnosticRange({ severity: "error", message: "", path: "", line: 2, source: "latex" }, state.doc))
      .toEqual([4, 12]);
    expect(diagnosticRange({ severity: "error", message: "", path: "", line: 2, column: 3, source: "latex" }, state.doc))
      .toEqual([4, 8]);
    const l3 = state.doc.line(3).from;
    expect(includeTargetAt(state, l3 + 9)).toBe("chap/one");
    expect(labelKeyAt(state, l3 + state.doc.line(3).text.indexOf("b}"))).toBe("b");
  });
});

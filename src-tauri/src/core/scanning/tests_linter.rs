//! Port of `LinterChecks.swift`.

use crate::core::log_parser::{LogDiagnostic, Severity, Source};
use crate::core::scanner::tests_common::url;
use crate::core::scanner::walker::lint_text;

fn lint(text: &str) -> Vec<LogDiagnostic> {
    lint_text(text, &url("main.tex"))
}

fn describe(diagnostics: &[LogDiagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|d| {
            let sev = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::BadBox => "badbox",
                Severity::Info => "info",
            };
            format!("{sev}@{}:{}+{}", d.line.map(|v| v as i64).unwrap_or(-1), d.column.map(|v| v as i64).unwrap_or(-1), d.length.map(|v| v as i64).unwrap_or(-1))
        })
        .collect()
}

fn messages(diagnostics: &[LogDiagnostic]) -> Vec<String> {
    diagnostics.iter().map(|d| format!("{}:{} {}", d.line.unwrap_or(0), d.column.unwrap_or(0), d.message)).collect()
}

#[test]
fn silent_on_valid_documents() {
    let valid = "\\documentclass[11pt]{article}\n\
        \\usepackage{amsmath,amssymb}\n\
        \\usepackage{listings}\n\
        \\newcommand{\\bra}[1]{\\left\\langle #1\\right|}\n\
        \\newcommand{\\beginenv}{\\begin{equation}}\n\
        \\newcommand{\\endenv}{\\end{equation}}\n\
        \\newenvironment{myenv}{\\begin{center}}{\\end{center}}\n\
        \\def\\half{\\frac{1}{2}}\n\
        \\DeclareDocumentCommand{\\pair}{m m}{\\left\\{#1,#2\\right\\}}\n\
        % A comment with unbalanced { and \\begin{foo} and $\n\
        \\begin{document}\n\
        \\title{A {nested {braces}} title}\n\
        \\maketitle\n\
        \\section{Sets \\& $\\{x \\mid x > 0\\}$}\n\
        We write \\{a, b\\} for sets, \\$5 for dollars, 50\\% rates, \\verb|{ unbalanced| and \\verb+\\begin{x}+.\n\
        Inline math $a$ and $b$, plus \\(c\\) and \\[ \\left\\{ x \\right\\} \\].\n\
        $$ \\sum_{i=1}^n i = \\frac{n(n+1)}{2} $$\n\
        \\begin{equation}\n\
        \x20 \\left\\{ \\begin{matrix} a & b \\\\ c & d \\end{matrix} \\right\\}\n\
        \x20 \\text{if } x \\in \\{ 1, 2 \\}\n\
        \\end{equation}\n\
        \\begin{align*}\n\
        \x20 f(x) &= \\left\\lbrace x \\right\\rbrace \\\\[2pt]\n\
        \x20 g(x) &= \\text{a $y$ inside text}\n\
        \\end{align*}\n\
        \\begin{itemize}\n\
        \x20 \\item First \\begin{enumerate} \\item nested \\end{enumerate}\n\
        \x20 \\item[$\\bullet$] Second\n\
        \\end{itemize}\n\
        \\begin{verbatim}\n\
        unbalanced { and \\begin{nothing} and $ stuff \\end{itemize}\n\
        \\end{verbatim}\n\
        \\begin{lstlisting}[language=C]\n\
        int main() { return 0;\n\
        \\end{lstlisting}\n\
        \\begin{tabular}{|c|c|}\n\
        \x20 \\hline a & b \\\\ \\hline\n\
        \\end{tabular}\n\
        See \\url{http://example.org/a%20b#c} and \\href{http://x.org/%7Ey}{a link}.\n\
        \\iffalse\n\
        \\begin{broken}{\n\
        \\fi\n\
        Text with a footnote\\footnote{An {inner} note.} and a math group $a_{i}^{2}$.\n\
        \n\
        New paragraph with \\texttt{\\{} and \\texttt{\\}} and a lone \\% sign.\n\
        \\end{document}\n\
        garbage after the end } { \\begin{x}";
    assert_eq!(messages(&lint(valid)), Vec::<String>::new(), "realistic valid document");
    assert_eq!(lint("").len(), 0);
    assert_eq!(lint("plain text only").len(), 0);

    // Section files (no \begin{document}) may open and close environments of the including file.
    assert_eq!(lint("\\section{S}\n\\begin{itemize}\n\\item a\n").len(), 0, "unclosed \\begin in a section file");
    assert_eq!(lint("\\item b\n\\end{itemize}\n\\section{S}\n").len(), 0, "stray \\end in a section file");
    assert_eq!(lint("\\begin{document}\n\\end{document}\n").len(), 0);
    assert_eq!(messages(&lint("a % } stray in a comment\n\\% \\{ \\} \\$ \\\\\n")), Vec::<String>::new());
    assert_eq!(lint("\\catcode`\\{=12 } unbalanced on purpose").len(), 0, "catcode changes disable brace checks");
    assert_eq!(lint("$a$ and $$b$$ and $c$\n\n$d$").len(), 0);
    assert_eq!(lint("\\[ a \\]\\( b \\)").len(), 0);
    assert_eq!(lint("\\begin{equation} $ \\end{equation}").len(), 0, "$ inside a math environment is inert");
    assert_eq!(lint("\\newcommand{\\foo}{\\begin{itemize}}\n\\newcommand{\\bar}{\\end{itemize}}\n").len(), 0, "definition bodies are not analysed");
    assert_eq!(lint("\\begin{document}\n\\iffalse\n}\n\\fi\n\\end{document}").len(), 0);
    assert_eq!(lint("\\begin{comment}\n}{\n\\end{comment}\n").len(), 0);
    assert_eq!(lint("\\section{ação — é 😀}\n$é$\n").len(), 0);
    // Hook commands carry code that may hold half an environment.
    let hooks = "\\documentclass{article}\n\
        \\AtEndEnvironment{abstract}{\\end{center}}\n\
        \\AtBeginEnvironment{abstract}{\\begin{center}}\n\
        \\BeforeBeginEnvironment{tikzpicture}{\\begin{center}}\n\
        \\AfterEndEnvironment{tikzpicture}{\\end{center}}\n\
        \\AtBeginDocument{\\begin{center}}\n\
        \\AtEndDocument{\\end{center}}\n\
        \\AddToHook{begindocument}[label]{\\begin{center}}\n\
        \\appto\\foo{\\end{center}}\n\
        \\begin{document}\n\
        x\n\
        \\end{document}";
    assert_eq!(messages(&lint(hooks)), Vec::<String>::new(), "hook arguments are not analysed");
    // A kitchen sink of valid constructs.
    let sink = "\\documentclass{beamer}\n\
        \\begin{document}\n\
        \\begin{frame}[fragile]{Title \\& $\\alpha$}\n\
        \x20 \\begin{lstlisting}[caption={A {nested} caption}]\n\
        \x20 if (x) { return; \\end{itemize}\n\
        \x20 \\end{lstlisting}\n\
        \x20 \\only<2->{Text with \\textbf{bold}}\n\
        \\end{frame}\n\
        \\begin{figure}[h]\\centering\\includegraphics[width=.5\\textwidth]{a}\\caption{A {B}}\\label{f}\\end{figure}\n\
        \\begin{thebibliography}{99}\\bibitem{a} X.\\end{thebibliography}\n\
        \\begin{tabular}{@{}l@{}} a \\\\[1ex] b \\end{tabular}\n\
        \\item[\\textbf{a}] \\\"{o} \\'{e} \\^{o} {\\\"o} \\ldots{} 100\\% \\# \\_ \\& $\\{$ $\\}$ \\verb|\\| \\verb|{|\n\
        \\renewcommand{\\labelenumi}{(\\alph{enumi})}\n\
        \\setlength{\\parindent}{0pt}\n\
        \\mintinline{python}{x = {1}} \\lstinline!{! \\lstinline[language=C]|}|\n\
        \\begin{minted}[linenos]{python}\n\
        d = {\"a\": [1, 2}\n\
        \\end{minted}\n\
        \\expandafter\\def\\csname foo\\endcsname{\\begin{center}}\n\
        \\end{document}";
    assert_eq!(messages(&lint(sink)), Vec::<String>::new(), "kitchen sink");
}

#[test]
fn braces() {
    let stray = lint("\\documentclass{article}\n\\begin{document}\nHello } world\n\\end{document}\n");
    assert_eq!(describe(&stray), ["error@3:7+1"]);
    assert_eq!(stray[0].source, Source::Structure);
    assert_eq!(stray[0].path.as_deref(), Some(url("main.tex").as_str()));
    assert!(stray[0].message.contains('}'));

    let missing = lint("\\documentclass{article}\n\\begin{document}\n\\textbf{bold text\nmore\n\\end{document}\n");
    assert_eq!(describe(&missing), ["error@3:8+1"], "reported at the opening brace");
    assert_eq!(missing[0].source, Source::Structure);

    // Missing brace in a section file (EOF is the end of the file there).
    assert_eq!(describe(&lint("\\section{Title\nbody\n")), ["error@1:9+1"]);
    // Two problems come back sorted by position.
    assert_eq!(describe(&lint("a } b\n\\textbf{c\n}}\n")), ["error@1:3+1", "error@3:2+1"]);
    // Nested missing brace: both opening braces are reported, outermost first.
    assert_eq!(describe(&lint("{ { }\n")), ["error@1:1+1"]);
    assert_eq!(describe(&lint("{{\n")), ["error@1:1+1", "error@1:2+1"]);
    // Columns count UTF-16 units; CRLF does not shift lines.
    assert_eq!(describe(&lint("é😀 }")), ["error@1:5+1"]);
    assert_eq!(describe(&lint("a\r\nb\r\nc }\r\n")), ["error@3:3+1"]);
    assert_eq!(describe(&lint("a\rb\rc }\r")), ["error@3:3+1"]);
    // Escaped / commented / verbatim braces do not count.
    assert_eq!(lint("\\{ \\} \\verb|}| % }\n").len(), 0);
    assert_eq!(describe(&lint("\\verb|}| }")), ["error@1:10+1"]);
}

#[test]
fn environments() {
    let mismatch = lint("\\begin{document}\n\\begin{itemize}\n\\item a\n\\end{enumerate}\n\\end{document}\n");
    assert_eq!(describe(&mismatch), ["error@4:1+15"]);
    assert!(mismatch[0].message.contains("itemize") && mismatch[0].message.contains("enumerate"), "mismatch message names both environments: {}", mismatch[0].message);
    assert!(mismatch[0].message.contains('2'), "mismatch message mentions the \\begin line");

    // Mismatch is reported in section files as well.
    assert_eq!(describe(&lint("\\begin{a}\n x\n\\end{b}\n")), ["error@3:1+7"]);
    // A skipped level: \begin{a}\begin{b}\end{a} is one mismatch and recovers.
    assert_eq!(describe(&lint("\\begin{a}\\begin{b}\n\\end{a}\n")), ["error@2:1+7"]);
    // Unclosed environments at the end of a document.
    let unclosed = lint("\\begin{document}\n\\begin{itemize}\n\\item a\n");
    assert_eq!(describe(&unclosed), ["error@1:1+16", "error@2:1+15"], "document and itemize never closed");
    assert!(unclosed.last().unwrap().message.contains("itemize"));
    // An \end{document} that closes an open itemize is a mismatch on that line.
    assert_eq!(describe(&lint("\\begin{document}\n\\begin{itemize}\n\\item a\n\\end{document}\n")), ["error@4:1+14"]);
    // Stray \end inside a document: exactly one error, document still closes.
    let stray_end = lint("\\begin{document}\nx\n\\end{itemize}\n\\end{document}\n");
    assert_eq!(describe(&stray_end), ["error@3:1+13"]);
    assert!(stray_end[0].message.contains("itemize"));
    // Stray \end after everything is closed.
    assert_eq!(describe(&lint("\\begin{document}\n\\begin{a}\n\\end{a}\n\\end{a}\n\\end{document}\n")), ["error@4:1+7"]);
    // Environments inside nested braces and with stars.
    assert_eq!(lint("\\begin{document}\\begin{align*}x\\end{align*}\\end{document}").len(), 0);
    assert_eq!(describe(&lint("\\begin{document}\\begin{align*}x\\end{align}\\end{document}")), ["error@1:32+11"]);
    // Nothing is analysed after \end{document}.
    assert_eq!(lint("\\begin{document}\n\\end{document}\n\\end{x}\n}\n").len(), 0);
    // Environment names split across lines are not analysed.
    assert_eq!(lint("\\begin{doc\nument}").len(), 0);
    // Two independent problems, sorted.
    assert_eq!(describe(&lint("\\begin{document}\nA } B\n\\begin{x}\n\\end{y}\n\\end{document}\n")), ["error@2:3+1", "error@4:1+7"]);
}

#[test]
fn inline_math() {
    let one = lint("\\begin{document}\nPrice is $5 and more text\nsecond line of the paragraph\n\nNew paragraph.\n\\end{document}\n");
    assert_eq!(describe(&one), ["warning@2:10+1"]);
    assert_eq!(one[0].source, Source::Structure);
    assert_eq!(describe(&lint("text $x + y\n")), ["warning@1:6+1"], "unclosed at end of a section file");
    assert_eq!(describe(&lint("a $x$ b $y\n\nc $z$\n")), ["warning@1:9+1"]);
    assert_eq!(lint("$x\ny$\n").len(), 0, "math may span lines inside a paragraph");
    assert_eq!(lint("$$ x\n\ny $$").len(), 0, "display math is not checked");
    assert_eq!(lint("\\(x\\)").len(), 0);
    assert_eq!(lint("\\$5 and \\$6").len(), 0);
    assert_eq!(describe(&lint("a\r\n$x\r\n\r\nb")), ["warning@2:1+1"], "CRLF paragraph break");
    assert_eq!(describe(&lint("% $\n$x\n\n% c\n")), ["warning@2:1+1"], "$ inside a comment is ignored");
    assert_eq!(lint("$x\n% comment\ny$\n").len(), 0, "a comment-only line is not a paragraph break");
}

//! Port of `StatisticsChecks.swift` (+ the performance budgets of `PerformanceChecks.swift`).

use crate::core::project_index::{build, ProjectModel};
use crate::core::scanner::models::*;
use crate::core::scanner::statistics::*;
use crate::core::scanner::tests_common::*;
use std::collections::{HashMap, HashSet};

type Files = HashMap<String, String>;

fn files(list: &[(&str, &str)]) -> Files {
    list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn reader(f: &Files) -> impl Fn(&str) -> Option<String> + '_ {
    move |p: &str| p.strip_prefix("/proj/").and_then(|x| f.get(x)).cloned()
}

fn compute_files(f: &Files) -> DocumentStatistics {
    let read = reader(f);
    let exists = |p: &str| read(p).is_some();
    compute(&url("main.tex"), &read, &exists)
}

fn comp(list: &[(&str, &str)]) -> DocumentStatistics {
    compute_files(&files(list))
}

/// A complete one-file document around `text`.
fn document(text: &str) -> String {
    format!("\\documentclass{{article}}\n\\begin{{document}}\n{text}\n\\end{{document}}\n")
}

fn body(text: &str) -> DocumentStatistics {
    comp(&[("main.tex", &document(text))])
}

fn words(text: &str) -> usize {
    body(text).totals.words()
}

fn titles(stats: &DocumentStatistics) -> Vec<String> {
    stats.sections.iter().map(|s| s.title.clone()).collect()
}

fn outline_ids(nodes: &[OutlineNode]) -> Vec<String> {
    let mut ids = Vec::new();
    for node in nodes {
        if node.entry.kind != OutlineKind::Label {
            ids.push(node.id());
        }
        ids.extend(outline_ids(node.children.as_deref().unwrap_or(&[])));
    }
    ids
}

fn model_of(f: &Files) -> ProjectModel {
    let read = reader(f);
    build(&url("main.tex"), &read, &|p| read(p).is_some())
}

fn sum_of(stats: &DocumentStatistics) -> StatisticsCounts {
    stats.sections.iter().fold(stats.front_matter.own, |acc, s| acc + s.own)
}

fn c(text_words: usize, heading_words: usize, inline_math: usize, display_math: usize) -> StatisticsCounts {
    StatisticsCounts { text_words, heading_words, inline_math, display_math }
}

#[test]
fn what_is_a_word() {
    assert_eq!(words("Hello world, this is a test."), 6, "plain prose");
    assert_eq!(words(""), 0, "empty body");
    assert_eq!(words("   \n\n  \t "), 0, "only blanks");
    assert_eq!(words("state-of-the-art design"), 2, "hyphenated word is one");
    assert_eq!(words("guarda-chuva e bem-vindo"), 3, "Portuguese compounds");
    assert_eq!(words("pages 10--20"), 3, "an en dash separates");
    assert_eq!(words("a---b"), 2, "an em dash separates");
    assert_eq!(words("a - b"), 2, "a spaced hyphen is not a word");
    assert_eq!(words("hy\\-phen"), 1, "discretionary hyphen");
    assert_eq!(words("don't stop"), 2, "apostrophe inside a word");
    assert_eq!(words("dogs' toys"), 2, "trailing apostrophe");
    assert_eq!(words("d\u{2019}água clara"), 2, "typographic apostrophe");
    assert_eq!(words("``quoted'' text"), 2, "TeX quotes are punctuation");
    assert_eq!(words("He said \"hello\" to me"), 5, "straight quotes are punctuation");
    assert_eq!(words("3.14 is pi, 1,5 too, and 1.000.000."), 7, "decimal numbers are one word");
    assert_eq!(words("In 1990."), 2, "a number before the full stop");
    assert_eq!(words("COVID-19 and 2nd"), 3, "letters and digits mix");
    assert_eq!(words("a.b"), 2, "a dot between letters separates");
    assert_eq!(words("3.a"), 2, "a dot between a digit and a letter separates");
    assert_eq!(words("one~two"), 2, "non-breaking space");
    assert_eq!(words("one\\,two"), 2, "thin space");
    assert_eq!(words("wait\\ldots what"), 2, "a command ends the word");
    assert_eq!(words("word\u{2014}another"), 2, "Unicode em dash");
    assert_eq!(words("\u{2018}quoted\u{2019} words"), 2, "typographic single quotes");
    assert_eq!(words("one\r\ntwo\rthree\nfour"), 4, "CR, LF and CRLF");
}

#[test]
fn accents_unicode_babel() {
    assert_eq!(words("ação coração Brasília"), 3, "precomposed Portuguese accents");
    assert_eq!(words("a\u{0303}c\u{0327}a\u{0303}o"), 1, "decomposed accents stay one word");
    assert_eq!(words("Müller Straße naïve"), 3, "German, French");
    assert_eq!(words("Poincar\\'e"), 1, "\\'e");
    assert_eq!(words("Poincar{\\'e}"), 1, "{{\\'e}}");
    assert_eq!(words("Poincar\\'{e}"), 1, "\\'{{e}}");
    assert_eq!(words("\\c{c}\\~ao"), 1, "\\c{{c}}\\~ao");
    assert_eq!(words("S\\~ao Paulo"), 2, "S\\~ao Paulo");
    assert_eq!(words("na\\\"{\\i}ve"), 1, "\\\"{{\\i}}");
    assert_eq!(words("\\\"Ubung \\`a la"), 3, "\\\" and \\`");
    assert_eq!(words("\\v{S}koda \\H{o}"), 2, "\\v \\H");
    assert_eq!(words("Stra\\ss e"), 1, "\\ss swallows the space after it, as TeX does");
    assert_eq!(words("Stra\\ss{}e"), 1, "\\ss{{}}");
    assert_eq!(words("Stra{\\ss}e"), 1, "{{\\ss}}");
    assert_eq!(words("Bj\\o rn \\AA ngstr\\o m"), 2, "\\o and \\AA");
    assert_eq!(words("Sch\"on"), 1, "babel \"o");
    assert_eq!(words("Wort\"=Trenner"), 1, "babel \"=");
    assert_eq!(words("Wort\"-Trenner"), 1, "babel \"-");
    assert_eq!(words("\u{6F22}\u{5B57}\u{304B}\u{306A}"), 4, "Han and kana count per character");
    assert_eq!(words("\u{6F22}\u{5B57} and \u{304B}\u{306A}"), 5, "mixed scripts");
    assert_eq!(words("hello \u{1F600} world"), 2, "an emoji is not a word");
    assert_eq!(words("\u{20000} x"), 2, "supplementary-plane ideograph");
    assert_eq!(words("\\'"), 0, "a stray accent");
    assert_eq!(words("\\"), 0, "a lone backslash");
}

#[test]
fn commands_versus_text_arguments() {
    assert_eq!(words("\\textbf{bold} and \\emph{emphasis}"), 3, "text arguments count");
    assert_eq!(words("See Section~\\ref{sec:intro} and \\cite{knuth84}."), 3, "ref and cite do not");
    assert_eq!(words("See \\cite[p.~3]{x} now"), 2, "cite with an option");
    assert_eq!(words("\\textcite{x} says \\autocite*[1]{y} more \\nocite{*}"), 2, "biblatex forms");
    assert_eq!(words("\\citep{a,b} \\citet{c}"), 0, "natbib forms");
    assert_eq!(words("\\label{eq:x}\\eqref{a}\\pageref{b}\\autoref{c}\\cref{d,e}\\crefrange{f}{g}"), 0, "references");
    assert_eq!(words("before\\footnote{A note here.} after"), 5, "footnotes count");
    assert_eq!(words("\\includegraphics[width=0.5\\textwidth]{fig.png}"), 0, "includegraphics");
    assert_eq!(words("\\hspace{1cm}\\vspace*{2em} done"), 1, "lengths are not words");
    assert_eq!(words("\\setlength{\\parindent}{0pt} word"), 1, "setlength");
    assert_eq!(words("\\url{https://example.com/a-b} see"), 1, "url");
    assert_eq!(words("\\href{https://x.org/a}{link text} end"), 3, "href counts the link text only");
    assert_eq!(words("\\begin{figure}[h]\\centering\\includegraphics{a}\\caption{A nice figure}\\label{fig:a}\\end{figure}"), 3, "figure with a caption");
    assert_eq!(words("\\begin{figure}\\caption[short]{Long caption here}\\end{figure}"), 3, "short caption title is dropped");
    assert_eq!(words("see \\begin{tabular}{lcr} a & b & c \\\\ \\hline d & e & f \\\\ \\end{tabular} below"), 2, "table cells are layout, not prose");
    assert_eq!(words("\\begin{minipage}[t]{0.5\\textwidth} inside text \\end{minipage}"), 2, "minipage");
    assert_eq!(words("\\begin{multicols}{2}[Wide title] body text \\end{multicols}"), 4, "multicols: the option is a title");
    assert_eq!(words("\\begin{multicols*}{3} body \\end{multicols*}"), 1, "multicols*");
    assert_eq!(words("\\begin{minted}[linenos]{python} print(1) \\end{minted} done"), 1, "minted: language and code");
    assert_eq!(words("\\mintinline{python}{print(1)} and \\mintinline{c}|x = 1| done"), 2, "mintinline");
    assert_eq!(words("\\textcolor{red}{red words} \\colorbox{yellow}{marked}"), 3, "color arguments");
    assert_eq!(words("\\begin{itemize}\\item[Label] text \\item Next\\end{itemize}"), 3, "item labels are text");
    assert_eq!(words("\\begin{itemize}[leftmargin=*]\\item First\\item Second\\end{itemize}"), 2, "list options");
    assert_eq!(words("\\begin{enumerate}[label=(\\alph*)]\\item one\\end{enumerate}"), 1, "enumitem options");
    assert_eq!(words("a\\\\[2pt] b"), 2, "line break with a length");
    assert_eq!(words("a\\\\* b"), 2, "starred line break");
    assert_eq!(words("\\foo[opt]{arg words}"), 2, "unknown command: option dropped, argument read");
    assert_eq!(words("\\hyperref[sec:a]{see here}"), 2, "hyperref");
    assert_eq!(words("\\texttt{code_name}"), 2, "texttt");
    assert_eq!(words("\\newpage text"), 1, "newpage");
    assert_eq!(words("\\LaTeX{} rocks"), 2, "a logo is a word; braces after a command");
    assert_eq!(words("\\TeX\\ and \\LaTeXe{}"), 3, "logos");
    assert_eq!(words("\\author{Ann Writer\\thanks{Funded.} \\and Bob Reader} \\date{May 2025} text"), 1, "author and date are not prose");
    let title = body("\\title{A Short Title} text");
    assert_eq!(title.totals.heading_words, 3, "a \\title in the body is a heading");
    assert_eq!(title.totals.text_words, 1);
    let title = comp(&[("main.tex", "\\documentclass{article}\n\\title{Not Counted Here}\n\\author{Me}\n\\begin{document}\n\\maketitle\nBody.\n\\end{document}\n")]);
    assert_eq!(title.totals.words(), 1, "title and author in the preamble");
    assert_eq!(words("\\parbox{5cm}{some boxed words} \\raisebox{1ex}[2ex]{up} done"), 5, "parbox and raisebox: the first argument is a length");
    assert_eq!(words("\\textbf{foo}bar"), 1, "braces never split a word");
    assert_eq!(words("foo\\textbf{bar}"), 2, "a command splits a word");
    assert_eq!(words("\\only<2>{shown text}"), 2, "overlay specification");
    assert_eq!(words("\\begin{itemize}\\item<1-> point\\textbf<2>{bold}\\end{itemize}"), 2, "overlays on item and textbf");
    assert_eq!(words("\\newcommand{\\foo}[1]{Some words #1} text"), 1, "newcommand body");
    assert_eq!(words("\\renewcommand{\\arraystretch}{1.5} x"), 1, "renewcommand");
    assert_eq!(words("\\newenvironment{myenv}{begin words}{end words} body"), 1, "newenvironment");
    assert_eq!(words("\\def\\foo#1{ignored words} kept"), 1, "def");
    assert_eq!(words("\\DeclareMathOperator{\\Hom}{Hom} ok"), 1, "DeclareMathOperator");
    assert_eq!(words("\\NewDocumentCommand{\\mycmd}{m}{some words} ok"), 1, "NewDocumentCommand");
    assert_eq!(words("\\begin{tikzpicture}\\draw (0,0) -- (1,1) node {label words};\\end{tikzpicture} after"), 1, "tikzpicture");
    assert_eq!(words("\\begin{thebibliography}{9}\\bibitem{a} Some Author. A title.\\end{thebibliography} end"), 1, "bibliography");
    assert_eq!(words("\\texorpdfstring{one two}{ignored three} four"), 3, "texorpdfstring keeps the TeX argument");
    assert_eq!(words("\\foreignlanguage{portuguese}{olá mundo}"), 2, "foreignlanguage");
}

#[test]
fn tables_floats_captions() {
    assert_eq!(words("\\begin{tabular}[t]{|l|r|} a & b \\end{tabular}"), 0, "tabular with [pos]");
    assert_eq!(words("\\begin{tabular*}{\\textwidth}{@{}ll} a & b \\end{tabular*}"), 0, "tabular*");
    assert_eq!(words("\\begin{tabularx}{\\textwidth}{XX} a & b \\end{tabularx} after"), 1, "tabularx");
    assert_eq!(words("\\begin{tabulary}{\\linewidth}{LL} a & b \\end{tabulary} after"), 1, "tabulary");
    assert_eq!(words("\\begin{tabular}{cc} \\multicolumn{2}{c}{Merged cell} \\\\ \\cmidrule(lr){1-2} x & y \\end{tabular}"), 0, "multicolumn and cmidrule");
    assert_eq!(words("\\begin{tabular}{l} \\begin{tabular}{l} inner \\end{tabular} outer \\end{tabular} after"), 1, "nested tabular");
    assert_eq!(words("\\begin{tabbing} a \\= b \\\\ c \\> d \\end{tabbing} after"), 1, "tabbing");
    assert_eq!(words("\\begin{titlepage} University of Somewhere \\end{titlepage} Introduction text"), 2, "titlepage");
    assert_eq!(words("\\begin{tabular}{l} a $x$ \\[\\y\\] \\end{tabular}"), 0, "words in a table");
    let stats = body("\\begin{tabular}{l} $x$ and $$y$$ \\end{tabular}");
    assert_eq!(stats.totals.inline_math + stats.totals.display_math, 0, "formulas in a table are not counted");
    assert_eq!(words("\\begin{longtable}{ll} \\caption{Long caption} \\\\ a & b \\\\ \\end{longtable} after"), 3, "longtable: caption and text after only");

    // Floats: only the captions.
    assert_eq!(words("\\begin{figure}[htbp] \\centering \\includegraphics{a} \\caption{Five six} \\label{f} \\end{figure}"), 2, "figure");
    assert_eq!(words("\\begin{figure*} stray words \\caption{Five six seven} \\end{figure*}"), 3, "figure*: stray words do not count");
    assert_eq!(words("\\begin{figure} one two \\begin{minipage}{5cm} three four \\end{minipage} \\caption{five six} \\end{figure}"), 2, "minipage in a figure");
    assert_eq!(words("\\begin{figure} one two \\begin{center} three four \\end{center} \\caption{five six} \\end{figure}"), 2, "center in a figure");
    assert_eq!(words("\\begin{figure} one two\\footnote{foot note} \\caption[short opt]{five six} \\end{figure}"), 2, "footnotes in a figure follow the figure");
    assert_eq!(words("\\begin{table} \\begin{tabular}{ll} a & b \\end{tabular} \\caption{five six} \\end{table}"), 2, "table float");
    assert_eq!(words("\\begin{table}[h] \\caption{Five} \\begin{tabularx}{\\textwidth}{lX} a & b \\end{tabularx} \\end{table}"), 1, "caption above a tabularx");
    assert_eq!(words("\\begin{wrapfigure}{r}{5cm} a b c \\caption{d e} \\end{wrapfigure} f"), 3, "wrapfigure");
    assert_eq!(words("\\begin{sidewaystable} a b \\caption{c d e} \\end{sidewaystable}"), 3, "sidewaystable");
    assert_eq!(words("\\begin{figure} \\begin{subfigure}{.5\\textwidth} a b \\caption{c d} \\end{subfigure} \\caption{e} \\end{figure}"), 3, "subfigure environments inside a figure");
    assert_eq!(words("\\begin{figure} \\subfloat[sub cap]{\\includegraphics{x}} \\subfloat[list][other cap]{x y} \\end{figure}"), 4, "subfloat: the last option is the caption");
    assert_eq!(words("\\begin{figure} \\subfigure[one two]{\\includegraphics{x}} \\caption{three} \\end{figure}"), 3, "subfigure command");
    assert_eq!(words("\\begin{figure} \\includegraphics{x} \\captionof{table}{c d e} \\end{figure}"), 3, "captionof inside a float");
    assert_eq!(words("before \\captionof{figure}{c d e} after"), 5, "captionof outside a float: the type is not text");
    assert_eq!(words("\\captionof*{figure}[short]{Only this} z"), 3, "captionof with star and short title");
    assert_eq!(words("\\caption*{Starred caption}"), 2, "caption*");
    assert_eq!(words("\\begin{figure} \\begin{subfigure}{.3\\textwidth} x \\subcaption{Sub text} \\end{subfigure} \\end{figure}"), 2, "subcaption");
    assert_eq!(words("\\begin{figure} \\caption{A \\emph{bold} claim\\footnote{with a note}} \\end{figure}"), 6, "markup and footnotes inside a caption");
    assert_eq!(words("\\begin{figure} \\caption{Caption with $x^2$ inside} \\end{figure}"), 3, "formulas in a caption are formulas");
    let stats = body("\\begin{figure} $a$ \\[ b \\] \\caption{With $x$} \\end{figure}");
    assert_eq!(stats.totals.inline_math, 1, "only the caption's formula");
    assert_eq!(stats.totals.display_math, 0);
    assert_eq!(words("\\begin{figure} unclosed words \\caption{kept}"), 3, "a figure still being typed counts as text");
    assert_eq!(words("\\begin{figure} \\caption{one} \\end{figure} \\begin{figure} \\caption{two three} \\end{figure} after"), 4, "two floats in a row");
    assert_eq!(words("\\begin{figure}\\begin{figure} \\caption{inner} \\end{figure} \\caption{outer} \\end{figure} x"), 3, "nested floats");
    assert_eq!(words("% \\begin{figure}\ntext\n% \\end{figure}\nmore"), 2, "commented float markers do nothing");
    assert_eq!(words("\\begin{figure}\n\n\\caption{Across paragraphs}\n\n\\end{figure}"), 2, "blank lines inside a float");
    assert_eq!(
        words("\\begin{tikzpicture} \\node {x}; \\end{tikzpicture} \\begin{figure} \\begin{tikzpicture} \\node {z}; \\end{tikzpicture} \\caption{Plot} \\end{figure}"),
        1,
        "picture in a figure"
    );

    // Captions sit in the section that is open.
    let stats = comp(&[("main.tex", &document("\\section{One}\nText.\n\\begin{figure}\\caption{Cap}\\end{figure}\n\\section{Two}\nMore text.\n"))]);
    assert_eq!(stats.sections.iter().map(|s| s.own.text_words).collect::<Vec<_>>(), [2, 2], "the caption belongs to its section");

    // Includes inside a float are still walked in order, but a caption in them counts.
    let stats = comp(&[
        ("main.tex", &document("\\section{A}\n\\begin{figure}\\input{fig}\\end{figure}\n\\section{B}\nWord.\n")),
        ("fig.tex", "\\includegraphics{x} stray \\caption{from the file}\n"),
    ]);
    assert_eq!(stats.sections.iter().map(|s| s.own.text_words).collect::<Vec<_>>(), [3, 1], "a caption in an included figure file");
    assert_eq!(stats.files.len(), 2);
}

#[test]
fn comments_verbatim_escapes() {
    assert_eq!(words("Hello % hidden words\nworld"), 2, "comments");
    assert_eq!(words("% only a comment\n"), 0, "comment line");
    assert_eq!(words("50\\% of cases"), 3, "escaped percent");
    assert_eq!(words("\\begin{verbatim}\nlots of words\n\\end{verbatim}\nafter"), 1, "verbatim");
    assert_eq!(words("\\verb|code here| done"), 1, "\\verb");
    assert_eq!(words("\\begin{lstlisting}[language=C]\nint main() {}\n\\end{lstlisting}"), 0, "lstlisting");
    assert_eq!(words("\\begin{comment}\nnot counted\n\\end{comment} yes"), 1, "comment environment");
    assert_eq!(words("a \\& b \\_ c \\# d"), 4, "escaped specials separate");
    assert_eq!(words("cost \\$5 here"), 3, "escaped dollar");
}

#[test]
fn formulas() {
    let stats = body("The value $x^2 + y^2$ is positive");
    assert_eq!(stats.totals.text_words, 4, "inline math is not text");
    assert_eq!(stats.totals.inline_math, 1);
    assert_eq!(stats.totals.display_math, 0);

    let stats = body("Let $f\\colon X \\to Y$ be a map, $g$ too.");
    assert_eq!(stats.totals.inline_math, 2);
    assert_eq!(stats.totals.text_words, 5);

    let stats = body("before $$a+b$$ after \\[ c \\] and \\(d\\)");
    assert_eq!(stats.totals.display_math, 2, "$$ and \\[");
    assert_eq!(stats.totals.inline_math, 1, "\\(");
    assert_eq!(stats.totals.text_words, 3, "before, after, and");

    let stats = body("Text\n\\begin{equation}\\label{eq:1} E=mc^2 \\end{equation}\nmore");
    assert_eq!(stats.totals.display_math, 1, "equation");
    assert_eq!(stats.totals.text_words, 2);

    let stats = body("\\begin{align*} a &= b \\\\ c &= d \\end{align*} \\begin{gather} x \\end{gather}");
    assert_eq!(stats.totals.display_math, 2, "one per environment");
    assert_eq!(stats.totals.words(), 0);

    let stats = body("\\begin{math} x \\end{math} \\begin{displaymath} y \\end{displaymath}");
    assert_eq!(stats.totals.inline_math, 1, "math environment is inline");
    assert_eq!(stats.totals.display_math, 1, "displaymath");

    let stats = body("$x \\text{ if } y$");
    assert_eq!(stats.totals.words(), 0, "\\text inside a formula is not counted");
    assert_eq!(stats.totals.inline_math, 1);

    let stats = body("$x \\text{if $y$} z$ after");
    assert_eq!(stats.totals.inline_math, 1, "a nested $ inside a text group");
    assert_eq!(stats.totals.text_words, 1);

    let stats = body("price $5 and more");
    assert_eq!(stats.totals.inline_math, 0, "an unclosed $ is not a formula");
    assert_eq!(stats.totals.text_words, 4);

    let stats = body("a $b\n\nc $d$ e");
    assert_eq!(stats.totals.inline_math, 1, "a blank line ends an unclosed $");
    assert_eq!(stats.totals.text_words, 4);

    let stats = body("\\begin{equation}\nfoo bar\n\nnext");
    assert_eq!(stats.totals.display_math, 0, "an environment still being typed");
    assert_eq!(stats.totals.text_words, 3);

    let stats = body("\\section{Area of $\\pi r^2$}");
    assert_eq!(stats.totals.heading_words, 2);
    assert_eq!(stats.totals.inline_math, 1, "formula in a heading");

    let stats = body("\\label{eq:$x$} ok");
    assert_eq!(stats.totals.inline_math, 0, "formulas in ignored arguments");
}

#[test]
fn preamble_and_end_of_document() {
    let full = comp(&[("main.tex", "\\documentclass{article}\n\\usepackage{amsmath}\n\\title{My Title}\n\\author{A B}\n\\begin{document}\nBody words.\n\\end{document}\nThese words come after.")]);
    assert_eq!(full.totals.words(), 2, "preamble and trailing text are excluded");

    let no_begin = comp(&[("main.tex", "\\documentclass{article}\nJust some words\n")]);
    assert_eq!(no_begin.totals.words(), 3, "without \\begin{{document}} everything counts");

    let fragment = comp(&[("main.tex", "Just text\n")]);
    assert_eq!(fragment.totals.words(), 2, "a file without \\documentclass");

    let split = comp(&[
        ("main.tex", "\\documentclass{article}\n\\input{pre}\n\\begin{document}\n\\begin{thm}[Named result] a b \\end{thm}\n\\begin{foo}[option] c \\end{foo}\n\\end{document}"),
        ("pre.tex", "\\newtheorem{thm}{Theorem}\n\\title{Ignored here}\n"),
    ]);
    assert_eq!(split.totals.words(), 5, "preamble file excluded; \\newtheorem makes [Title] text, other options are dropped");
    assert_eq!(split.files.iter().map(|f| f.to_string()).collect::<Vec<_>>(), ["/proj/main.tex", "/proj/pre.tex"]);

    let declared = comp(&[("main.tex", "\\documentclass{article}\n\\usepackage{thmtools}\n\\declaretheorem[style=plain]{lemma, corollary}\n\\begin{document}\n\\begin{corollary}[Big one] x \\end{corollary}\n\\begin{proof}[Proof of x] y \\end{proof}\n\\end{document}")]);
    assert_eq!(declared.totals.words(), 7, "\\declaretheorem and amsthm's proof");
}

#[test]
fn sections_and_front_matter() {
    let stats = body("\\maketitle\n\\begin{abstract}This is it.\\end{abstract}\n\\section{A}\ntext");
    assert_eq!(stats.front_matter.own.text_words, 3, "front matter");
    assert!(stats.front_matter.is_front_matter());
    assert_eq!(titles(&stats), ["A"]);
    assert_eq!(stats.sections[0].own, c(1, 1, 0, 0));
    assert_eq!(stats.totals.words(), 5);

    let stats = body("\\section*{Unnumbered Title}x\n\\section[Short]{Long Title Here}y z");
    assert_eq!(titles(&stats), ["Unnumbered Title", "Long Title Here"]);
    assert!(stats.sections[0].is_starred);
    assert_eq!(stats.sections[0].own.heading_words, 2);
    assert_eq!(stats.sections[1].own.heading_words, 3, "the short title is ignored");
    assert_eq!(stats.sections[1].own.text_words, 2);

    let stats = body("\\section{The \\emph{great} idea}\\label{sec:i}");
    assert_eq!(stats.sections[0].own.heading_words, 3);
    assert_eq!(stats.sections[0].own.text_words, 0);

    let stats = body("\\chapter{C1} a b \\section{S1} c \\subsection{SS1} d e \\section{S2} f \\chapter{C2} g");
    assert_eq!(titles(&stats), ["C1", "S1", "SS1", "S2", "C2"]);
    assert_eq!(stats.sections.iter().map(|s| s.own.words()).collect::<Vec<_>>(), [3, 2, 3, 2, 2]);
    assert_eq!(
        stats.sections.iter().map(|s| s.subtree.words()).collect::<Vec<_>>(),
        [10, 5, 3, 2, 2],
        "a section's subtree runs to the next heading of the same or a shallower level"
    );
    assert_eq!(stats.totals.words(), 12);
    assert_eq!(stats.sections.iter().map(|s| s.level).collect::<Vec<_>>(), [1, 2, 3, 2, 1]);

    let stats = body("\\subsection{Skips} a \\section{Then} b");
    assert_eq!(stats.sections.iter().map(|s| s.subtree.words()).collect::<Vec<_>>(), [2, 2], "a shallower heading closes the deeper one");

    let stats = body("");
    assert_eq!(stats.totals, StatisticsCounts::ZERO, "empty document");
    assert_eq!(stats.sections.len(), 0);
    assert_eq!(comp(&[("main.tex", "")]).totals, StatisticsCounts::ZERO, "empty file");

    let ids: Vec<&str> = stats.all_sections().iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, [FRONT_MATTER_ID]);
    assert!(stats.section(FRONT_MATTER_ID).map(|s| s.is_front_matter()).unwrap_or(false));
    assert!(stats.section("nope").is_none());
}

#[test]
fn multi_file_reading_order() {
    let project = [
        ("main.tex", "\\documentclass{article}\n\\begin{document}\n\\section{One}\nalpha beta\n\\input{b}\ngamma\n\\section{Two}\ndelta\n\\end{document}"),
        ("b.tex", "\\subsection{Sub}\nepsilon zeta eta\n"),
    ];
    let stats = comp(&project);
    assert_eq!(titles(&stats), ["One", "Sub", "Two"], "the included heading sits where \\input is");
    assert_eq!(stats.sections.iter().map(|s| s.own.words()).collect::<Vec<_>>(), [3, 5, 2], "text after the \\input stays in the section the file ended in");
    assert_eq!(stats.sections.iter().map(|s| s.subtree.words()).collect::<Vec<_>>(), [8, 5, 2]);
    assert_eq!(stats.totals.words(), 10);
    assert_eq!(&*stats.sections[1].file, "/proj/b.tex");
    assert_eq!(&*stats.sections[0].file, "/proj/main.tex");
    assert_eq!(stats.files.iter().map(|f| f.to_string()).collect::<Vec<_>>(), ["/proj/main.tex", "/proj/b.tex"]);
    assert_eq!(sum_of(&stats), stats.totals, "per-section counts add up to the totals");

    // A section that continues in another file, a primitive \input, \include, \import, \subfile.
    let mixed = comp(&[
        ("main.tex", "\\documentclass{article}\n\\begin{document}\n\\section{A}\none \\input chapters/x\ntwo \\include{chapters/y} three\n\\import{chapters/}{z}\n\\subfile{chapters/w}\nend\n\\end{document}"),
        ("chapters/x.tex", "x1 x2\n"),
        ("chapters/y.tex", "y1\n\\subsection{Why}\ny2\n"),
        ("chapters/z.tex", "z1\n"),
        ("chapters/w.tex", "\\documentclass[../main.tex]{subfiles}\n\\begin{document}\nw1 w2\n\\end{document}\ntrailing words"),
    ]);
    assert_eq!(titles(&mixed), ["A", "Why"]);
    assert_eq!(mixed.totals.text_words, 11, "one x1 x2 two y1 | y2 three z1 w1 w2 end; the subfile's trailing words are outside its document");
    assert_eq!(mixed.sections[0].own.text_words, 5, "A: one x1 x2 two y1 (up to the subsection)");
    assert_eq!(mixed.sections[1].own.text_words, 6, "Why: y2 three z1 w1 w2 end");
    assert_eq!(mixed.files.len(), 5, "all files walked");

    // Each file is counted once, cycles are safe.
    let cyclic = comp(&[
        ("main.tex", &document("\\input{a} \\input{a} main")),
        ("a.tex", "one \\input{main} \\input{b}"),
        ("b.tex", "two \\input{a}"),
    ]);
    assert_eq!(cyclic.totals.words(), 3, "a, b and main once each (one two main)");
    assert_eq!(cyclic.files.len(), 3);

    // Missing and non-.tex files are not followed.
    let missing = comp(&[("main.tex", &document("\\input{nothing} \\input{data.csv} \\input{pic.pgf} word")), ("data.csv", "a b c"), ("pic.pgf", "p q r")]);
    assert_eq!(missing.totals.words(), 1);
}

#[test]
fn skipped_headings_subfiles_outline_parity() {
    // An \input and a heading inside a definition: part of the outline, but not typeset.
    let hidden = comp(&[
        ("main.tex", "\\documentclass{article}\n\\newcommand{\\x}{\\input{hidden}}\n\\newcommand{\\mysec}{\\section{Phantom}}\n\\begin{document}\n\\section{Real}\nreal words\n\\end{document}"),
        ("hidden.tex", "\\section{Hidden}\nsecret words here\n"),
    ]);
    assert_eq!(titles(&hidden), ["Hidden", "Phantom", "Real"], "same sections, same order as the outline");
    assert_eq!(hidden.sections.iter().map(|s| s.own.words()).collect::<Vec<_>>(), [0, 0, 3], "skipped headings take no text, and do not steal the text after them");
    assert_eq!(hidden.totals.words(), 3);

    // Outline parity on a project with nesting, cycles and several files.
    let f = files(&[
        ("main.tex", "\\documentclass{book}\n\\begin{document}\n\\chapter{Intro}\\label{c:i}\n\\input{sections/intro}\n\\section{Methods}\n\\input{sections/methods}\n\\include{sections/results}\n\\end{document}"),
        ("sections/intro.tex", "\\section{Background}\\label{s:b}\nword\n\\input{sections/deep}\n\\section{Goals}\ngoal\n"),
        ("sections/deep.tex", "\\subsection{Deep}\n\\input{sections/intro}\ndeep\n"),
        ("sections/methods.tex", "\\subsection{Setup}\nsetup\n\\input{sections/missing}\n"),
        ("sections/results.tex", "\\chapter{Results}\nres\n\\begin{frame}{Slide title}x\\end{frame}\n"),
    ]);
    let stats = compute_files(&f);
    let index = model_of(&f);
    assert_eq!(stats.sections.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), outline_ids(&index.outline), "section ids equal the outline's node ids");
    assert_eq!(stats.files, index.files, "same files in the same order");
    for section in &stats.sections {
        assert!(stats.section(&section.id).is_some(), "lookup by id: {}", section.title);
    }
    assert_eq!(sum_of(&stats), stats.totals, "sums equal totals");
    let keys: Vec<&String> = stats.sections.iter().map(|s| &s.stable_key).collect();
    assert_eq!(keys.iter().collect::<HashSet<_>>().len(), keys.len(), "stable keys are unique");

    // Same heading twice: stable keys keep them apart.
    let twice = body("\\section{Same} a \\section{Same} b c");
    let k: HashSet<&String> = twice.sections.iter().map(|s| &s.stable_key).collect();
    assert_eq!(k.len(), twice.sections.len());
    assert_eq!(twice.sections.iter().map(|s| s.own.text_words).collect::<Vec<_>>(), [1, 2]);

    // A subfile counts only between its \begin{document} and \end{document}.
    let subfiles = comp(&[
        ("main.tex", "\\documentclass{article}\n\\usepackage{subfiles}\n\\begin{document}\n\\subfile{chap}\nafter\n\\end{document}"),
        ("chap.tex", "\\documentclass[main.tex]{subfiles}\n\\title{Not counted}\n\\begin{document}\nChapter words.\n\\end{document}"),
    ]);
    assert_eq!(subfiles.totals.words(), 3, "Chapter words + after");

    // Beamer frames are sections.
    let beamer = comp(&[("main.tex", "\\documentclass{beamer}\n\\begin{document}\n\\begin{frame}[t]{First slide}\nSome content here.\n\\end{frame}\n\\begin{frame}\n\\frametitle{Second one}\nMore.\n\\end{frame}\n\\end{document}")]);
    assert_eq!(titles(&beamer), ["First slide", "Second one"]);
    assert_eq!(beamer.sections.iter().map(|s| s.kind).collect::<Vec<_>>(), [Some(OutlineKind::Frame), Some(OutlineKind::Frame)]);
    assert_eq!(beamer.sections[0].own, c(3, 2, 0, 0));
    assert_eq!(beamer.sections[1].own, c(1, 2, 0, 0));
    assert_eq!(beamer.sections.iter().map(|s| s.level).collect::<Vec<_>>(), [3, 3]);
}

#[test]
fn heading_lines() {
    let stats = comp(&[("main.tex", "\\documentclass{article}\n\\begin{document}\n\\section{One line}\n\\section{Two\nlines}\n\\end{document}\n")]);
    assert_eq!(stats.sections.iter().map(|s| s.line).collect::<Vec<_>>(), [3, 4]);
    assert_eq!(stats.sections.iter().map(|s| s.heading_end_line).collect::<Vec<_>>(), [3, 5], "a heading that spans lines reports where it ends");
}

#[test]
fn random_input_invariants() {
    let pieces = [
        "\\section{", "\\subsection*{", "\\chapter[s]{", "}", "{", "$", "$$", "\\[", "\\]", "\\(", "\\)", "[", "]", "(", ")", "\\begin{equation}",
        "\\end{equation}", "\\begin{itemize}", "\\end{itemize}", "\\item ", "\\item[", "% c\n", "\n", "\n\n", " ", "  ", "word", "two words", "ação",
        "state-of-art", "x", "\\emph{", "\\textbf{x}", "\\verb|x|", "\\verb", "\\begin{verbatim}", "\\end{verbatim}", "\\input{b}", "\\include{b}",
        "\\label{a}", "\\ref{a}", "\\\\", "\\", "\"", "'", "-", "--", "3.14", "1,5", "\\'e", "\\c{c}", "\\ss ", "&", "\\newcommand{\\x}{", "\\def\\y#1{",
        "\\begin{document}", "\\end{document}", "\\documentclass{article}", "\u{1F600}", "e\u{0301}", "\\begin{tabular}{lc}", "\\end{tabular}",
        "\\caption{", "\\footnote{", "\\begin{frame}{T}", "\\end{frame}", "\\frametitle{", "\\begin{tikzpicture}", "\\end{tikzpicture}",
        "\\begin{align}", "\\end{align}", "\\only<2>{", "\\textcolor{red}{", "~", "\r\n",
    ];
    let mut random = Lcg(20261028);
    let soup = |random: &mut Lcg| {
        let mut text = String::new();
        for _ in 0..(5 + random.next() % 40) {
            text.push_str(pieces[random.next() % pieces.len()]);
        }
        text
    };
    let mut mismatches = 0;
    for round in 0..1500 {
        let f = files(&[("main.tex", &soup(&mut random)), ("b.tex", &soup(&mut random))]);
        let stats = compute_files(&f);
        let again = compute_files(&f);
        let index = model_of(&f);
        let sum = sum_of(&stats);
        let valid = stats.sections.iter().all(|s| s.subtree.words() >= s.own.words());
        let parity = stats.sections.iter().map(|s| s.id.clone()).collect::<Vec<_>>() == outline_ids(&index.outline);
        if sum != stats.totals || !valid || stats != again || !parity {
            mismatches += 1;
            if mismatches <= 3 {
                panic!("round {round}: sums {} valid {valid} stable {} outline parity {parity} for {:?}", sum == stats.totals, stats == again, f["main.tex"]);
            }
        }
        // Counting CRLF text gives the same counts as LF text.
        let crlf: Files = f.iter().map(|(k, v)| (k.clone(), v.replace("\r\n", "\n").replace('\n', "\r\n"))).collect();
        let lf: Files = f.iter().map(|(k, v)| (k.clone(), v.replace("\r\n", "\n"))).collect();
        let (crlf, lf) = (compute_files(&crlf), compute_files(&lf));
        if crlf.totals != lf.totals || crlf.sections.iter().map(|s| s.own).collect::<Vec<_>>() != lf.sections.iter().map(|s| s.own).collect::<Vec<_>>() {
            mismatches += 1;
            if mismatches <= 3 {
                panic!("round {round}: CRLF and LF differ for {:?}", f["main.tex"]);
            }
        }
        // A selection of random text is fine too.
        let _ = counts_in(&f["b.tex"]);
    }
    assert_eq!(mismatches, 0, "random projects");
}

#[test]
fn selections() {
    let counts = counts_in("some \\emph{words} here $x$");
    assert_eq!(counts.text_words, 3);
    assert_eq!(counts.inline_math, 1);
    let counts = counts_in("\\section{A B} text");
    assert_eq!(counts.heading_words, 2);
    assert_eq!(counts.words(), 3);
    assert_eq!(counts_in("").words(), 0);
    assert_eq!(counts_in("words}").words(), 1, "a selection can start inside a group");
    assert_eq!(counts_in("\\documentclass{article} the preamble is not special here").words(), 6);
    assert_eq!(counts_in("% comment only").words(), 0);
    assert_eq!(counts_in("ação, coração").words(), 2);
    assert_eq!(counts_in("a \\input{x} b").words(), 2, "no includes in a selection");
}

#[test]
fn page_attribution() {
    let hp = |page: i64, top: bool| Some(HeadingPlacement { page, is_at_page_top: top });
    let span = |first: i64, last: i64, approx: bool| Some(PageSpan { first, last, is_approximate: approx });
    // front matter (p1), A (p1 mid), B (p3 top), C (p5 mid), unknown, D (p8 mid); 10 pages.
    let placements = [hp(1, true), hp(1, false), hp(3, true), hp(5, false), None, hp(8, false)];
    let spans = page_spans(&placements, 10);
    assert_eq!(spans.len(), 6);
    assert_eq!(spans[0], span(1, 1, true), "shares its page with the next heading");
    assert_eq!(spans[1], span(1, 2, false), "the next heading starts a page: the section ends before it");
    assert_eq!(spans[2], span(3, 5, false), "the next heading is mid-page: the page is shared");
    assert_eq!(spans[3], span(5, 8, false), "an unknown heading in between is skipped");
    assert_eq!(spans[4], None, "no page, no span");
    assert_eq!(spans[5], span(8, 10, false), "the last section runs to the last page");
    assert_eq!(spans[1].unwrap().page_count(), 2);

    // Pages going backwards or past the end are ignored.
    let odd = page_spans(&[hp(2, false), hp(1, false), hp(3, false), hp(9, false)], 5);
    assert_eq!(odd[0], span(2, 3, false));
    assert_eq!(odd[1], None, "a page before the previous one");
    assert_eq!(odd[2], span(3, 5, false));
    assert_eq!(odd[3], None, "a page beyond the PDF");

    assert_eq!(page_spans(&placements, 0).iter().flatten().count(), 0, "no PDF");
    assert_eq!(page_spans(&[], 4).len(), 0);
    assert_eq!(page_spans(&[hp(1, true)], 1), vec![span(1, 1, false)], "a single section");
    // The next heading at the top of the same page cannot make the span empty.
    assert_eq!(page_spans(&[hp(4, false), hp(4, true)], 6)[0], span(4, 4, true));

    // A section's span reaches the last page of its sub-sections.
    let nested = subtree_spans(
        &[-1, 1, 2, 3, 2, 1],
        &[span(1, 1, true), span(1, 2, false), span(2, 2, false), span(2, 4, false), span(4, 5, false), span(5, 7, false)],
    );
    assert_eq!(
        nested,
        vec![span(1, 1, true), span(1, 5, false), span(2, 4, false), span(2, 4, false), span(4, 5, false), span(5, 7, false)],
        "chapter 1 reaches page 5, section 1 reaches page 4, the front matter stays"
    );
    assert_eq!(subtree_spans(&[1, 2, 2], &[None, span(1, 2, false), None]), vec![None, span(1, 2, false), None], "no span, no change");
    assert_eq!(
        subtree_spans(&[1, 2], &[span(3, 3, true), span(3, 4, false)]),
        vec![span(3, 4, false), span(3, 4, false)],
        "a section that grows is no longer approximate"
    );
    assert_eq!(subtree_spans(&[1], &[span(1, 2, false), span(2, 3, false)]), vec![span(1, 2, false), span(2, 3, false)], "fewer levels than spans");
    assert_eq!(subtree_spans(&[], &[]).len(), 0);

    assert!(is_at_page_top(720.0, 0.0, 792.0));
    assert!(!is_at_page_top(400.0, 0.0, 792.0));
    assert!(is_at_page_top(740.0, 100.0, 800.0), "a media box that does not start at 0");
    assert!(!is_at_page_top(10.0, 0.0, 0.0), "degenerate media box");
}

#[test]
fn classifier_unicode() {
    assert_eq!(classify('a'), CharacterClass::Letter);
    assert_eq!(classify('ç'), CharacterClass::Letter);
    assert_eq!(classify('\u{0301}'), CharacterClass::Mark);
    assert_eq!(classify('٣'), CharacterClass::Digit);
    assert_eq!(classify('²'), CharacterClass::Other);
    assert_eq!(classify('Ⅷ'), CharacterClass::Other);
    assert_eq!(classify('漢'), CharacterClass::Ideograph);
    assert_eq!(classify('\u{2019}'), CharacterClass::Apostrophe);
    assert_eq!(classify('\u{2010}'), CharacterClass::Hyphen);
    assert_eq!(classify('\u{200D}'), CharacterClass::Mark);
    assert_eq!(classify('😀'), CharacterClass::Other);
    assert_eq!(classify('—'), CharacterClass::Other);
}

#[test]
fn speed() {
    let limit = if cfg!(debug_assertions) { 3000.0 } else { 300.0 };
    let block = "\\section{Introduction}\\label{sec:intro}\n\
        Let $X$ be a topological space and $f\\colon X\\to Y$ a map; see \\cite[Thm.~2.1]{hatcher2002} and \\eqref{eq:main}.\n\
        % a comment with \\command and $math\n\
        \\begin{equation}\\label{eq:main}\n\
        \x20 \\sum_{i=1}^{n} \\frac{\\partial f}{\\partial x_i}\\,dx_i = \\int_M \\omega \\quad \\text{for all } n\\ge 0\n\
        \\end{equation}\n\
        \\begin{itemize}\n\
        \x20 \\item \\textbf{Bold} text with \\emph{emphasis}, 50\\% of cases, and \\verb|x % y|.\n\
        \x20 \\item Another item \\ref{sec:intro}, see \\url{https://example.org/a%20b#c}.\n\
        \\end{itemize}\n\
        Plain prose without much markup, just words and more words to dilute the density of the commands in ação.\n\n";
    let mut text = String::from("\\documentclass{article}\n\\begin{document}\n");
    while text.encode_utf16().count() < 400_000 {
        text.push_str(block);
    }
    text.push_str("\\end{document}\n");
    let start = std::time::Instant::now();
    let stats = comp(&[("main.tex", &text)]);
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    eprintln!("Statistics: {} KB, {} sections, {} words in {ms:.0} ms", text.len() / 1000, stats.sections.len(), stats.totals.words());
    assert!(ms < limit, "counting 400 KB took {ms} ms (limit {limit})");
    assert!(stats.totals.words() > 10_000);

    let half: String = text.chars().take(200_000).collect();
    let start = std::time::Instant::now();
    let selected = counts_in(&half);
    let selection_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert!(selected.words() > 1000);
    assert!(selection_ms < limit, "counting a 200 KB selection took {selection_ms} ms");
}

// ---------------------------------------------------------------------------------------------------
// Performance budgets of PerformanceChecks.swift (scanner, linter, bib, index)
// ---------------------------------------------------------------------------------------------------

fn median_ms<F: FnMut()>(mut body: F) -> f64 {
    for _ in 0..2 {
        body();
    }
    let mut times: Vec<f64> = (0..9)
        .map(|_| {
            let start = std::time::Instant::now();
            body();
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times[times.len() / 2]
}

#[test]
fn performance_budgets() {
    use crate::core::scanner::{bib, latex_scanner as ls, walker};
    let scale = if cfg!(debug_assertions) { 40.0 } else { 1.0 };
    let block = "\\section{Introduction}\\label{sec:intro}\n\
        Let $X$ be a topological space and $f\\colon X\\to Y$ a map; see \\cite[Thm.~2.1]{hatcher2002} and \\eqref{eq:main}.\n\
        % a comment with \\command and $math\n\
        \\begin{equation}\\label{eq:main}\n\
        \x20 \\sum_{i=1}^{n} \\frac{\\partial f}{\\partial x_i}\\,dx_i = \\int_M \\omega \\quad \\text{for all } n\\ge 0\n\
        \\end{equation}\n\
        \\begin{itemize}\n\
        \x20 \\item \\textbf{Bold} text with \\emph{emphasis}, 50\\% of cases, and \\verb|x % y|.\n\
        \x20 \\item Another item \\ref{sec:intro}, see \\url{https://example.org/a%20b#c}.\n\
        \\end{itemize}\n\
        \\begin{verbatim}\n\
        \\foo { % $ unbalanced\n\
        \\end{verbatim}\n\
        \\input{sections/part}\n\
        Plain prose without much markup, just words and more words to dilute the density of the commands in ação.\n\n";
    let mut doc = String::new();
    while doc.encode_utf16().count() < 100_000 {
        doc.push_str(block);
    }
    let file = url("main.tex");
    let mut report: Vec<String> = Vec::new();
    let mut check = |name: &str, limit_ms: f64, ms: f64| {
        report.push(format!("{name} {ms:.2} ms"));
        assert!(ms < limit_ms * scale, "{name}: {ms:.2} ms exceeds {} ms", limit_ms * scale);
    };
    check("outline", 25.0, median_ms(|| drop(ls::outline_entries(&doc, &file))));
    check("labels", 25.0, median_ms(|| drop(ls::labels(&doc, &file))));
    check("includes", 25.0, median_ms(|| drop(ls::includes(&doc))));
    check("customCommands", 25.0, median_ms(|| drop(ls::custom_commands(&doc))));
    check("structure lint", 40.0, median_ms(|| drop(walker::lint_text(&doc, &file))));

    // BibParser on ~1 MB.
    let bib_text = (0..6000)
        .map(|i| format!("@article{{key{i},\n  author = {{Author, Name and Other, Person}},\n  title = {{A {{T}}itle about topology number {i}}},\n  journal = {{Journal}},\n  year = {{{}}},\n}}\n", 1900 + i % 120))
        .collect::<Vec<_>>()
        .join("\n");
    check("bib parse", 80.0, median_ms(|| drop(bib::entries(&bib_text, &url("refs.bib")))));

    // ProjectIndex over 60 files of ~16 KB each.
    let mut f = Files::new();
    let mut main = String::from("\\documentclass{article}\n\\begin{document}\n");
    for n in 0..60 {
        main.push_str(&format!("\\input{{sections/s{n}}}\n"));
        f.insert(format!("sections/s{n}.tex"), block.replace("sections/part", "sections/leaf").repeat(18));
    }
    f.insert("sections/leaf.tex".into(), "\\subsection{Leaf}\n".into());
    f.insert("main.tex".into(), format!("{main}\\end{{document}}\n"));
    check("ProjectIndex.build (60 files)", 200.0, median_ms(|| drop(model_of(&f))));
    eprintln!("TIMING: {}", report.join("; "));
}

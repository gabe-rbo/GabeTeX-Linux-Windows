//! Port of `TokenizerChecks.swift`.

use crate::core::scanner::tests_common::*;
use crate::core::tokenizer::*;

fn d(text: &str) -> Vec<String> {
    describe(text)
}

fn only(text: &str, kinds: &[TokenKind]) -> Vec<String> {
    let u = crate::core::scanner::syntax::units(text);
    tokenize_units(&u)
        .into_iter()
        .filter(|t| kinds.contains(&t.kind))
        .map(|t| format!("{}:{}", t.kind.raw(), String::from_utf16_lossy(&u[t.start..t.end()])))
        .collect()
}

fn has_prefix(v: Vec<String>, p: &str) -> Vec<String> {
    v.into_iter().filter(|s| s.starts_with(p)).collect()
}

macro_rules! eq {
    ($left:expr, [$($right:expr),* $(,)?] $(, $($msg:tt)+)?) => {
        assert_eq!($left, Vec::<String>::from([$($right.to_string()),*]) $(, $($msg)+)?)
    };
}

#[test]
fn commands_and_escaped_specials() {
    eq!(d("\\alpha text \\foo{bar}"), ["command:\\alpha", "command:\\foo", "brace:{", "brace:}"]);
    eq!(
        d("\\% \\{ \\} \\$ \\& \\\\ \\_ \\, x"),
        ["command:\\%", "command:\\{", "command:\\}", "command:\\$", "command:\\&", "command:\\\\", "command:\\_", "command:\\,"]
    );
    eq!(d("50\\% sure % rest {"), ["command:\\%", "comment:% rest {"]);
    eq!(d("cost \\$5 and \\$6"), ["command:\\$", "command:\\$"], "escaped dollars never open math");
    eq!(d("\\@ifnextchar and \\makeatletter"), ["command:\\@ifnextchar", "command:\\makeatletter"], "@ is a letter in control words");
    eq!(d("Hello world. Plain prose, with commas; (parens) and [brackets] \"quotes\"."), [], "plain prose yields no tokens");
    eq!(d("a & b ~ c ^ d _ e # f"), ["specialCharacter:&", "specialCharacter:~", "specialCharacter:^", "specialCharacter:_", "specialCharacter:#"]);
    eq!(d(""), []);
    eq!(d("\\"), ["command:\\"], "lone trailing backslash");
    eq!(d("\\)"), ["command:\\)"], "stray \\) is an ordinary command");
    eq!(d("x \\\ny"), ["command:\\"], "backslash before a line break");
}

#[test]
fn structure_commands_braces_brackets() {
    eq!(d("\\section*{A}"), ["structureCommand:\\section*", "brace:{", "brace:}"]);
    eq!(d("\\section[short]{Long}"), ["structureCommand:\\section", "bracket:[", "bracket:]", "brace:{", "brace:}"]);
    for name in ["part", "chapter", "section", "subsection", "subsubsection", "paragraph", "subparagraph"] {
        assert_eq!(d(&format!("\\{name}{{X}}")).first().cloned(), Some(format!("structureCommand:\\{name}")));
        assert_eq!(d(&format!("\\{name}*{{X}}")).first().cloned(), Some(format!("structureCommand:\\{name}*")));
    }
    for name in [
        "documentclass", "usepackage", "RequirePackage", "input", "include", "subfile", "maketitle", "tableofcontents", "appendix",
        "frontmatter", "mainmatter", "backmatter", "bibliography", "printbibliography",
    ] {
        assert_eq!(d(&format!("\\{name}")).first().cloned(), Some(format!("structureCommand:\\{name}")));
    }
    eq!(
        d("{a} [b] \\foo[c]{d}"),
        ["brace:{", "brace:}", "command:\\foo", "bracket:[", "bracket:]", "brace:{", "brace:}"],
        "prose brackets stay plain; brackets after a command are tokens"
    );
    eq!(d("see [1] and [2]"), []);
    eq!(
        d("\\begin{itemize}\\item a\\end{itemize}"),
        [
            "structureCommand:\\begin", "brace:{", "environmentName:itemize", "brace:}", "command:\\item", "structureCommand:\\end", "brace:{",
            "environmentName:itemize", "brace:}"
        ]
    );
    eq!(d("\\begin{tabular}{cc}"), ["structureCommand:\\begin", "brace:{", "environmentName:tabular", "brace:}", "brace:{", "brace:}"]);
    eq!(
        d("\\begin{theorem}[Main result] x"),
        ["structureCommand:\\begin", "brace:{", "environmentName:theorem", "brace:}", "bracket:[", "bracket:]"]
    );
    eq!(d("\\begin {itemize}"), ["structureCommand:\\begin", "brace:{", "environmentName:itemize", "brace:}"], "space before the brace");
}

#[test]
fn comments_and_magic_comments() {
    eq!(d("a % c1 $ { \\x\n\\y"), ["comment:% c1 $ { \\x", "command:\\y"]);
    let magic = "% !TEX root = main.tex\n%!TEX program = xelatex\n  % !TeX spellcheck = pt_BR\n% !BIB program = biber\n% just a comment\ntext % !TEX root = nope";
    eq!(
        d(magic),
        [
            "magicComment:% !TEX root = main.tex",
            "magicComment:%!TEX program = xelatex",
            "magicComment:% !TeX spellcheck = pt_BR",
            "magicComment:% !BIB program = biber",
            "comment:% just a comment",
            "comment:% !TEX root = nope",
        ]
    );
    eq!(d("% !TEXT not magic"), ["comment:% !TEXT not magic"]);
    eq!(d("% c\r\n\\foo"), ["comment:% c", "command:\\foo"], "comment ends at CR");
    eq!(d("%"), ["comment:%"]);
    eq!(d("% $ unbalanced\n$x$"), ["comment:% $ unbalanced", "mathDelimiter:$", "math:x", "mathDelimiter:$"], "comment does not open math");
}

#[test]
fn math_modes() {
    eq!(d("$x^2$"), ["mathDelimiter:$", "math:x", "specialCharacter:^", "math:2", "mathDelimiter:$"]);
    eq!(d("$$a+b$$"), ["mathDelimiter:$$", "math:a+b", "mathDelimiter:$$"]);
    eq!(d("\\(a\\)"), ["mathDelimiter:\\(", "math:a", "mathDelimiter:\\)"]);
    eq!(d("\\[ \\alpha + 1 \\]"), ["mathDelimiter:\\[", "mathCommand:\\alpha", "math:+ 1", "mathDelimiter:\\]"]);
    eq!(d("$a$ and $b$"), ["mathDelimiter:$", "math:a", "mathDelimiter:$", "mathDelimiter:$", "math:b", "mathDelimiter:$"]);
    eq!(
        d("$\\text{if } x$"),
        ["mathDelimiter:$", "mathCommand:\\text", "brace:{", "math:if", "brace:}", "math:x", "mathDelimiter:$"]
    );
    eq!(d("$x + y"), ["mathDelimiter:$", "math:x + y"], "unterminated math flushes at EOF");
    eq!(
        d("\\begin{equation}x\\end{equation}"),
        [
            "mathDelimiter:\\begin", "brace:{", "environmentName:equation", "brace:}", "math:x", "mathDelimiter:\\end", "brace:{",
            "environmentName:equation", "brace:}"
        ]
    );
    eq!(
        d("\\begin{align*}a&b\\\\c\\end{align*}"),
        [
            "mathDelimiter:\\begin", "brace:{", "environmentName:align*", "brace:}", "math:a", "specialCharacter:&", "math:b", "mathCommand:\\\\",
            "math:c", "mathDelimiter:\\end", "brace:{", "environmentName:align*", "brace:}"
        ]
    );
    for name in [
        "equation", "equation*", "align", "align*", "alignat", "alignat*", "flalign", "flalign*", "gather", "gather*", "multline", "multline*",
        "eqnarray", "eqnarray*", "math", "displaymath", "dmath",
    ] {
        let t = d(&format!("\\begin{{{name}}}\\alpha\\end{{{name}}} \\alpha"));
        assert!(t.contains(&"mathCommand:\\alpha".to_string()), "{name}: \\alpha inside is a math command");
        assert!(t.contains(&"mathDelimiter:\\begin".to_string()), "{name}: \\begin is a math delimiter");
        assert_eq!(t.last().map(String::as_str), Some("command:\\alpha"), "{name}: back to text mode after \\end");
    }
    // Text environments nested inside math keep math mode afterwards.
    let nested = d("\\begin{equation}\\begin{split}a\\end{split}b\\end{equation} c \\foo");
    assert!(nested.contains(&"math:b".to_string()), "math continues after a nested non-math environment: {nested:?}");
    assert_eq!(nested.last().map(String::as_str), Some("command:\\foo"));
    // A non-math environment does not turn math on.
    eq!(has_prefix(d("\\begin{itemize}\\alpha\\end{itemize}"), "math"), []);
}

#[test]
fn math_nested_in_a_group() {
    eq!(
        d("$a \\text{if $b$ then} c$"),
        [
            "mathDelimiter:$", "math:a", "mathCommand:\\text", "brace:{", "math:if", "mathDelimiter:$", "math:b", "mathDelimiter:$", "math:then",
            "brace:}", "math:c", "mathDelimiter:$"
        ],
        "dollar pair inside a text group does not close the outer formula"
    );
    eq!(d("$a {b} c$ d"), ["mathDelimiter:$", "math:a", "brace:{", "math:b", "brace:}", "math:c", "mathDelimiter:$"]);
    eq!(
        d("$\\text{$b$}$ \\foo"),
        [
            "mathDelimiter:$", "mathCommand:\\text", "brace:{", "mathDelimiter:$", "math:b", "mathDelimiter:$", "brace:}", "mathDelimiter:$",
            "command:\\foo"
        ]
    );
    eq!(has_prefix(d("{ $x$ } $y$ \\foo"), "command"), ["command:\\foo"], "a group opened before the formula does not confuse it");
    assert_eq!(d("\\[ \\text{$x$} \\] \\foo").last().map(String::as_str), Some("command:\\foo"));
}

#[test]
fn blank_line_ends_inline_math() {
    eq!(d("$x\n\n\\alpha $y$"), ["mathDelimiter:$", "math:x", "command:\\alpha", "mathDelimiter:$", "math:y", "mathDelimiter:$"]);
    eq!(d("\\(x\n \t\n\\alpha"), ["mathDelimiter:\\(", "math:x", "command:\\alpha"], "whitespace-only line also ends it");
    eq!(d("$x\r\n\r\n\\alpha"), ["mathDelimiter:$", "math:x", "command:\\alpha"], "CRLF blank line");
    eq!(d("$$a\n\nb$$"), ["mathDelimiter:$$", "math:a\n\nb", "mathDelimiter:$$"], "display math survives blank lines");
    eq!(d("$x\ny$"), ["mathDelimiter:$", "math:x\ny", "mathDelimiter:$"], "single newline does not end it");
}

#[test]
fn verbatim() {
    eq!(
        d("\\begin{verbatim}\\foo % $ { \\end{verbatim}"),
        [
            "structureCommand:\\begin", "brace:{", "environmentName:verbatim", "brace:}", "verbatim:\\foo % $ { ", "structureCommand:\\end", "brace:{",
            "environmentName:verbatim", "brace:}"
        ]
    );
    eq!(has_prefix(d("\\begin{verbatim*}x\\end{verbatim*}"), "verbatim"), ["verbatim:x"]);
    eq!(
        d("\\begin{lstlisting}[language=C]\nint x;\n\\end{lstlisting}"),
        [
            "structureCommand:\\begin", "brace:{", "environmentName:lstlisting", "brace:}", "bracket:[", "bracket:]", "verbatim:\nint x;\n",
            "structureCommand:\\end", "brace:{", "environmentName:lstlisting", "brace:}"
        ]
    );
    eq!(
        d("\\begin{minted}{python}\nx = {1}\n\\end{minted}"),
        [
            "structureCommand:\\begin", "brace:{", "environmentName:minted", "brace:}", "brace:{", "brace:}", "verbatim:\nx = {1}\n",
            "structureCommand:\\end", "brace:{", "environmentName:minted", "brace:}"
        ]
    );
    eq!(has_prefix(d("\\begin{Verbatim}\n\\x\n\\end{Verbatim}"), "verbatim"), ["verbatim:\n\\x\n"]);
    let filtered: Vec<String> = d("\\begin{comment}\n\\section{x}\n\\end{comment}\\foo")
        .into_iter()
        .filter(|s| s.starts_with("verbatim") || s == "command:\\foo")
        .collect();
    eq!(filtered, ["verbatim:\n\\section{x}\n", "command:\\foo"], "comment environment");
    assert_eq!(d("\\begin{verbatim}\nabc").last().map(String::as_str), Some("verbatim:\nabc"), "unterminated verbatim runs to EOF");
    eq!(d("\\verb|a % b|"), ["command:\\verb", "verbatim:|a % b|"]);
    eq!(d("\\verb*+x+"), ["command:\\verb*", "verbatim:+x+"]);
    eq!(d("\\verb|x| \\foo"), ["command:\\verb", "verbatim:|x|", "command:\\foo"]);
    eq!(d("\\verb|abc\nnext"), ["command:\\verb"], "no closing delimiter on the line");
    eq!(d("\\verb+$+ and $x$"), ["command:\\verb", "verbatim:+$+", "mathDelimiter:$", "math:x", "mathDelimiter:$"], "$ inside \\verb is inert");
    eq!(d("\\url{http://a.b/c%d#e}"), ["command:\\url", "brace:{", "verbatim:http://a.b/c%d#e", "brace:}"]);
    eq!(
        d("\\href{http://x.org/a%20b}{text \\alpha}"),
        ["command:\\href", "brace:{", "verbatim:http://x.org/a%20b", "brace:}", "brace:{", "command:\\alpha", "brace:}"]
    );
}

#[test]
fn reference_arguments() {
    eq!(d("\\label{eq:1}"), ["command:\\label", "brace:{", "referenceArgument:eq:1", "brace:}"]);
    eq!(d("\\cite[p. 3]{knuth}"), ["command:\\cite", "bracket:[", "bracket:]", "brace:{", "referenceArgument:knuth", "brace:}"]);
    eq!(d("\\input{sections/intro}"), ["structureCommand:\\input", "brace:{", "referenceArgument:sections/intro", "brace:}"]);
    eq!(
        d("\\usepackage[utf8]{inputenc}"),
        ["structureCommand:\\usepackage", "bracket:[", "bracket:]", "brace:{", "referenceArgument:inputenc", "brace:}"]
    );
    eq!(
        d("\\includegraphics[width=3cm]{fig.png}"),
        ["command:\\includegraphics", "bracket:[", "bracket:]", "brace:{", "referenceArgument:fig.png", "brace:}"]
    );
    assert_eq!(d("\\cref{a,b}").get(2).map(String::as_str), Some("referenceArgument:a,b"));
    eq!(d("\\ref{}"), ["command:\\ref", "brace:{", "brace:}"], "empty argument has no token");
    eq!(d("\\label{abc"), ["command:\\label", "brace:{"], "unclosed argument degrades gracefully");
    for name in [
        "label", "ref", "eqref", "cref", "Cref", "autoref", "pageref", "nameref", "cite", "citep", "citet", "citeauthor", "parencite", "textcite",
        "autocite", "footcite", "nocite", "Cite", "supercite", "input", "include", "subfile", "includegraphics", "includepdf", "usepackage",
        "documentclass", "bibliography", "addbibresource", "bibliographystyle", "lstinputlisting",
    ] {
        assert!(d(&format!("\\{name}{{key}}")).contains(&"referenceArgument:key".to_string()), "\\{name} takes a reference argument");
    }
    for name in ["textbf", "emph", "citestyle", "texttt", "hspace"] {
        assert!(!d(&format!("\\{name}{{key}}")).contains(&"referenceArgument:key".to_string()), "\\{name} does not");
    }
    eq!(only("\\label{a}\\ref{b}", &[TokenKind::ReferenceArgument]), ["referenceArgument:a", "referenceArgument:b"]);
}

#[test]
fn utf16_ranges() {
    let text = "ação \\alpha é $é$ 😀 \\beta \\😀 {é}";
    eq!(
        d(text),
        ["command:\\alpha", "mathDelimiter:$", "math:é", "mathDelimiter:$", "command:\\beta", "command:\\😀", "brace:{", "brace:}"]
    );
    let tokens = tokenize(text);
    assert_eq!((tokens[0].start, tokens[0].length), (5, 6), "location after 'ação ' is 5");
    assert_eq!(tokens.iter().find(|t| t.kind == TokenKind::Command && t.length == 3).map(|t| t.length), Some(3), "\\ + emoji = 3 units");
    let t = tokenize("😀\\x");
    assert_eq!((t[0].start, t[0].length), (2, 2), "emoji counts as two units");
    let t = tokenize("e\u{301}\\x");
    assert_eq!((t[0].start, t[0].length), (2, 2), "combining accent counts");
    let t = tokenize("é%x");
    assert_eq!((t[0].start, t[0].length), (1, 2));
    eq!(d("é\\foo ação % ção\n\\bar"), ["command:\\foo", "comment:% ção", "command:\\bar"]);
}

#[test]
fn invariants_on_fuzzed_input() {
    let pieces = [
        "\\", "{", "}", "[", "]", "$", "$$", "%", "&", "~", "^", "_", "#", "a", "b", " ", "\n", "\n\n", "\r\n", "\\begin{equation}", "\\end{equation}",
        "\\begin{verbatim}", "\\end{verbatim}", "\\verb|", "|", "\\label{", "\\cite[", "\\(", "\\)", "\\[", "\\]", "é", "😀", "\\section{", "\\url{",
        "\\begin{minted}{c}", "% !TEX root = x", "*", "\\\\", "\\%", "\\$",
    ];
    let mut rng = Lcg(20260607);
    let mut violations = 0;
    for _ in 0..400 {
        let mut text = String::new();
        for _ in 0..(1 + rng.next() % 60) {
            text.push_str(pieces[rng.next() % pieces.len()]);
        }
        let length = text.encode_utf16().count();
        let mut previous_end = 0;
        for token in tokenize(&text) {
            if token.start < previous_end || token.length == 0 || token.end() > length {
                violations += 1;
                break;
            }
            previous_end = token.end();
        }
    }
    assert_eq!(violations, 0, "tokens sorted, non-overlapping, in bounds, non-empty");
}

#[test]
fn performance_100kb() {
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
        Plain prose without much markup, just words and more words to dilute the density of the commands in ação.\n\n";
    let mut doc = String::new();
    while doc.encode_utf16().count() < 100_000 {
        doc.push_str(block);
    }
    let u = crate::core::scanner::syntax::units(&doc);
    let mut count = 0;
    for _ in 0..5 {
        count = tokenize_units(&u).len();
    }
    let mut times: Vec<f64> = Vec::new();
    for _ in 0..30 {
        let start = std::time::Instant::now();
        count = tokenize_units(&u).len();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = times[times.len() / 2];
    let limit = if cfg!(debug_assertions) { 400.0 } else { 10.0 };
    eprintln!("TIMING tokenizer: {} UTF-16 units, {count} tokens, min {:.2} ms, median {median:.2} ms", u.len(), times[0]);
    assert!(count > 5_000, "document produced a plausible number of tokens ({count})");
    assert!(median < limit, "100 KB tokenized in {median:.2} ms (limit {limit} ms)");
}

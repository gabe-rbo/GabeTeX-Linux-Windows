//! Port of `ScannerChecks.swift`, `BibChecks.swift` and the `LineIndex` check of `ModelChecks.swift`.

use super::tests_common::*;
use crate::core::scanner::bib;
use crate::core::scanner::latex_scanner as ls;
use crate::core::scanner::models::{IncludeReference, MagicComments, OutlineKind, OutlineEntry, BibRecord};
use crate::core::scanner::*;

fn described(refs: &[IncludeReference]) -> Vec<String> {
    refs.iter()
        .map(|r| format!("{}|{}|{}|{}", r.kind.raw(), r.argument, r.directory.as_deref().unwrap_or("-"), r.line))
        .collect()
}

fn args_of(text: &str) -> Vec<String> {
    ls::includes(text).into_iter().map(|r| r.argument).collect()
}

#[test]
fn scanner_include_kinds() {
    let text = "\\documentclass[a4paper]{article}\n\
        \\usepackage{amsmath, tikz}\n\
        \\usepackage[utf8]{inputenc}\n\
        \\input{sections/intro}\n\
        \\include{chapters/one}\n\
        \\subfile{sub/a}\n\
        \\import{dir/}{file}\n\
        \\subimport{sub/}{part}\n\
        \\bibliography{refs,more}\n\
        \\addbibresource{refs.bib}\n\
        \\includegraphics[width=3cm]{figs/plot}\n\
        \\includepdf[pages=-]{doc.pdf}\n\
        \\lstinputlisting[language=C]{code.c}\n\
        \\inputminted{python}{script.py}\n\
        \\input foo.tex\n\
        % \\input{commented}\n\
        \\verb|\\input{verb}| and \\url{http://x.org/\\input{no}}";
    let refs = ls::includes(text);
    assert_eq!(
        described(&refs),
        vec![
            "documentclass|article|-|1",
            "usepackage|amsmath|-|2",
            "usepackage|tikz|-|2",
            "usepackage|inputenc|-|3",
            "input|sections/intro|-|4",
            "include|chapters/one|-|5",
            "subfile|sub/a|-|6",
            "importFile|file|dir/|7",
            "subimport|part|sub/|8",
            "bibliography|refs|-|9",
            "bibliography|more|-|9",
            "addbibresource|refs.bib|-|10",
            "includegraphics|figs/plot|-|11",
            "includepdf|doc.pdf|-|12",
            "lstinputlisting|code.c|-|13",
            "inputminted|script.py|-|14",
            "input|foo.tex|-|15",
        ]
    );
    // Every range points at its argument in the UTF-16 text.
    for r in &refs {
        assert_eq!(substring(text, r.range.clone()), r.argument, "range of {} {}", r.kind.raw(), r.argument);
    }
}

#[test]
fn scanner_arguments() {
    let list = "\\usepackage{amsmath,amssymb , amsthm}";
    let refs = ls::includes(list);
    assert_eq!(refs.iter().map(|r| r.argument.as_str()).collect::<Vec<_>>(), ["amsmath", "amssymb", "amsthm"]);
    for r in &refs {
        assert_eq!(substring(list, r.range.clone()), r.argument);
    }
    assert_eq!(args_of("\\usepackage{ a ,\n b }"), ["a", "b"]);
    assert_eq!(args_of("\\bibliography{a,,b,}"), ["a", "b"], "empty items dropped");
    assert_eq!(args_of("\\includegraphics*[scale=.5]{a.png}"), ["a.png"], "star + optional");
    assert_eq!(args_of("\\includegraphics[width={0.5\\textwidth},angle=90]{b.png}"), ["b.png"], "braces inside the optional argument");
    assert_eq!(args_of("\\input {spaced}"), ["spaced"], "space before brace");
    assert_eq!(args_of("\\input\n{nl}"), ["nl"], "newline before brace");
    assert_eq!(args_of("\\input{a%b\n}"), ["a"], "comment inside an argument is ignored");
    assert_eq!(args_of("\\input chapter.tex\n\\foo"), ["chapter.tex"], "\\input file form");
    let primitive = ls::includes("x\n\\input chapter.tex\n");
    assert_eq!(primitive.first().map(|r| substring("x\n\\input chapter.tex\n", r.range.clone())), Some("chapter.tex".to_string()));
    assert_eq!(ls::includes("\\input{#1}").len(), 0, "macro parameters are not files");
    assert_eq!(ls::includes("\\input{\\jobname.aux}").len(), 0, "macros are not files");
    assert_eq!(ls::includes("\\\\input{x}").len(), 0, "\\\\ is a line break, `input{{x}}` is plain text");
    assert_eq!(args_of("\\\\\\input{x}"), ["x"], "\\\\ followed by \\input");
    assert_eq!(args_of("\\%\\input{x}"), ["x"], "escaped percent is not a comment");
    assert_eq!(args_of("\\begin{verbatim}\n\\input{x}\n\\end{verbatim}\n\\input{y}"), ["y"], "verbatim ignored");
    assert_eq!(ls::includes("\\begin{lstlisting}[x]\n\\input{x}\n\\end{lstlisting}").len(), 0);
    assert_eq!(args_of("\\iffalse\n\\input{x}\n\\fi\n\\input{y}"), ["y"], "\\iffalse block");
    assert_eq!(args_of("ação é \\input{ção}"), ["ção"]);
    assert_eq!(ls::includes("😀\\input{x}").first().map(|r| r.range.clone()), Some(9..10), "UTF-16 offsets after an emoji");
    assert_eq!(ls::includes("a\r\nb\r\n\\input{x}").first().map(|r| r.line), Some(3), "CRLF line numbers");
    assert_eq!(ls::includes("a\rb\r\\input{x}").first().map(|r| r.line), Some(3), "CR line numbers");
    assert_eq!(args_of("\\input{a{b}c}"), ["a{b}c"], "nested braces");
    assert_eq!(ls::includes("\\input{unclosed").len(), 0, "unterminated argument");
}

#[test]
fn scanner_magic_comments_and_root() {
    let text = "% !TEX root = ../main.tex\n% !TEX TS-program = xelatex\n% !TeX SPELLCHECK = pt_BR\n% !BIB program = biber\n\\section{x}";
    let m = magic_comments(text);
    assert_eq!(m.root.as_deref(), Some("../main.tex"));
    assert_eq!(m.program.as_deref(), Some("xelatex"));
    assert_eq!(m.spellcheck.as_deref(), Some("pt_BR"));
    assert_eq!(m.bib_program.as_deref(), Some("biber"));
    assert_eq!(magic_comments("%!TEX program = lualatex\n").program.as_deref(), Some("lualatex"));
    assert_eq!(magic_comments("% !TEX program = xelatex\n% !TEX program = lualatex").program.as_deref(), Some("xelatex"), "first wins");
    assert_eq!(magic_comments("% !TEX bib = biber").bib_program.as_deref(), Some("biber"));
    assert_eq!(magic_comments("% !TEX encoding = UTF-8 Unicode\n% !TEX root = r.tex").root.as_deref(), Some("r.tex"));
    assert_eq!(magic_comments("% !TeX ROOT = Upper.tex").root.as_deref(), Some("Upper.tex"), "keys are case-insensitive");
    assert_eq!(magic_comments("% just text\n"), MagicComments::default());
    assert_eq!(magic_comments("x % !TEX root = a.tex"), MagicComments::default(), "must start the line");
    assert_eq!(magic_comments("% !TEX root =\n"), MagicComments::default(), "empty value ignored");
    let late = format!("{}% !TEX root = late.tex", "\n".repeat(25));
    assert_eq!(magic_comments(&late).root, None, "only the first 20 lines are searched");
    let line20 = format!("{}% !TEX root = l20.tex", "\n".repeat(19));
    assert_eq!(magic_comments(&line20).root.as_deref(), Some("l20.tex"), "line 20 is still inside");
    let busy = format!("{}\n% !TEX root = l20.tex\nmore\nlines\n", (1..20).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n"));
    assert_eq!(magic_comments(&busy).root.as_deref(), Some("l20.tex"), "value stops at the end of line 20");
    assert_eq!(magic_comments("% !TEX root = a b.tex\r\n% !TEX program = xelatex\r\n").root.as_deref(), Some("a b.tex"), "CRLF and spaces");
    assert_eq!(magic_comments("% !TEX root = a b.tex\r\n% !TEX program = xelatex\r\n").program.as_deref(), Some("xelatex"));

    assert!(is_root("\\documentclass{article}\n"));
    assert!(!is_root("% \\documentclass{article}\n"));
    assert!(!is_root("\\section{x}"));
    assert!(is_root("\\begin{document}\nx\\end{document}"));
    assert!(!is_root("\\begin{verbatim}\n\\documentclass{article}\n\\end{verbatim}"));
}

fn is_root(text: &str) -> bool {
    ls::is_root_document(text)
}

fn outline_desc(entries: &[OutlineEntry]) -> Vec<String> {
    entries.iter().map(|e| format!("{}|{}|{}|{}", e.kind.raw(), e.title, e.is_starred, e.line)).collect()
}

#[test]
fn scanner_outline_entries() {
    let file = url("main.tex");
    let text = "\\part{Part One}\n\
        \\chapter{Chapter}\n\
        \\section{Intro \\label{sec:intro}}\n\
        \\section*{Unnumbered}\n\
        \\subsection[Short]{The {nested} long   title\n  over two lines}\n\
        % \\section{commented}\n\
        \\subsubsection{Deep}\n\
        \\paragraph{Para}\n\
        \\subparagraph{Sub-para}\n\
        \\section{ação — é}\n\
        \\label{lab:top}\n\
        \\section\\relax{Weird}";
    let entries = ls::outline_entries(text, &file);
    assert_eq!(
        outline_desc(&entries),
        vec![
            "part|Part One|false|1",
            "chapter|Chapter|false|2",
            "section|Intro|false|3",
            "label|sec:intro|false|3",
            "section|Unnumbered|true|4",
            "subsection|The {nested} long title over two lines|false|5",
            "subsubsection|Deep|false|8",
            "paragraph|Para|false|9",
            "subparagraph|Sub-para|false|10",
            "section|ação — é|false|11",
            "label|lab:top|false|12",
        ]
    );
    let units_text = crate::core::scanner::syntax::units(text);
    for e in &entries {
        assert_eq!(&*e.file, file.as_str());
        assert_eq!(units_text[e.offset], 92, "offset of {} is the backslash", e.title);
    }
    let first = entries.iter().find(|e| e.kind == OutlineKind::Section && e.title == "Intro");
    assert_eq!(first.map(|e| LineIndex::new(text).line_of_offset(e.offset)), Some(3));

    let beamer = "\\begin{frame}{First}\nx\n\\end{frame}\n\\begin{frame}[t]{Second}\n\\end{frame}\n\\begin{frame}\n  \\frametitle{Third}\n  \\label{fr:3}\n\\end{frame}\n\\begin{frame}<1->[fragile]{Fourth}\n\\end{frame}\n\\begin{frame}[plain]\n\\end{frame}\n\\frametitle{Loose}";
    let frames: Vec<String> =
        ls::outline_entries(beamer, &file).iter().map(|e| format!("{}|{}|{}", e.kind.raw(), e.title, e.line)).collect();
    assert_eq!(frames, ["frame|First|1", "frame|Second|4", "frame|Third|6", "label|fr:3|8", "frame|Fourth|10", "frame|Loose|14"], "untitled frames without \\frametitle are dropped");
}

#[test]
fn scanner_labels_and_context() {
    let file = url("main.tex");
    let text = "\\section{Alpha}\n\
        \\label{sec:a}\n\
        \\begin{equation}\\label{eq:1} x \\end{equation}\n\
        \\begin{theorem}\\label{thm:1} t \\end{theorem}\n\
        \\begin{figure}\\begin{center}\\label{fig:1}\\end{center}\\end{figure}\n\
        \\begin{align}\n  a &= b \\label{eq:2} \\\\\n  c &= d\n\\end{align}\n\
        \\subsection{Beta}\n\
        \\label{sub:b}\n\
        \\begin{itemize}\\item\\label{it:1} x\\end{itemize}\n\
        % \\label{commented}\n\
        \\label{ação:1}";
    let labels = ls::labels(text, &file);
    let d: Vec<String> = labels.iter().map(|l| format!("{}|{}|{}", l.key, l.context.as_deref().unwrap_or("nil"), l.line)).collect();
    assert_eq!(
        d,
        vec!["sec:a|Alpha|2", "eq:1|equation|3", "thm:1|theorem|4", "fig:1|figure|5", "eq:2|align|7", "sub:b|Beta|11", "it:1|itemize|12", "ação:1|Beta|14"]
    );
    assert_eq!(ls::labels("\\label{x}", &file).first().and_then(|l| l.context.clone()), None, "no context outside headings and environments");
    assert_eq!(
        ls::labels("\\begin{document}\\label{x}\\end{document}", &file).first().and_then(|l| l.context.clone()),
        None,
        "document is a layout wrapper"
    );
    assert_eq!(ls::labels("\\label{a#1}\\label{\\foo}", &file).len(), 0);
}

#[test]
fn scanner_custom_commands() {
    let text = "\\newcommand{\\foo}[2]{#1#2}\n\
        \\newcommand\\bar{x}\n\
        \\newcommand{\\opt}[2][d]{#1#2}\n\
        \\renewcommand{\\ren}[1]{#1}\n\
        \\providecommand*{\\prov}{}\n\
        \\def\\baz#1#2{#1#2}\n\
        \\DeclareMathOperator*{\\argmin}{arg\\,min}\n\
        \\DeclareMathOperator{\\Hom}{Hom}\n\
        \\NewDocumentCommand{\\xx}{m o m}{#1}\n\
        \\NewDocumentCommand{\\yy}{O{x} m +m}{#1}\n\
        \\DeclareDocumentCommand\\zz{ m }{#1}\n\
        \\DeclarePairedDelimiter{\\abs}{\\lvert}{\\rvert}\n\
        \\newcommand{\\foo}{dup}\n\
        % \\newcommand{\\hidden}{x}\n\
        \\newcommand{\\no@letters2}{x}\n\
        \\begin{verbatim}\n\\newcommand{\\inverb}{x}\n\\end{verbatim}";
    let items = ls::custom_commands(text);
    let d: Vec<String> = items.iter().map(|i| format!("{}={}", i.label, i.insert_text)).collect();
    assert_eq!(
        d,
        vec![
            "\\foo=\\foo{${1:arg}}{${2:arg}}",
            "\\bar=\\bar",
            "\\opt=\\opt{${1:arg}}",
            "\\ren=\\ren{${1:arg}}",
            "\\prov=\\prov",
            "\\baz=\\baz{${1:arg}}{${2:arg}}",
            "\\argmin=\\argmin",
            "\\Hom=\\Hom",
            "\\xx=\\xx{${1:arg}}{${2:arg}}",
            "\\yy=\\yy{${1:arg}}{${2:arg}}",
            "\\zz=\\zz{${1:arg}}",
            "\\abs=\\abs{${1:arg}}",
        ],
        "deduplicated, in source order"
    );
    assert!(items.iter().all(|i| i.kind == "command" && i.detail.as_deref() == Some("Defined in project")));
}

#[test]
fn scanner_custom_environments() {
    let text = "\\newenvironment{myenv}{a}{b}\n\
        \\renewenvironment{renv}{a}{b}\n\
        \\newtheorem{thm}{Theorem}[section]\n\
        \\newtheorem*{rem}{Remark}\n\
        \\declaretheorem[style=plain]{lemma}\n\
        \\declaretheorem{prop, cor}\n\
        \\NewDocumentEnvironment{docenv}{m}{a}{b}\n\
        \\newtcolorbox{box}{colback=red}\n\
        \\newtcbtheorem{ex}{Example}{}{ex}\n\
        \\newtheorem{thm}{Duplicate}\n\
        % \\newenvironment{hidden}{}{}\n\
        \\newenvironment{\\bad}{}{}";
    assert_eq!(ls::custom_environments(text), ["myenv", "renv", "thm", "rem", "lemma", "prop", "cor", "docenv", "box", "ex"]);
}

// ---------------------------------------------------------------------------------------------------
// BibParser
// ---------------------------------------------------------------------------------------------------

fn parse(text: &str) -> Vec<BibRecord> {
    bib::entries(text, &url("refs.bib"))
}

fn summary(e: &BibRecord) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        e.entry_type,
        e.key,
        e.author.as_deref().unwrap_or("-"),
        e.title.as_deref().unwrap_or("-"),
        e.year.as_deref().unwrap_or("-"),
        e.line
    )
}

#[test]
fn bib_typical_file() {
    let text = "% leading comment\n\
        @comment{ignore {nested} me}\n\
        @preamble{\"\\newcommand{\\noop}[1]{}\"}\n\
        @string{jan = \"January\"}\n\
        \n\
        Some free text between entries (and an email me@example.com).\n\
        \n\
        @article{knuth1984,\n\
        \x20 author  = {Knuth, Donald E.},\n\
        \x20 title   = {The {TeX}book},\n\
        \x20 year    = 1984,\n\
        \x20 month   = jan,\n\
        }\n\
        @BOOK{Hatcher2002, AUTHOR=\"Hatcher, Allen\" # \" and Other\", Title = \"Algebraic {Topology}\", year = \"2002\" }\n\
        @book(parens, title = {Parenthesised}, year = {1999})\n\
        % @article{commented, title = {No}}\n\
        @misc{nofields}\n\
        @inproceedings{multi,\n\
        \x20 title = {A very\n\
        \x20          long title   spanning lines},\n\
        \x20 author = {Doe, Jane and\n\
        \x20           Roe, Richard},\n\
        \x20 date = {2020-05-01},\n\
        }";
    let entries = parse(text);
    assert_eq!(
        entries.iter().map(summary).collect::<Vec<_>>(),
        vec![
            "article|knuth1984|Knuth, Donald E.|The TeXbook|1984|8",
            "book|Hatcher2002|Hatcher, Allen and Other|Algebraic Topology|2002|14",
            "book|parens|-|Parenthesised|1999|15",
            "misc|nofields|-|-|-|17",
            "inproceedings|multi|Doe, Jane and Roe, Richard|A very long title spanning lines|2020|18",
        ]
    );
    assert!(entries.iter().all(|e| &*e.file == url("refs.bib").as_str()));
}

fn keys(text: &str) -> Vec<String> {
    parse(text).into_iter().map(|e| e.key).collect()
}

fn first(text: &str) -> BibRecord {
    parse(text).into_iter().next().expect("an entry")
}

#[test]
fn bib_edge_cases() {
    // Recovery from a missing closing brace.
    let broken = parse("@article{broken, title = {Unclosed\n@book{good, title = {Fine}}\n");
    assert_eq!(broken.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), ["broken", "good"]);
    assert_eq!(broken.last().unwrap().title.as_deref(), Some("Fine"));
    assert_eq!(broken.last().unwrap().line, 2);

    // CRLF and CR line endings.
    let crlf = parse("@article{a,\r\n title = {T},\r\n year = 2000\r\n}\r\n@book{b, title={U}}");
    assert_eq!(crlf.iter().map(|e| format!("{}:{}", e.key, e.line)).collect::<Vec<_>>(), ["a:1", "b:5"]);
    assert_eq!(crlf[0].year.as_deref(), Some("2000"));
    let cr = parse("@article{a,\r title = {T}\r}\r@book{b, title={U}}");
    assert_eq!(cr.iter().map(|e| format!("{}:{}", e.key, e.line)).collect::<Vec<_>>(), ["a:1", "b:4"]);

    // EOF inside an entry.
    assert_eq!(keys("@article{eof, title = {abc"), ["eof"]);
    assert_eq!(keys("@article{eof,"), ["eof"]);
    assert_eq!(parse("@article{").len(), 0);
    assert_eq!(parse("").len(), 0);
    assert_eq!(parse("no entries here @ all").len(), 0);

    // Keys with punctuation, trailing comma, empty entry.
    assert_eq!(first("@article{Doe:2020-ab_c+d/x, title={x}}").key, "Doe:2020-ab_c+d/x");
    assert_eq!(keys("@article{k,}"), ["k"]);
    assert_eq!(first("@article{ spaced , title = {x} }").key, "spaced", "key is trimmed");
    assert_eq!(first("@Article{Mixed, Title = {x}}").entry_type, "article", "types are lowercased, keys keep their case");
    assert_eq!(first("@Article{Mixed, Title = {x}}").key, "Mixed");
    assert_eq!(first("@online{k, url = {http://a.b/c%20d}, title = {t}}").title.as_deref(), Some("t"), "% inside a value is not a comment");

    // Values.
    assert_eq!(first("@a{k, title = {Cost: \\{x\\}}}").title.as_deref(), Some("Cost: {x}"), "escaped braces survive");
    assert_eq!(first("@a{k, author = {{Barnes and Noble, Inc.}}}").author.as_deref(), Some("Barnes and Noble, Inc."), "protective braces removed");
    assert_eq!(first("@a{k, title = \"Say {\\\"o}\"}").title.as_deref(), Some("Say \\\"o"), "braced quote inside a quoted value");
    assert_eq!(first("@a{k, author = {A} # { and B}}").author.as_deref(), Some("A and B"), "concatenation with braces");
    assert_eq!(first("@a{k, author = \"A\" # \" and \" # \"B\"}").author.as_deref(), Some("A and B"), "concatenation keeps inner spaces");
    assert_eq!(first("@a{k, title = {  padded   value  }}").title.as_deref(), Some("padded value"));
    assert_eq!(first("@a{k, editor = {Ed, Itor}}").author.as_deref(), Some("Ed, Itor"), "editor is used when there is no author");
    assert_eq!(first("@a{k, author = {Au}, editor = {Ed}}").author.as_deref(), Some("Au"));
    assert_eq!(first("@a{k, date = {2019-12-31}}").year.as_deref(), Some("2019"));
    assert_eq!(first("@a{k, year = {2001}, date = {2019-12-31}}").year.as_deref(), Some("2001"), "explicit year wins");
    assert_eq!(first("@a{k, TITLE = {upper}, title = {second}}").title.as_deref(), Some("upper"), "first field wins");
    assert_eq!(first("@a{k, volume = 12, title = {t}}").title.as_deref(), Some("t"), "bare numbers");
    assert_eq!(first("@a{k, title = {Ação — é 😀}, year = {2020}}").title.as_deref(), Some("Ação — é 😀"));

    // Skipped entry types do not leak.
    assert_eq!(keys("@comment{@article{inside, title={x}}}\n@article{real, title={y}}"), ["real"]);
    assert_eq!(keys("@string{a = \"x\"}\n@string(b = \"y\")\n@article{real}"), ["real"]);
    assert_eq!(keys("@preamble{ \"\\usepackage{x}\" }\n@article{real}"), ["real"]);

    // Many entries.
    let many: Vec<String> = (0..500).map(|i| format!("@article{{key{i}, title = {{Title {i}}}, year = {}}}", 1500 + i)).collect();
    let parsed = parse(&many.join("\n"));
    assert_eq!(parsed.len(), 500);
    assert_eq!(parsed.last().unwrap().key, "key499");
    assert_eq!(parsed.last().unwrap().line, 500);
}

// ---------------------------------------------------------------------------------------------------
// LineIndex
// ---------------------------------------------------------------------------------------------------

#[test]
fn line_index() {
    let idx = LineIndex::new("ab\ncd\r\nef\rg");
    assert_eq!(idx.line_count(), 4);
    assert_eq!(idx.position_of_offset(3), SourcePosition::new(2, 1));
    assert_eq!(idx.offset_of(SourcePosition::new(3, 2)), 8);
    assert_eq!(idx.contents_range(2), 3..5);
    assert_eq!(LineIndex::new("").line_count(), 1);
    assert_eq!(LineIndex::new("a\n").line_count(), 2);
    assert_eq!(LineIndex::new("é😀x").position_of_offset(3), SourcePosition::new(1, 4), "columns are UTF-16 units");
    // Clamping.
    assert_eq!(idx.offset_of(SourcePosition::new(99, 99)), idx.length);
    assert_eq!(idx.line_of_offset(10_000), 4);
    assert_eq!(idx.full_range(1), 0..3);
}

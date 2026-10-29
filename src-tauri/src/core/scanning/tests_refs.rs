//! Port of `ReferenceChecks.swift`: reference & citation tracking (scanner, index additions, linter).

use crate::core::log_parser::{LogDiagnostic, Severity, Source};
use crate::core::project_index::{build, ProjectModel};
use crate::core::scanner::references::{self as refs, FindingKind, KeyOccurrence, ReferenceScan};
use crate::core::scanner::syntax::units;
use crate::core::scanner::tests_common::*;
use std::collections::HashMap;

/// In-memory project: relative path -> contents (all under /proj).
#[derive(Clone)]
struct RefProject {
    files: HashMap<String, String>,
}

impl RefProject {
    fn new(files: &[(&str, &str)]) -> Self {
        RefProject { files: files.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }
    fn read(&self, path: &str) -> Option<String> {
        path.strip_prefix("/proj/").and_then(|p| self.files.get(p)).cloned()
    }
    fn index(&self) -> ProjectModel {
        build(&url("main.tex"), &|p| self.read(p), &|p| self.read(p).is_some())
    }
    fn lint(&self) -> Vec<LogDiagnostic> {
        refs::diagnostics(&self.index())
    }
    /// The text a diagnostic underlines (line / column / length in UTF-16 units).
    fn underlined(&self, d: &LogDiagnostic) -> Option<String> {
        let text = self.read(d.path.as_deref()?)?;
        let u = units(&text);
        let mut starts = vec![0usize];
        let mut i = 0;
        while i < u.len() {
            if u[i] == 10 {
                starts.push(i + 1);
            } else if u[i] == 13 {
                if i + 1 < u.len() && u[i + 1] == 10 {
                    i += 1;
                }
                starts.push(i + 1);
            }
            i += 1;
        }
        let (line, column, length) = (d.line? as usize, d.column? as usize, d.length? as usize);
        if line < 1 || line > starts.len() {
            return None;
        }
        let location = starts[line - 1] + column - 1;
        if location + length > u.len() {
            return None;
        }
        Some(String::from_utf16_lossy(&u[location..location + length]))
    }
}

fn keys(v: &[KeyOccurrence]) -> Vec<String> {
    v.iter().map(|o| o.key.clone()).collect()
}

fn scan(text: &str) -> ReferenceScan {
    refs::scan_text(text, &url("main.tex"))
}

fn describe(diagnostics: &[LogDiagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|d| {
            let path = d.path.as_deref().map(|p| p.strip_prefix("/proj/").unwrap_or(p)).unwrap_or("?");
            format!("{path}:{} {}", d.line.map(|l| l as i64).unwrap_or(-1), d.message)
        })
        .collect()
}


// ---------------------------------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------------------------------

#[test]
fn scanner_reference_macros() {
    for name in ["ref", "eqref", "pageref", "autoref", "nameref", "vref", "cref", "Cref", "cpageref", "labelcref"] {
        assert!(refs::is_reference_command(name), "\\{name} is a reference command");
        let text = format!("See \\{name}{{a, b ,c}}.");
        let found = scan(&text).references;
        assert_eq!(keys(&found), ["a", "b", "c"], "\\{name} comma list");
        assert_eq!(found.iter().map(|f| f.command.as_str()).collect::<Vec<_>>(), [name, name, name]);
        assert!(found.iter().all(|f| substring(&text, f.range()) == f.key), "\\{name} ranges cover the keys");
        assert_eq!(keys(&scan(&format!("\\{name}*{{x}}")).references), ["x"], "\\{name}* starred");
    }
    let r = scan("See \\ref{a, b ,c}.").references;
    assert_eq!(r.iter().map(|o| o.column).collect::<Vec<_>>(), [10, 13, 16]);
    assert_eq!(r.iter().map(|o| o.offset).collect::<Vec<_>>(), [9, 12, 15]);
    assert_eq!(r.iter().map(|o| o.length).collect::<Vec<_>>(), [1, 1, 1]);
    assert_eq!(r.iter().map(|o| o.line).collect::<Vec<_>>(), [1, 1, 1]);

    for name in ["crefrange", "Crefrange", "cpagerefrange", "Cpagerefrange", "vrefrange", "Vrefrange"] {
        assert!(refs::is_reference_range_command(name));
        assert_eq!(keys(&scan(&format!("\\{name}{{eq:a}}{{eq:b}}")).references), ["eq:a", "eq:b"], "\\{name} both keys");
    }
    assert_eq!(keys(&scan("\\crefrange{ a }{\n b }").references), ["a", "b"], "range keys trimmed");
    assert_eq!(scan("\\crefrange{a}").references.len(), 0, "a range without its second argument is ignored");

    assert_eq!(keys(&scan("\\hyperref[sec:x]{the section}").references), ["sec:x"], "\\hyperref optional label");
    assert_eq!(keys(&scan("\\hyperref{http://x.org}{cat}{name}{text}").references), Vec::<String>::new(), "old \\hyperref form has no label");
    assert_eq!(keys(&scan("\\subref{fig:a} \\Vref{x} \\Cpageref{y} \\namecref{z}").references), ["fig:a", "x", "y", "z"]);
    let lines = scan("\\cref{a,\n      b}").references;
    assert_eq!(lines.iter().map(|o| o.line).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(lines.iter().map(|o| o.column).collect::<Vec<_>>(), [7, 7]);
    assert_eq!(keys(&scan("\\ref {spaced}").references), ["spaced"], "space before the brace");
    assert_eq!(keys(&scan("\\ref\n{next}").references), ["next"], "line break before the brace");
    assert_eq!(keys(&scan("\\ref{a}\\ref{b}\\eqref{c}").references), ["a", "b", "c"], "adjacent commands");
    assert_eq!(keys(&scan("\\footnote{see \\ref{in:footnote}}").references), ["in:footnote"], "inside another argument");
    assert_eq!(keys(&scan("$x = \\text{see \\eqref{eq:in:math}}$").references), ["eq:in:math"], "inside math");

    assert_eq!(
        scan("\\crefname{section}{section}{sections} \\refstepcounter{x} \\refname \\reflectbox{x} \\labelformat{x}{y}").references.len(),
        0,
        "look-alikes are not references"
    );
    assert_eq!(scan("\\ref \\ref} \\ref").references.len(), 0, "no argument, no use");
}

#[test]
fn scanner_citation_macros() {
    let spec = ["cite", "citep", "citet", "citealp", "citeauthor", "citeyear", "parencite", "textcite", "autocite", "footcite", "smartcite", "supercite", "nocite"];
    for name in spec {
        assert!(refs::is_citation_command(name), "\\{name} is a citation command");
        let text = format!("As shown \\{name}{{k1, k2,\n  k3}}.");
        let found = scan(&text).citations;
        assert_eq!(keys(&found), ["k1", "k2", "k3"], "\\{name} comma list over two lines");
        assert_eq!(found.iter().map(|o| o.line).collect::<Vec<_>>(), [1, 1, 2]);
        assert!(found.iter().all(|f| substring(&text, f.range()) == f.key));
        assert_eq!(keys(&scan(&format!("\\{name}*{{star}}")).citations), ["star"], "\\{name}* starred");
        assert_eq!(keys(&scan(&format!("\\{name}[p.~5]{{one}}")).citations), ["one"], "\\{name} one optional argument");
        assert_eq!(keys(&scan(&format!("\\{name}[see][p.~5]{{a, b}}")).citations), ["a", "b"], "\\{name} two optional arguments");
        assert_eq!(keys(&scan(&format!("\\{name}[\\ref{{x}}]{{c}}")).citations), ["c"], "\\{name} optional argument with a macro");
    }
    for name in ["Citep", "Citet", "citealt", "citeyearpar", "citenum", "Parencite", "Textcite", "Autocite", "Cite", "fullcite", "footfullcite", "citetitle"] {
        assert_eq!(keys(&scan(&format!("\\{name}{{k}}")).citations), ["k"], "\\{name}");
    }
    let first = scan("\\cite[x]{a}");
    assert_eq!(first.citations.iter().map(|c| c.column).collect::<Vec<_>>(), [10]);
    assert_eq!(first.citations.iter().map(|c| c.offset).collect::<Vec<_>>(), [9]);

    assert_eq!(scan("\\nocite{*}").citations.len(), 0, "\\nocite{{*}} is skipped");
    assert_eq!(keys(&scan("\\nocite{a, *, b}").citations), ["a", "b"], "only the star item is skipped");
    assert_eq!(keys(&scan("\\nocite{a}").citations), ["a"]);

    assert_eq!(keys(&scan("\\parencites{a}{b}").citations), ["a"], "multicite reads its first group");
    assert_eq!(scan("\\parencites(pre)(post)[x][y]{a}").citations.len(), 0, "parenthesised notes are not parsed");

    assert_eq!(scan("\\citestyle{authoryear} \\citetext{see x} \\citeindexfalse \\citepunct \\citeleft").citations.len(), 0, "commands that take no key");
    assert_eq!(scan("\\cite").citations.len(), 0);
    assert_eq!(scan("\\cite{}").citations.len(), 0, "empty argument");
}

#[test]
fn scanner_comments_verbatim_macro_bodies() {
    let text = [
        r"% \ref{commented} and \cite{commented}",
        r"Real \ref{real} % trailing \ref{trail}",
        r"\verb|\ref{verb}| and 50\% \ref{afterpercent}",
        r"\begin{verbatim}",
        r"\ref{inverb} \cite{inverb}",
        r"\end{verbatim}",
        r"\iffalse",
        r"\ref{iffalse}",
        r"\fi",
        r"\\ref{doublebackslash}",
        r"\ref{} \ref{ #1 } \ref{sec:#1} \cite{\mykey} \ref{a\b} \ref{x{y}}",
        r"\newcommand{\myref}[1]{\ref{fig:#1}~\cite{#1}}",
        r"\url{http://example.org/\ref{inurl}}",
    ]
    .join("\n");
    let found = scan(&text);
    assert_eq!(keys(&found.references), ["real", "afterpercent"]);
    assert_eq!(keys(&found.citations), Vec::<String>::new());
    let with_labels = scan("% \\label{c}\n\\label{ok} \\verb+\\label{v}+");
    assert_eq!(keys(&with_labels.labels), ["ok"], "labels ignore comments and \\verb");
}

#[test]
fn scanner_utf16_columns() {
    let prefixes = ["\u{00C1}lgebra \u{1D538} ", "A\u{0301}lgebra \u{1F600}\u{1F600} ", "Teoria dos n\u{00FA}meros \u{2014} "];
    for prefix in prefixes {
        let text = format!("{prefix}\\ref{{se\u{00E7}\u{00E3}o:\u{00E1}}} e \\cite{{a\u{00E7}\u{00E3}o,\u{1F600}}}\n");
        let u = units(&text);
        let found = scan(&text);
        let all: Vec<KeyOccurrence> = found.references.iter().chain(found.citations.iter()).cloned().collect();
        assert_eq!(keys(&all), ["se\u{00E7}\u{00E3}o:\u{00E1}", "a\u{00E7}\u{00E3}o", "\u{1F600}"]);
        assert!(all.iter().all(|o| substring(&text, o.range()) == o.key), "ranges are UTF-16 exact");
        assert!(all.iter().all(|o| o.length == o.key.encode_utf16().count()), "lengths are UTF-16 lengths");
        let ref_loc = u.windows(5).position(|w| w == units("\\ref{").as_slice()).unwrap();
        assert_eq!(found.references.first().map(|o| o.column), Some(ref_loc + 5 + 1), "column counts UTF-16 units");
        assert_eq!(found.citations.last().map(|o| o.length), Some(2), "an emoji key is two units long");
        let emoji_loc = u.windows(3).position(|w| w == units("\u{1F600}}").as_slice()).unwrap();
        assert_eq!(found.citations.last().map(|o| o.column), Some(emoji_loc + 1));
    }
    let second = "\u{1D538}\u{1D538} \\label{fig:\u{1F600}}";
    assert_eq!(scan(second).labels.first().map(|l| l.column), Some(4 + 1 + 7 + 1), "two astral letters (4 units), a space, \\label{{");
    assert_eq!(scan(second).labels.first().map(|l| l.length), Some(6));

    let crlf = "a\r\n\\ref{x}\r\n\\ref{y}\rl4 \\ref{z}\n\\ref{w}";
    let found = scan(crlf).references;
    assert_eq!(found.iter().map(|o| o.line).collect::<Vec<_>>(), [2, 3, 4, 5], "CRLF, CR and LF all end lines");
    assert_eq!(found.iter().map(|o| o.column).collect::<Vec<_>>(), [6, 6, 9, 6]);
    assert!(found.iter().all(|o| substring(crlf, o.range()) == o.key));
}

#[test]
fn scanner_labels_option_labels_bibitems_dynamic_xr() {
    let text = [
        r"\label{a} \label[equation]{b} \label {c}",
        r"% \label{commented}",
        r"\label{sec:#1} \label{fig:\thechapter-x} \label{#1}",
        r"\begin{lstlisting}[language=C, label=lst:code]",
        r"int x; \label{verbatim}",
        r"\end{lstlisting}",
        r"\begin{theorem}[name=Main, label = {thm:main}]",
        r"\begin{enumerate}[label=(\alph*)]",
        r"\begin{itemize}[leftmargin=*, labelsep=1em]",
        r"\lstinputlisting[caption={x, y}, label=lst:file]{code.c}",
        r"\newlabel{manual}{{1}{1}}",
        r"\begin{thebibliography}{9}",
        r"\bibitem{knuth} K.",
        r"\bibitem[Lamport 1994]{lamport94} L.",
        r"\end{thebibliography}",
    ]
    .join("\n");
    let found = scan(&text);
    assert_eq!(keys(&found.labels), ["a", "b", "c"]);
    assert_eq!(keys(&found.option_labels), ["lst:code", "thm:main", "lst:file", "manual"]);
    assert_eq!(found.dynamic_label_prefixes, ["", "fig:", "sec:"]);
    assert_eq!(keys(&found.bib_items), ["knuth", "lamport94"]);
    assert!(found.labels.iter().chain(&found.option_labels).chain(&found.bib_items).all(|o| substring(&text, o.range()) == o.key));

    assert!(scan("\\externaldocument[ch2-]{chapter2}").uses_external_documents);
    assert!(scan("\\usepackage{xr-hyper}").uses_external_documents);
    assert!(scan("\\usepackage[opt]{amsmath, xr}").uses_external_documents);
    assert!(scan("\\RequirePackage{xr}").uses_external_documents);
    assert!(!scan("\\usepackage{xcolor,hyperref} \\usepackage{xrcolor}").uses_external_documents);
    assert!(!scan("% \\externaldocument{x}").uses_external_documents, "commented out");
    assert!(scan("\\externalcitedocument{x}").uses_external_citations);
    assert!(!scan("\\ref{a}").uses_external_citations);
}

// ---------------------------------------------------------------------------------------------------
// Index
// ---------------------------------------------------------------------------------------------------

fn base_project() -> RefProject {
    RefProject::new(&[
        (
            "main.tex",
            "\\documentclass{article}\n\\usepackage{hyperref}\n\\begin{document}\n\\section{Intro}\\label{sec:intro}\nSee \\ref{sec:method} and \\cite{knuth84,missing1}.\n\\input{sections/method}\n\\input{sections/results}\n\\bibliographystyle{plain}\n\\bibliography{refs}\n\\end{document}",
        ),
        (
            "sections/method.tex",
            "\\section{Method}\\label{sec:method}\n\\begin{equation}\\label{eq:a}x\\end{equation}\nBy \\eqref{eq:a}, \\cref{sec:intro,sec:results} and \\ref{sec:nowhere}.",
        ),
        ("sections/results.tex", "\\section{Results}\\label{sec:results}\n\\label{eq:a}\nSee \\autoref{sec:method} and \\cite{hatcher2002}."),
        ("refs.bib", "@article{knuth84, title={T}}\n@book{hatcher2002, title={AT}}\n"),
    ])
}

#[test]
fn index_reference_data_across_files() {
    let project = base_project();
    let index = project.index();
    assert_eq!(keys(&index.label_sites), ["sec:intro", "sec:method", "eq:a", "sec:results", "eq:a"], "every \\label, duplicates included, in file order");
    let names: Vec<&str> = index.label_sites.iter().map(|l| l.file.rsplit('/').next().unwrap()).collect();
    assert_eq!(names, ["main.tex", "method.tex", "method.tex", "results.tex", "results.tex"]);
    assert_eq!(index.labels.iter().map(|l| l.key.clone()).collect::<Vec<_>>(), keys(&index.label_sites), "labels and labelSites agree");
    assert_eq!(keys(&index.reference_uses), ["sec:method", "eq:a", "sec:intro", "sec:results", "sec:nowhere", "sec:method"]);
    assert_eq!(keys(&index.citation_uses), ["knuth84", "missing1", "hatcher2002"]);
    assert!(!index.uses_external_documents && !index.uses_external_citations && !index.has_unknown_bibliography_source);
    assert_eq!(index.bib_entries.iter().map(|b| b.key.clone()).collect::<Vec<_>>(), ["knuth84", "hatcher2002"]);

    // Not included from the root: not part of the project.
    let mut with_orphan = project.clone();
    with_orphan.files.insert("orphan.tex".into(), "\\label{orphan}\\ref{orphan2}".into());
    assert_eq!(with_orphan.index().label_sites.len(), 5, "files outside the include closure are not scanned");

    // A missing, unreadable or non-.bib bibliography makes the key set unknown.
    let mut missing = project.clone();
    missing.files.remove("refs.bib");
    assert!(missing.index().has_unknown_bibliography_source, "\\bibliography file not found");
    let main_text = project.files["main.tex"].clone();
    let mut resource = project.clone();
    resource.files.insert("main.tex".into(), main_text.replace("\\bibliography{refs}", "\\addbibresource{refs.json}"));
    resource.files.insert("refs.json".into(), "[]".into());
    assert!(resource.index().has_unknown_bibliography_source, "a non-.bib resource");
    let mut fine = project.clone();
    fine.files.insert("main.tex".into(), main_text.replace("\\bibliography{refs}", "\\addbibresource{refs.bib}"));
    assert!(!fine.index().has_unknown_bibliography_source, "\\addbibresource of an existing .bib");
}

// ---------------------------------------------------------------------------------------------------
// Linter
// ---------------------------------------------------------------------------------------------------

#[test]
fn linter_across_files() {
    let project = RefProject::new(&[
        (
            "main.tex",
            "\\documentclass{article}\n\\begin{document}\n\\section{Intro}\\label{sec:intro}\nSee \\ref{sec:method} and \\cite{knuth84,missing1}.\n\\input{sections/method}\n\\input{sections/results}\n\\bibliography{refs}\n\\end{document}",
        ),
        (
            "sections/method.tex",
            "\\section{Method}\\label{sec:method}\n\\begin{equation}\\label{eq:a}x\\end{equation}\nBy \\eqref{eq:a}, \\cref{sec:intro,sec:results} and \\ref{sec:nowhere}.",
        ),
        ("sections/results.tex", "\\section{Results}\\label{sec:results}\n\\label{eq:a}\nSee \\autoref{sec:method} and \\cite{hatcher2002}."),
        ("refs.bib", "@article{knuth84, title={T}}\n@book{hatcher2002, title={AT}}\n"),
    ]);
    let found = project.lint();
    assert_eq!(
        describe(&found),
        [
            "main.tex:4 Citation \u{2018}missing1\u{2019} is not in the bibliography",
            "sections/method.tex:2 Label \u{2018}eq:a\u{2019} is defined more than once",
            "sections/method.tex:3 Reference \u{2018}sec:nowhere\u{2019} is not defined",
            "sections/results.tex:2 Label \u{2018}eq:a\u{2019} is defined more than once",
        ],
        "ordered by file (document order), then position"
    );
    assert_eq!(
        found.iter().map(|d| project.underlined(d).unwrap_or_default()).collect::<Vec<_>>(),
        ["missing1", "eq:a", "sec:nowhere", "eq:a"],
        "every diagnostic underlines exactly its key"
    );
    assert!(found.iter().all(|d| d.severity == Severity::Warning && d.source == Source::Structure));
    assert!(found.iter().all(|d| d.length == project.underlined(d).map(|u| u.encode_utf16().count() as u32)));
    assert_eq!(found[1].detail.as_deref(), Some("Also defined at sections/results.tex:2"));
    assert_eq!(found[3].detail.as_deref(), Some("Also defined at sections/method.tex:2"));
    assert_eq!(found[0].detail, None);
    assert_eq!(found[2].detail, None);
    assert_eq!(found.iter().map(|d| d.column.unwrap()).collect::<Vec<_>>(), [40, 24, 56, 8]);

    // The index is rebuilt from whatever text is current (unsaved buffers): fixing the text clears the warnings.
    let mut fixed = project.clone();
    fixed.files.insert("sections/method.tex".into(), "\\section{Method}\\label{sec:method}\n\\label{eq:b}\n\\ref{eq:b}".into());
    let main = project.files["main.tex"].replace("missing1", "knuth84");
    fixed.files.insert("main.tex".into(), main);
    assert_eq!(describe(&fixed.lint()), Vec::<String>::new(), "no warnings once everything is defined");
}

fn lint_main(main: &str, extra: &[(&str, &str)]) -> Vec<String> {
    let mut files: Vec<(&str, &str)> = vec![("main.tex", main)];
    files.extend_from_slice(extra);
    describe(&RefProject::new(&files).lint())
}

fn undefined(key: &str, line: usize) -> String {
    format!("main.tex:{line} Reference \u{2018}{key}\u{2019} is not defined")
}

#[test]
fn linter_undefined_references() {
    assert_eq!(lint_main("\\label{a}\\ref{a}\\eqref{a}\\cref{a}", &[]), Vec::<String>::new());
    assert_eq!(lint_main("\\ref{a}\n\\ref{a}", &[]), [undefined("a", 1), undefined("a", 2)], "every use is reported");
    assert_eq!(lint_main("\\cref{x,y}\\label{y}", &[]), [undefined("x", 1)], "a list reports its undefined keys");
    assert_eq!(lint_main("\\crefrange{x}{y}\\label{y}", &[]), [undefined("x", 1)]);
    assert_eq!(lint_main("\\label{A}\\ref{a}", &[]), [undefined("a", 1)], "labels are case-sensitive");
    assert_eq!(lint_main("% \\label{a}\n\\ref{a}", &[]), [undefined("a", 2)], "a commented-out label defines nothing");
    assert_eq!(lint_main("% \\ref{a}\n\\begin{verbatim}\\ref{b}\\end{verbatim}", &[]), Vec::<String>::new(), "commented and verbatim uses are ignored");
    assert_eq!(lint_main("\\input{o}\\ref{inc}", &[("o.tex", "\\label{inc}")]), Vec::<String>::new(), "labels in included files");
    assert_eq!(lint_main("\\ref{inc}", &[("o.tex", "\\label{inc}")]), [undefined("inc", 1)], "labels in files the root does not include");
    assert_eq!(
        lint_main("\\ref{inc}\\input{o}", &[("o.tex", "\\ref{back}\\label{inc}")]),
        ["o.tex:1 Reference \u{2018}back\u{2019} is not defined"],
        "forward references are fine"
    );

    // xr: skip the check entirely.
    assert_eq!(lint_main("\\externaldocument{other}\\ref{a}", &[]), Vec::<String>::new(), "\\externaldocument");
    assert_eq!(lint_main("\\usepackage{xr-hyper}\\ref{a}", &[]), Vec::<String>::new(), "xr-hyper");
    assert_eq!(lint_main("\\usepackage{xr}\\ref{a}", &[]), Vec::<String>::new(), "xr");
    assert_eq!(lint_main("\\input{o}\\ref{a}", &[("o.tex", "\\externaldocument[b-]{book}")]), Vec::<String>::new(), "xr declared in an included file");
    assert_eq!(lint_main("\\externaldocument{other}\\label{d}\\label{d}", &[]).len(), 2, "duplicate labels are still reported with xr");

    // Labels that macros or options define.
    assert_eq!(
        lint_main("\\newcommand{\\sectionlabel}[1]{\\label{sec:#1}}\\ref{sec:intro}\\ref{fig:x}", &[]),
        [undefined("fig:x", 1)],
        "a label built from a macro argument keeps its prefix"
    );
    assert_eq!(lint_main("\\newcommand{\\mylabel}[1]{\\label{#1}}\\ref{anything}", &[]), Vec::<String>::new(), "a fully dynamic label disables the check");
    assert_eq!(lint_main("\\newcommand{\\mylabel}{\\label{fig:\\thefigure}}\\ref{fig:7}\\ref{tab:7}", &[]), [undefined("tab:7", 1)]);
    assert_eq!(
        lint_main("\\begin{lstlisting}[label=lst:a]\nx\n\\end{lstlisting}\\ref{lst:a}\\lstinputlisting[label={lst:b}]{c.py}\\ref{lst:b}", &[]),
        Vec::<String>::new()
    );
    assert_eq!(lint_main("\\begin{theorem}[name=X, label=thm:x]\\end{theorem}\\ref{thm:x}\\newlabel{n}{{1}{1}}\\ref{n}", &[]), Vec::<String>::new());
    assert_eq!(lint_main("\\begin{enumerate}[label=(\\alph*)]\\end{enumerate}\\ref{(a)}", &[]).len(), 1, "an enumitem format is not a label");
    assert_eq!(lint_main("\\hyperref[sec:x]{text}\\hyperref{url}{c}{n}{text}", &[]), [undefined("sec:x", 1)]);
    assert_eq!(lint_main("\\pageref{LastPage} \\pageref*{TotPages} \\pageref{Other}", &[]), [undefined("Other", 1)], "lastpage / totpages");
}

fn lint_files(files: &[(&str, &str)]) -> Vec<LogDiagnostic> {
    RefProject::new(files).lint()
}

#[test]
fn linter_duplicate_labels() {
    let twice = lint_files(&[("main.tex", "\\label{d}\n\\label{e}\n  \\label{ d }\n")]);
    assert_eq!(
        describe(&twice),
        ["main.tex:1 Label \u{2018}d\u{2019} is defined more than once", "main.tex:3 Label \u{2018}d\u{2019} is defined more than once"],
        "a warning at every definition; keys are trimmed"
    );
    assert_eq!(twice.iter().map(|d| d.detail.clone().unwrap()).collect::<Vec<_>>(), ["Also defined at main.tex:3", "Also defined at main.tex:1"]);
    assert_eq!(twice.iter().map(|d| d.column.unwrap()).collect::<Vec<_>>(), [8, 11]);
    assert_eq!(twice.iter().map(|d| d.length.unwrap()).collect::<Vec<_>>(), [1, 1]);

    let across = lint_files(&[("main.tex", "\\input{a}\\input{b/c}\\label{x}"), ("a.tex", "\\label{x}"), ("b/c.tex", "\\label{x}")]);
    assert_eq!(across.len(), 3);
    assert_eq!(
        across.iter().map(|d| d.detail.clone().unwrap()).collect::<Vec<_>>(),
        ["Also defined at a.tex:1, b/c.tex:1", "Also defined at main.tex:1, b/c.tex:1", "Also defined at main.tex:1, a.tex:1"],
        "the other places, in document order, paths relative to the root file"
    );
    assert_eq!(across.iter().map(|d| d.path.as_deref().unwrap().rsplit('/').next().unwrap().to_string()).collect::<Vec<_>>(), ["main.tex", "a.tex", "c.tex"]);

    let many_text = (1..=8).map(|_| "\\label{d}").collect::<Vec<_>>().join("\n");
    let many = lint_files(&[("main.tex", &many_text)]);
    assert_eq!(many.len(), 8);
    assert_eq!(many[0].detail.as_deref(), Some("Also defined at main.tex:2, main.tex:3, main.tex:4, main.tex:5, main.tex:6 and 2 more"));
    assert_eq!(many[7].detail.as_deref(), Some("Also defined at main.tex:1, main.tex:2, main.tex:3, main.tex:4, main.tex:5 and 2 more"));

    assert_eq!(lint_files(&[("main.tex", "\\label{a}\\label{A}\\label{a }")]).len(), 2, "case-sensitive; trimmed");
    assert_eq!(lint_files(&[("main.tex", "\\label{a}\\begin{lstlisting}[label=a]\n\\end{lstlisting}")]).len(), 0, "an option label never makes a duplicate");
    assert_eq!(lint_files(&[("main.tex", "\\label{x}\n% \\label{x}\n\\begin{verbatim}\\label{x}\\end{verbatim}")]).len(), 0);

    // Alternatives of one conditional are not duplicates.
    let count = |text: &str| lint_files(&[("main.tex", text)]).len();
    assert_eq!(count("\\newif\\ifenglish\\englishtrue\n\\ifenglish\\label{intro}\\else\\label{intro}\\fi\n"), 0, "the two branches of \\if…\\else…\\fi");
    assert_eq!(count("\\label{a}\\iftrue\\label{a}\\fi"), 0, "one always-defined and one conditional definition");
    assert_eq!(count("\\label{a}\\ifx\\foo\\bar\\label{a}\\fi\\label{a}"), 3, "two unconditional definitions: all are reported");
    assert_eq!(count("\\iftrue x\\fi\\label{a}\\label{a}"), 2, "after the \\fi");
    assert_eq!(count("\\ifdefined\\foo\\label{a}\\fi\\label{a}"), 0, "\\ifdefined opens a conditional");
    assert_eq!(count("$a \\iff b$ \\label{a}\\label{a}"), 2, "\\iff is not a conditional");
    assert_eq!(count("\\ifthenelse{\\equal{a}{b}}{x}{y}\\label{a}\\label{a}"), 2, "\\ifthenelse is not closed by \\fi");
    assert_eq!(count("\\ifdraft{x}{y}\\iftoggle{t}{a}{b}\\label{a}\\label{a}"), 2, "braced conditionals");
    assert_eq!(count("\\newif\\iffoo \\label{a}\\label{a}"), 2, "\\newif declares, it does not open");
    assert_eq!(count("\\fi\\fi\\label{a}\\label{a}"), 2, "stray \\fi");
    assert_eq!(count("\\if aa\\label{a}\\fi\\label{a}"), 0, "\\if opens a conditional");
    let conditional = scan("\\ifx\\a\\b\\ref{in}\\fi\\ref{out}").references;
    assert_eq!(conditional.iter().map(|o| o.is_conditional).collect::<Vec<_>>(), [true, false]);
}

#[test]
fn linter_citations() {
    let bib: [(&str, &str); 1] = [("refs.bib", "@article{knuth84, title={T}}\n@book{Hatcher2002, title={AT}}\n@comment{ignored,}")];
    fn with<'a>(bib: &[(&'a str, &'a str)], extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut v: Vec<(&str, &str)> = bib.to_vec();
        v.extend_from_slice(extra);
        v
    }
    let missing = |key: &str, line: usize| format!("main.tex:{line} Citation \u{2018}{key}\u{2019} is not in the bibliography");

    assert_eq!(lint_main("\\bibliography{refs}\\cite{knuth84}\\citep[p.~2]{Hatcher2002, knuth84}", &bib), Vec::<String>::new());
    assert_eq!(lint_main("\\bibliography{refs}\\cite{nope}", &bib), [missing("nope", 1)]);
    assert_eq!(lint_main("\\bibliography{refs}\\cite{knuth84,nope,other}", &bib), [missing("nope", 1), missing("other", 1)]);
    assert_eq!(lint_main("\\bibliography{refs}\\textcite{KNUTH84}\\cite{hatcher2002}", &bib), Vec::<String>::new(), "BibTeX compares keys case-insensitively");
    assert_eq!(lint_main("\\bibliography{refs}\\cite{ignored}", &bib), [missing("ignored", 1)], "@comment is not an entry");
    assert_eq!(lint_main("\\addbibresource{refs.bib}\\parencite{nope}\\autocite{knuth84}", &bib), [missing("nope", 1)]);
    assert_eq!(lint_main("\\bibliography{a,b}\\cite{x,y,z}", &[("a.bib", "@misc{x,}"), ("b.bib", "@misc{y,}")]), [missing("z", 1)], "several .bib files");
    assert_eq!(
        lint_main("\\input{ch}\\bibliography{refs}", &with(&bib, &[("ch.tex", "\\cite{inchapter}\\cite{knuth84}")])),
        ["ch.tex:1 Citation \u{2018}inchapter\u{2019} is not in the bibliography"]
    );
    assert_eq!(lint_main("\\bibliography{refs}\\nocite{*}\\nocite{nope}", &bib), [missing("nope", 1)], "\\nocite{{*}} is skipped");
    assert_eq!(lint_main("\\bibliography{refs}\\cite{a}\n\\cite{a}", &bib), [missing("a", 1), missing("a", 2)], "every use is reported");

    // \bibitem
    let items = "\\begin{thebibliography}{9}\\bibitem{knuth}K.\\bibitem[L 94]{lamport}L.\\end{thebibliography}";
    assert_eq!(lint_main(&format!("{items}\\cite{{knuth,lamport}}"), &[]), Vec::<String>::new());
    assert_eq!(lint_main(&format!("{items}\\cite{{knuth,other}}"), &[]), [missing("other", 1)]);
    assert_eq!(lint_main(&format!("{items}\\cite{{Knuth}}"), &[]), [missing("Knuth", 1)], "\\bibitem keys match exactly");
    assert_eq!(lint_main(&format!("{items}\\bibliography{{refs}}\\cite{{knuth,knuth84,nope}}"), &bib), [missing("nope", 1)], "bib entries and bibitems together");

    // No bibliography source at all: say nothing.
    assert_eq!(lint_main("\\cite{a}\\nocite{b}", &[]), Vec::<String>::new(), "no bibliography yet");
    assert_eq!(lint_main("\\bibliographystyle{plain}\\cite{a}", &[]), Vec::<String>::new(), "a style alone is not a bibliography");
    // The keys cannot be known: say nothing.
    assert_eq!(lint_main("\\bibliography{refs}\\cite{a}", &[]), Vec::<String>::new(), "\\bibliography file not found");
    assert_eq!(lint_main("\\bibliography{refs,more}\\cite{a}", &bib), Vec::<String>::new(), "one of the files not found");
    assert_eq!(lint_main("\\addbibresource{refs.json}\\cite{a}", &[("refs.json", "[]")]), Vec::<String>::new(), "not a .bib resource");
    assert_eq!(lint_main("\\bibliography{refs}\\externalcitedocument{other}\\cite{a}", &bib), Vec::<String>::new(), "xcite");
    // An empty bibliography is still a bibliography.
    assert_eq!(lint_main("\\bibliography{refs}\\cite{a}", &[("refs.bib", "% nothing yet\n")]), [missing("a", 1)]);
    assert_eq!(lint_main("% \\bibliography{refs}\n\\cite{a}", &[]), Vec::<String>::new(), "a commented-out bibliography is no bibliography");
    assert_eq!(lint_main("\\bibliography{refs}\n% \\cite{a}\n\\begin{verbatim}\\cite{b}\\end{verbatim}", &bib), Vec::<String>::new());
}

#[test]
fn linter_diagnostics_are_plain_values() {
    let project = RefProject::new(&[("main.tex", "\\ref{a}\\ref{b}")]);
    let a = project.lint();
    let b = project.lint();
    assert_eq!(a.len(), 2);
    assert_eq!(a.iter().map(|d| d.message.clone()).collect::<Vec<_>>(), b.iter().map(|d| d.message.clone()).collect::<Vec<_>>());
    assert_eq!(a.iter().map(|d| d.column).collect::<Vec<_>>(), b.iter().map(|d| d.column).collect::<Vec<_>>(), "deterministic");
    assert_eq!(refs::diagnostics(&ProjectModel::new(&url("main.tex"))).len(), 0, "an empty index");
}

#[test]
fn linter_findings_and_tex_warnings() {
    let project = RefProject::new(&[("main.tex", "\\label{d}\\label{d}\\ref{u}\\bibliography{r}\\cite{c}"), ("r.bib", "@misc{z,}")]);
    let index = project.index();
    let findings = refs::findings(&index);
    assert_eq!(findings.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(), ["d", "d", "u", "c"]);
    assert_eq!(
        findings.iter().map(|f| f.kind).collect::<Vec<_>>(),
        [FindingKind::DuplicateLabel, FindingKind::DuplicateLabel, FindingKind::UndefinedReference, FindingKind::MissingCitation]
    );
    assert_eq!(
        findings.iter().map(|f| f.diagnostic.message.clone()).collect::<Vec<_>>(),
        refs::diagnostics(&index).iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );

    let tex = |message: &str| -> String {
        match refs::tex_warning(message) {
            Some((kind, key)) => format!("{kind:?}:{key}"),
            None => "-".to_string(),
        }
    };
    assert_eq!(tex("Reference `sec:a' on page 3 undefined"), "UndefinedReference:sec:a");
    assert_eq!(tex("Reference `sec:a' on page 3 undefined."), "UndefinedReference:sec:a", "trailing period");
    assert_eq!(tex("Citation 'knuth84' on page 1 undefined"), "MissingCitation:knuth84", "plain apostrophes");
    assert_eq!(tex("Citation \u{2018}x\u{2019} on page 12 undefined."), "MissingCitation:x", "curly quotes");
    assert_eq!(tex("Label `eq:a' multiply defined."), "DuplicateLabel:eq:a");
    assert_eq!(tex("Reference `my key' on page 1 undefined"), "UndefinedReference:my key");
    assert_eq!(tex("Reference `it's' on page 1 undefined"), "UndefinedReference:it's", "a quote inside the key");
    for other in [
        "There were undefined references.",
        "Reference `a' on page 1 defined",
        "Label(s) may have changed. Rerun to get cross-references right.",
        "Reference on page 1 undefined",
        "Reference `' on page 1 undefined",
        "Overfull \\hbox (3.0pt too wide)",
        "Citation `a' on page 1 undefined on input line 5",
        "hyperref: Token not allowed",
        "",
    ] {
        assert_eq!(tex(other), "-", "not a reference warning: {other}");
    }
}

#[test]
fn linter_200_file_project_performance() {
    let scale = if cfg!(debug_assertions) { 40.0 } else { 1.0 };
    let mut files: HashMap<String, String> = HashMap::new();
    let mut main = String::from("\\documentclass{article}\n\\begin{document}\n");
    for f in 0..200 {
        main.push_str(&format!("\\input{{s{f}}}\n"));
        let mut body = format!("\\section{{Section {f}}}\\label{{sec:{f}}}\\label{{dup}}\n");
        body.push_str(&format!("Ghost \\ref{{missing{f}}} and \\cite{{ghost{f}}}.\n"));
        for k in 0..10 {
            body.push_str(&format!("Text \\ref{{sec:{}}} \\cite{{key{}}} \\eqref{{eq:{f}-{k}}}.\n", (f + k) % 200, (f * 7 + k) % 400));
            body.push_str(&format!("\\begin{{equation}}\\label{{eq:{f}-{k}}} x \\end{{equation}}\n"));
        }
        files.insert(format!("s{f}.tex"), body);
    }
    files.insert("refs.bib".into(), (0..400).map(|i| format!("@article{{key{i}, title={{Title {i}}}}}")).collect::<Vec<_>>().join("\n"));
    files.insert("main.tex".into(), format!("{main}\\bibliography{{refs}}\n\\end{{document}}\n"));
    let project = RefProject { files };

    let start = std::time::Instant::now();
    let index = project.index();
    let build_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(index.files.len(), 201);
    assert_eq!(index.label_sites.len(), 200 * 12);
    assert_eq!(index.reference_uses.len(), 200 * 21);
    assert_eq!(index.citation_uses.len(), 200 * 11);

    let mut times: Vec<f64> = Vec::new();
    let mut count = 0;
    for run in 0..7 {
        let start = std::time::Instant::now();
        let found = refs::diagnostics(&index);
        if run >= 2 {
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        count = found.len();
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = times[times.len() / 2];
    // 200 duplicate "dup" labels + 200 undefined references + 200 missing citations.
    assert_eq!(count, 600);
    eprintln!("TIMING reference lint (200 files, {} refs, {} cites): {median:.2} ms; index build {build_ms:.1} ms", index.reference_uses.len(), index.citation_uses.len());
    assert!(median < 30.0 * scale, "lint took {median} ms (bound {} ms)", 30.0 * scale);
}

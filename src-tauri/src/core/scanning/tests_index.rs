//! Port of `ProjectIndexChecks.swift`.

use crate::core::project_index::*;
use crate::core::scanner::models::*;
use crate::core::scanner::tests_common::url;
use std::collections::{HashMap, HashSet};

/// In-memory project: relative path -> contents.
struct FakeProject {
    files: HashMap<String, String>,
}

impl FakeProject {
    fn new(files: &[(&str, &str)]) -> Self {
        FakeProject { files: files.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }
    fn url(&self, path: &str) -> String {
        url(path)
    }
    fn read(&self, path: &str) -> Option<String> {
        path.strip_prefix("/proj/").and_then(|p| self.files.get(p)).cloned()
    }
    fn exists(&self, path: &str) -> bool {
        self.read(path).is_some()
    }
    fn build(&self, root: &str) -> ProjectModel {
        build(&self.url(root), &|p| self.read(p), &|p| self.exists(p))
    }
}

fn describe_outline(nodes: &[OutlineNode], indent: usize, out: &mut Vec<String>) {
    for node in nodes {
        out.push(format!("{}{}:{}", "  ".repeat(indent), node.entry.kind.raw(), node.entry.title));
        describe_outline(node.children.as_deref().unwrap_or(&[]), indent + 1, out);
    }
}

fn outline_lines(index: &ProjectModel) -> Vec<String> {
    let mut out = Vec::new();
    describe_outline(&index.outline, 0, &mut out);
    out
}

fn paths(v: &[FilePath]) -> Vec<String> {
    v.iter().map(|p| p.to_string()).collect()
}

#[test]
fn resolve_rules() {
    let existing: HashSet<&str> = [
        "/proj/sections/deep.tex", "/proj/x", "/proj/x.tex", "/proj/y", "/proj/refs.bib", "/proj/fig/plot.png", "/proj/fig/plot.pdf",
        "/proj/fig/only.png", "/proj/fig/exact.jpg", "/proj/mystyle.sty", "/proj/myclass.cls", "/abs/file.tex", "/proj/chapters/c1a.tex",
        "/proj/chapters/sub/part.tex", "/proj/imp/x.tex", "/abs/d/x.tex", "/proj/shared/x.tex", "/proj/code.c", "/proj/script.py",
        "/proj/sections/intro.tex", "/proj/main.tex", "/proj/plain/refs2.bib", "/proj/chapters/local.tex",
    ]
    .into_iter()
    .collect();
    let exists = |p: &str| existing.contains(p);
    let root = url("main.tex");
    let intro = url("sections/intro.tex");
    let c1 = url("chapters/c1.tex");

    let res = |kind: IncludeKind, argument: &str, directory: Option<&str>, from: Option<&str>| -> Option<String> {
        let reference = IncludeReference { kind, argument: argument.to_string(), directory: directory.map(String::from), line: 1, range: 0..0 };
        resolve(&reference, from.unwrap_or(&intro), &root, &exists)
    };
    let r = |kind, argument| res(kind, argument, None, None);
    use IncludeKind::*;

    assert_eq!(r(Input, "sections/deep").as_deref(), Some("/proj/sections/deep.tex"), "\\input appends .tex");
    assert_eq!(r(Input, "deep"), None, "\\input is relative to the ROOT directory, not the including file");
    assert_eq!(r(Input, "sections/deep.tex").as_deref(), Some("/proj/sections/deep.tex"));
    assert_eq!(r(Input, "./sections/deep").as_deref(), Some("/proj/sections/deep.tex"), "leading ./");
    assert_eq!(r(Input, "  sections/deep \n").as_deref(), Some("/proj/sections/deep.tex"), "argument whitespace");
    assert_eq!(r(Input, "x").as_deref(), Some("/proj/x"), "extensionless file wins over x.tex for \\input");
    assert_eq!(r(Input, "/abs/file").as_deref(), Some("/abs/file.tex"), "absolute path");
    assert_eq!(r(Input, "nothing"), None);
    assert_eq!(r(Input, ""), None);
    assert_eq!(r(Include, "sections/deep").as_deref(), Some("/proj/sections/deep.tex"));
    assert_eq!(r(Include, "y"), None, "\\include always appends .tex");
    assert_eq!(r(Include, "x").as_deref(), Some("/proj/x.tex"));
    assert_eq!(r(Bibliography, "refs").as_deref(), Some("/proj/refs.bib"));
    assert_eq!(r(Bibliography, "refs.bib").as_deref(), Some("/proj/refs.bib"));
    assert_eq!(r(Bibliography, "plain/refs2").as_deref(), Some("/proj/plain/refs2.bib"));
    assert_eq!(r(Addbibresource, "refs.bib").as_deref(), Some("/proj/refs.bib"));
    assert_eq!(r(Addbibresource, "refs"), None, "\\addbibresource keeps the name as written");
    assert_eq!(r(Includegraphics, "fig/plot").as_deref(), Some("/proj/fig/plot.pdf"), "pdf is tried before png");
    assert_eq!(r(Includegraphics, "fig/only").as_deref(), Some("/proj/fig/only.png"));
    assert_eq!(r(Includegraphics, "fig/exact.jpg").as_deref(), Some("/proj/fig/exact.jpg"));
    assert_eq!(r(Includegraphics, "fig/plot.png").as_deref(), Some("/proj/fig/plot.png"));
    assert_eq!(r(Includegraphics, "fig/none"), None);
    assert_eq!(r(Includepdf, "fig/plot").as_deref(), Some("/proj/fig/plot.pdf"));
    assert_eq!(r(Usepackage, "mystyle").as_deref(), Some("/proj/mystyle.sty"));
    assert_eq!(r(Usepackage, "amsmath"), None);
    assert_eq!(r(Documentclass, "myclass").as_deref(), Some("/proj/myclass.cls"));
    assert_eq!(r(Lstinputlisting, "code.c").as_deref(), Some("/proj/code.c"));
    assert_eq!(r(Inputminted, "script.py").as_deref(), Some("/proj/script.py"));
    // Kinds relative to the including file's directory.
    assert_eq!(res(Subfile, "c1a", None, Some(&c1)).as_deref(), Some("/proj/chapters/c1a.tex"), "\\subfile is relative to the including file");
    assert_eq!(res(Subfile, "c1a", None, Some(&root)), None);
    assert_eq!(res(Subimport, "part", Some("sub/"), Some(&c1)).as_deref(), Some("/proj/chapters/sub/part.tex"));
    assert_eq!(res(Subimport, "x", Some("../shared/"), Some(&c1)).as_deref(), Some("/proj/shared/x.tex"), "directory with ..");
    assert_eq!(res(ImportFile, "x", Some("imp/"), Some(&c1)).as_deref(), Some("/proj/imp/x.tex"), "\\import dir is relative to the root");
    assert_eq!(res(ImportFile, "x", Some("/abs/d/"), Some(&c1)).as_deref(), Some("/abs/d/x.tex"), "absolute \\import dir");
    assert_eq!(res(Input, "local", None, Some(&c1)), None, "\\input in a subdirectory still uses the root directory");
}

#[test]
fn build_outline_nesting_aggregation_cycles() {
    let project = FakeProject::new(&[
        (
            "main.tex",
            "\\documentclass{article}\n\\usepackage{amsmath}\n\\newcommand{\\R}{\\mathbb{R}}\n\\begin{document}\n\\section{Intro}\n\\label{sec:intro}\n\\input{sections/intro}\n\\section{Methods}\n\\input{sections/methods}\n\\include{sections/results}\n\\bibliography{refs}\n\\end{document}\n% !TEX program = lualatex",
        ),
        ("sections/intro.tex", "\\subsection{Background}\n\\label{sub:bg}\n\\input{sections/deep}\n\\subsection{Goals}"),
        ("sections/deep.tex", "\\subsubsection{Deep}\n\\label{deep:1}\n\\input{sections/intro}"),
        (
            "sections/methods.tex",
            "\\subsection{Setup}\n\\label{eq:setup}\n\\newcommand{\\Z}{\\mathbb{Z}}\n\\addbibresource{extra.bib}\n\\input{sections/missing}\n\\input{data.csv}\n\\includegraphics{figs/plot}",
        ),
        ("sections/results.tex", "\\section{Results}\n\\newtheorem{thm}{Theorem}\n\\begin{thm}\\label{thm:main} x \\end{thm}"),
        ("refs.bib", "@article{knuth1984, author={Knuth, Donald}, title={Literate Programming}, year=1984}"),
        ("extra.bib", "@book{hatcher2002, author = {Hatcher, Allen}, title = {Algebraic Topology}, year = {2002}}"),
        ("data.csv", "a,b"),
    ]);
    let root = project.url("main.tex");
    let index = project.build("main.tex");

    assert_eq!(&*index.root_file, root.as_str());
    assert_eq!(
        paths(&index.files),
        ["/proj/main.tex", "/proj/sections/intro.tex", "/proj/sections/deep.tex", "/proj/sections/methods.tex", "/proj/sections/results.tex"],
        "depth-first document order, deduplicated, cycle-safe"
    );
    let g = |p: &str| index.include_graph.get(p).map(|v| paths(v));
    assert_eq!(g(&root), Some(vec!["/proj/sections/intro.tex".to_string(), "/proj/sections/methods.tex".into(), "/proj/sections/results.tex".into()]));
    assert_eq!(g("/proj/sections/intro.tex"), Some(vec!["/proj/sections/deep.tex".to_string()]));
    assert_eq!(g("/proj/sections/deep.tex"), Some(vec!["/proj/sections/intro.tex".to_string()]), "cycle edge is recorded");
    assert_eq!(g("/proj/sections/results.tex"), None);

    assert_eq!(
        outline_lines(&index),
        [
            "section:Intro",
            "  label:sec:intro",
            "  subsection:Background",
            "    label:sub:bg",
            "    subsubsection:Deep",
            "      label:deep:1",
            "  subsection:Goals",
            "section:Methods",
            "  subsection:Setup",
            "    label:eq:setup",
            "section:Results",
            "  label:thm:main",
        ]
    );
    // Entries keep their own file and line (1-based).
    let intro = index.outline.first().unwrap();
    assert_eq!(&*intro.entry.file, root.as_str());
    assert_eq!(intro.entry.line, 5);
    let background = intro.children.as_ref().unwrap().iter().find(|n| n.entry.title == "Background").unwrap();
    assert_eq!(&*background.entry.file, "/proj/sections/intro.tex");
    assert_eq!(background.entry.line, 1);
    let deep = background.children.as_ref().unwrap().iter().find(|n| n.entry.kind == OutlineKind::Subsubsection).unwrap();
    assert_eq!(&*deep.entry.file, "/proj/sections/deep.tex");
    fn leaves_have_none(nodes: &[OutlineNode]) -> bool {
        nodes.iter().all(|n| if n.entry.kind == OutlineKind::Label { n.children.is_none() } else { n.children.as_deref().map(leaves_have_none).unwrap_or(true) })
    }
    assert!(leaves_have_none(&index.outline), "labels are leaves (children == None)");
    let mut ids = HashSet::new();
    fn collect(nodes: &[OutlineNode], ids: &mut HashSet<String>) {
        for n in nodes {
            ids.insert(n.id());
            collect(n.children.as_deref().unwrap_or(&[]), ids);
        }
    }
    collect(&index.outline, &mut ids);
    assert_eq!(ids.len(), 12, "node ids are unique");

    assert_eq!(index.labels.iter().map(|l| l.key.as_str()).collect::<Vec<_>>(), ["sec:intro", "sub:bg", "deep:1", "eq:setup", "thm:main"]);
    assert_eq!(index.labels.iter().find(|l| l.key == "deep:1").and_then(|l| l.context.clone()).as_deref(), Some("Deep"));
    assert_eq!(index.labels.iter().find(|l| l.key == "thm:main").and_then(|l| l.context.clone()).as_deref(), Some("thm"));
    assert_eq!(paths(&index.bib_files), ["/proj/extra.bib", "/proj/refs.bib"], "bib files in document order");
    assert_eq!(index.bib_entries.iter().map(|b| b.key.as_str()).collect::<Vec<_>>(), ["hatcher2002", "knuth1984"]);
    assert_eq!(&*index.bib_entries[0].file, "/proj/extra.bib");
    assert_eq!(index.custom_commands.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["\\R", "\\Z"]);
    assert_eq!(index.custom_environments, ["thm"]);
    assert_eq!(index.magic_comments.program.as_deref(), Some("lualatex"));

    let symbols = index.symbols(vec!["a.tex".into()]);
    assert_eq!(symbols.labels.len(), 5);
    assert_eq!(symbols.bib_entries.len(), 2);
    assert_eq!(symbols.project_files, ["a.tex"]);
    assert_eq!(symbols.custom_environments, ["thm"]);
}

#[test]
fn build_subfile_import_edge_cases() {
    let project = FakeProject::new(&[
        ("main.tex", "\\documentclass{article}\n\\begin{document}\n\\subfile{chapters/c1}\n\\subimport{sub/}{part}\n\\import{imp/}{x}\n\\end{document}"),
        ("chapters/c1.tex", "\\documentclass[../main.tex]{subfiles}\n\\begin{document}\n\\section{C1}\n\\subfile{c1a}\n\\end{document}"),
        ("chapters/c1a.tex", "\\subsection{C1A}\n"),
        ("sub/part.tex", "\\section{Part from sub}\n"),
        ("imp/x.tex", "\\section{Imported}\n"),
    ]);
    let index = project.build("main.tex");
    assert_eq!(
        paths(&index.files),
        ["/proj/main.tex", "/proj/chapters/c1.tex", "/proj/chapters/c1a.tex", "/proj/sub/part.tex", "/proj/imp/x.tex"]
    );
    assert_eq!(outline_lines(&index), ["section:C1", "  subsection:C1A", "section:Part from sub", "section:Imported"]);

    // Root that cannot be read.
    let empty = build(&url("missing.tex"), &|_| None, &|_| false);
    assert_eq!(paths(&empty.files), ["/proj/missing.tex"]);
    assert_eq!(empty.outline.len(), 0);

    // Self-include and mutual cycle terminate.
    let cyc = FakeProject::new(&[("a.tex", "\\section{A}\\input{a}\\input{b}"), ("b.tex", "\\section{B}\\input{a}")]);
    let ci = cyc.build("a.tex");
    assert_eq!(paths(&ci.files), ["/proj/a.tex", "/proj/b.tex"]);
    assert_eq!(outline_lines(&ci), ["section:A", "section:B"]);

    // Labels before the first heading are top-level; headings out of order nest by level.
    let flat = FakeProject::new(&[("main.tex", "\\label{first}\n\\subsection{Deep first}\n\\section{Then up}\n\\label{l2}\n\\subsubsection{Skip a level}\n")]);
    assert_eq!(
        outline_lines(&flat.build("main.tex")),
        ["label:first", "subsection:Deep first", "section:Then up", "  label:l2", "  subsubsection:Skip a level"]
    );

    // Entries after the last include and between two includes keep their position.
    let mix = FakeProject::new(&[
        ("main.tex", "\\section{One}\n\\input{p}\n\\section{Two}\n\\input{q}\n\\section{Three}\n"),
        ("p.tex", "\\subsection{P}\n"),
        ("q.tex", "\\subsection{Q}\n"),
    ]);
    assert_eq!(outline_lines(&mix.build("main.tex")), ["section:One", "  subsection:P", "section:Two", "  subsection:Q", "section:Three"]);

    // An include on the same line as a heading, after it.
    let same = FakeProject::new(&[("main.tex", "\\section{A} \\input{p} \\section{B}\n"), ("p.tex", "\\subsection{P}\n")]);
    assert_eq!(outline_lines(&same.build("main.tex")), ["section:A", "  subsection:P", "section:B"]);

    // A commented-out include is ignored.
    let commented = FakeProject::new(&[("main.tex", "% \\input{p}\n\\section{A}\n"), ("p.tex", "\\section{P}\n")]);
    assert_eq!(commented.build("main.tex").files.len(), 1);

    // Local .sty definitions are picked up.
    let sty = FakeProject::new(&[("main.tex", "\\usepackage{local}\n"), ("local.sty", "\\newcommand{\\fromsty}[1]{#1}\n\\newtheorem{styenv}{S}")]);
    let si = sty.build("main.tex");
    assert_eq!(si.custom_commands.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["\\fromsty"]);
    assert_eq!(si.custom_environments, ["styenv"]);

    // Same bib referenced twice is listed once.
    let bib = FakeProject::new(&[("main.tex", "\\bibliography{r}\\addbibresource{r.bib}\n"), ("r.bib", "@misc{k, title={T}}")]);
    let bi = bib.build("main.tex");
    assert_eq!(bi.bib_files.len(), 1);
    assert_eq!(bi.bib_entries.iter().map(|b| b.key.as_str()).collect::<Vec<_>>(), ["k"]);
}

#[test]
fn find_root_rules() {
    let project = FakeProject::new(&[
        ("main.tex", "\\documentclass{article}\n\\begin{document}\n\\input{sections/intro}\n\\input{shared}\n\\input{sections/badmagic}\n\\end{document}\n"),
        ("other.tex", "\\documentclass{article}\n\\begin{document}x\\end{document}\n"),
        ("a.tex", "\\documentclass{article}\n\\begin{document}\\input{shared}\\end{document}\n"),
        ("shared.tex", "\\section{Shared}\n"),
        ("sections/intro.tex", "\\section{Intro}\n\\input{sections/deep}\n"),
        ("sections/deep.tex", "\\subsection{Deep}\n"),
        ("sections/magic.tex", "% !TEX root = ../main.tex\n\\section{M}\n"),
        ("sections/abs.tex", "% !TEX root = /proj/other.tex\n\\section{M}\n"),
        ("sections/badmagic.tex", "% !TEX root = ../nonexistent.tex\n\\section{M}\n"),
        ("sections/self.tex", "\\documentclass{article}\n\\begin{document}x\\end{document}\n"),
        ("sub/s.tex", "\\documentclass[../main.tex]{subfiles}\n\\begin{document}\\section{S}\\end{document}\n"),
        ("lonely.tex", "\\section{Lonely}\n"),
    ]);
    let candidates: Vec<String> = ["other.tex", "a.tex", "main.tex", "sections/self.tex", "sub/s.tex"].iter().map(|p| project.url(p)).collect();
    let root_of = |path: &str, cands: &[String]| -> Option<String> {
        find_root(&project.url(path), cands, &|p| project.read(p), &|p| project.exists(p))
    };
    assert_eq!(root_of("sections/magic.tex", &[]).as_deref(), Some("/proj/main.tex"), "magic comment (relative)");
    assert_eq!(root_of("sections/abs.tex", &[]).as_deref(), Some("/proj/other.tex"), "magic comment (absolute)");
    assert_eq!(root_of("sections/badmagic.tex", &candidates).as_deref(), Some("/proj/main.tex"), "bad magic comment falls back to the candidates");
    assert_eq!(root_of("sections/self.tex", &[]).as_deref(), Some("/proj/sections/self.tex"), "a root document is its own root");
    assert_eq!(root_of("sections/intro.tex", &candidates).as_deref(), Some("/proj/main.tex"), "closure search; other.tex does not include it");
    assert_eq!(root_of("sections/deep.tex", &candidates).as_deref(), Some("/proj/main.tex"), "two levels deep");
    assert_eq!(root_of("shared.tex", &candidates).as_deref(), Some("/proj/main.tex"), "main.tex is preferred over a.tex when both include the file");
    let ao: Vec<String> = ["a.tex", "other.tex"].iter().map(|p| project.url(p)).collect();
    assert_eq!(root_of("shared.tex", &ao).as_deref(), Some("/proj/a.tex"));
    assert_eq!(root_of("lonely.tex", &candidates), None);
    assert_eq!(root_of("lonely.tex", &[]), None);
    assert_eq!(root_of("sub/s.tex", &[]).as_deref(), Some("/proj/main.tex"), "subfiles document points at its main file");
    assert_eq!(root_of("unreadable.tex", &[]), None);
}

#[test]
fn path_helpers() {
    assert_eq!(standardize("/a/b/../c/./d/"), "/a/c/d");
    assert_eq!(standardize("/.."), "/");
    assert_eq!(standardize("a/../../b"), "../b");
    assert_eq!(join("/proj/chapters", "../shared/x.tex"), "/proj/shared/x.tex");
    assert_eq!(join("/proj", "/abs/y"), "/abs/y");
    assert_eq!(parent_dir("/proj/main.tex"), "/proj");
    assert_eq!(parent_dir("/main.tex"), "/");
    assert_eq!(extension_lower("/a/B.TeX"), "tex");
    assert_eq!(extension_lower("/a/.tex"), "");
    assert_eq!(extension_lower("/a/x."), "");
    assert_eq!(relative_path("/proj/a/b.tex", "/proj").as_deref(), Some("a/b.tex"));
    assert_eq!(relative_path("/other/b.tex", "/proj"), None);
    assert!(natural_cmp("ch2.tex", "ch10.tex").is_lt());
    assert!(natural_cmp("A.tex", "b.tex").is_lt());
}

#[test]
fn text_decoding() {
    assert_eq!(decode_text(b"").unwrap().encoding, "utf-8");
    let d = decode_text("ação".as_bytes()).unwrap();
    assert_eq!((d.text.as_str(), d.encoding, d.invalid_byte_count), ("ação", "utf-8", 0));
    // Latin-1: "ação" as ISO-8859-1.
    let d = decode_text(&[0x61, 0xE7, 0xE3, 0x6F]).unwrap();
    assert_eq!((d.text.as_str(), d.encoding), ("a\u{e7}\u{e3}o", "latin-1"));
    // Mostly UTF-8 with a stray byte stays UTF-8.
    let mut mixed = "ação ".as_bytes().to_vec();
    mixed.push(0xFF);
    let d = decode_text(&mixed).unwrap();
    assert_eq!(d.encoding, "utf-8");
    assert_eq!(d.invalid_byte_count, 1);
    // BOMs.
    let d = decode_text(&[0xEF, 0xBB, 0xBF, b'x']).unwrap();
    assert_eq!((d.text.as_str(), d.has_bom), ("x", true));
    let d = decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]).unwrap();
    assert_eq!((d.text.as_str(), d.encoding), ("hi", "utf-16le"));
    // Binary.
    assert!(decode_text(&[1, 2, 0, 3]).is_none());
}

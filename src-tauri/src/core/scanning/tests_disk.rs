//! End-to-end tests of `build_project_index` on a real directory (overrides, root detection, encodings).

use crate::core::project_index::*;
use std::collections::HashMap;
use std::path::PathBuf;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let p = std::env::temp_dir().join(format!("gabetex-test-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn write(&self, rel: &str, text: &str) -> String {
        self.write_bytes(rel, text.as_bytes())
    }
    fn write_bytes(&self, rel: &str, bytes: &[u8]) -> String {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn project_on_disk_with_overrides() {
    let dir = Dir::new("idx");
    let main = dir.write(
        "main.tex",
        "\\documentclass{article}\n\\begin{document}\n\\section{Intro}\\label{sec:i}\nHello world.\n\\input{chapters/one}\n\\cite{k1,nope}\\ref{sec:i}\\ref{missing}\n\\bibliography{refs}\n\\end{document}\n",
    );
    let one = dir.write("chapters/one.tex", "\\subsection{One}\nTwo words.\n");
    dir.write("refs.bib", "@article{k1, title={T}, author={A}, year=2000}\n");
    dir.write("other.tex", "\\section{Not a root}\n");
    dir.write(".hidden/x.tex", "\\documentclass{article}");

    let idx = build_project_index(&dir.0, &HashMap::new());
    assert_eq!(idx.root_file.as_deref(), Some(to_native(&standardize(&main)).as_str()));
    assert_eq!(idx.outline.iter().map(|o| (o.level, o.title.as_str(), o.line)).collect::<Vec<_>>(), [(1, "Intro", 3), (2, "One", 1)]);
    assert_eq!(idx.outline[0].label.as_deref(), Some("sec:i"));
    assert_eq!(idx.labels.len(), 1);
    assert_eq!(idx.citations, ["k1", "nope"]);
    assert_eq!(idx.bib_keys.len(), 1);
    assert_eq!(idx.bib_keys[0].key, "k1");
    assert_eq!(idx.bib_keys[0].entry_type.as_deref(), Some("article"));
    assert_eq!(idx.includes.len(), 1);
    assert_eq!(idx.stats.words, 1 + 2 + 1 + 2, "Intro + Hello world + One + Two words");
    let mut messages: Vec<&str> = idx.warnings.iter().map(|w| w.message.as_str()).collect();
    messages.sort();
    assert_eq!(messages, ["Citation \u{2018}nope\u{2019} is not in the bibliography", "Reference \u{2018}missing\u{2019} is not defined"]);
    assert!(idx.warnings.iter().all(|w| w.source == "reference"));

    // An unsaved buffer wins over the disk: the missing reference gets defined, the citation fixed, a word added.
    let mut overrides = HashMap::new();
    overrides.insert(
        main.clone(),
        "\\documentclass{article}\n\\begin{document}\n\\section{Intro}\\label{sec:i}\\label{missing}\nHello brave new world.\n\\input{chapters/one}\n\\cite{k1}\\ref{sec:i}\\ref{missing}\n\\bibliography{refs}\n\\end{document}\n".to_string(),
    );
    overrides.insert(one.clone(), "\\subsection{Renamed}\n".to_string());
    let idx = build_project_index(&dir.0, &overrides);
    assert_eq!(idx.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["Intro", "Renamed"]);
    assert!(idx.warnings.is_empty(), "{:?}", idx.warnings);
    assert_eq!(idx.stats.words, 1 + 4 + 1);

    // A new, unsaved file (not on disk) is picked up through the override.
    let new_file = dir.0.join("fresh.tex").to_string_lossy().into_owned();
    let mut o2 = HashMap::new();
    o2.insert(new_file, "\\documentclass{article}\n\\begin{document}\\section{Fresh}\\end{document}".to_string());
    let idx = build_project_index(&dir.0, &o2);
    // `main.tex` is still preferred over `fresh.tex`.
    assert_eq!(idx.root_file.as_deref(), Some(to_native(&standardize(&main)).as_str()));
}

#[test]
fn root_detection_and_magic_comments() {
    let dir = Dir::new("root");
    dir.write("a.tex", "\\documentclass{article}\n\\begin{document}\\section{A}\\end{document}\n");
    dir.write("b.tex", "\\documentclass{article}\n\\begin{document}\\section{B}\\end{document}\n");
    // No main.tex: the first candidate in natural order.
    let idx = build_project_index(&dir.0, &HashMap::new());
    assert!(idx.root_file.as_deref().unwrap().ends_with("a.tex"));

    // An explicit root file path indexes that file.
    let b = dir.0.join("b.tex");
    let idx = build_project_index(&b, &HashMap::new());
    assert!(idx.root_file.as_deref().unwrap().ends_with("b.tex"));
    assert_eq!(idx.outline[0].title, "B");

    // current_file + magic comment.
    dir.write("sec/s.tex", "% !TEX root = ../b.tex\n\\section{S}\n");
    let analysis = build_project_analysis(
        &dir.0,
        &HashMap::new(),
        &ProjectOptions { current_file: Some(dir.0.join("sec/s.tex").to_string_lossy().into_owned()), ..Default::default() },
    )
    .unwrap();
    assert!(analysis.model.root_file.ends_with("b.tex"));
    assert_eq!(analysis.root_candidates.len(), 2);

    // Empty directory: no root.
    let empty = Dir::new("empty");
    let idx = build_project_index(&empty.0, &HashMap::new());
    assert!(idx.root_file.is_none() && idx.outline.is_empty());
}

#[test]
fn latin1_and_utf8_files_are_read() {
    let dir = Dir::new("enc");
    // "\section{Introdução}" in Latin-1.
    let mut latin1 = b"\\documentclass{article}\n\\begin{document}\n\\section{Introdu".to_vec();
    latin1.extend_from_slice(&[0xE7, 0xE3]);
    latin1.extend_from_slice(b"o}\n\\end{document}\n");
    dir.write_bytes("main.tex", &latin1);
    let idx = build_project_index(&dir.0, &HashMap::new());
    assert_eq!(idx.outline[0].title, "Introdução");

    let dir = Dir::new("enc8");
    dir.write("main.tex", "\u{FEFF}\\documentclass{article}\n\\begin{document}\n\\section{Introdução}\n\\end{document}\n");
    let idx = build_project_index(&dir.0, &HashMap::new());
    assert_eq!(idx.outline[0].title, "Introdução");
}

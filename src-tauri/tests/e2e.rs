//! End-to-end checks of the Rust core against a real TeX installation: ports of the Swift
//! `TeXToolchainChecks` `IntegrationChecks` / `RegressionChecks`. Every test skips (passes) when latexmk,
//! pdflatex or synctex is not installed, so CI machines without TeX stay green.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gabetex_lib::core::compiler::{CompileOutcome, CompileRequest, CompileStatus, Engine, LaTeXCompiler, Purpose};
use gabetex_lib::core::distribution::{self, Distribution, Tool};
use gabetex_lib::core::log_parser::{LogDiagnostic, Severity, Source};
use gabetex_lib::core::project_index::build_project_index;
use gabetex_lib::core::synctex::SyncTeX;

fn dist() -> Option<Distribution> {
    static D: OnceLock<Option<Distribution>> = OnceLock::new();
    D.get_or_init(|| distribution::locate(None).filter(|d| d.has(Tool::Latexmk) && d.has(Tool::Pdflatex))).clone()
}

/// Skips the test (returns early) when no usable TeX installation exists.
macro_rules! need_tex {
    () => {
        match dist() {
            Some(d) => d,
            None => {
                eprintln!("SKIP: no TeX distribution with latexmk");
                return;
            }
        }
    };
}

fn scratch_root() -> PathBuf {
    let p = std::env::temp_dir().join(format!("gabetex-e2e-{}", std::process::id()));
    let _ = fs::create_dir_all(&p);
    fs::canonicalize(&p).unwrap_or(p)
}

fn scratch(name: &str) -> PathBuf {
    let p = scratch_root().join(name);
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap().flatten() {
        let t = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            fs::copy(e.path(), &t).unwrap();
        }
    }
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn compiler(dist: &Distribution, name: &str) -> LaTeXCompiler {
    LaTeXCompiler::with_shadow_root(dist.clone(), &s(&scratch(&format!("shadow-{name}"))))
}

fn req(project: &Path, root: &Path, engine: Engine, purpose: Purpose) -> CompileRequest {
    CompileRequest::new(&s(project), &s(root), engine, purpose)
}

fn describe(ds: &[LogDiagnostic]) -> String {
    ds.iter()
        .map(|d| format!("[{:?}/{:?}] {} @ {}:{:?}", d.severity, d.source, d.message, d.path.as_deref().unwrap_or("-"), d.line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn find<'a>(ds: &'a [LogDiagnostic], message: &str, file: &Path, line: u32) -> &'a LogDiagnostic {
    ds.iter()
        .find(|d| d.message == message && d.path.as_deref() == Some(&s(file)) && d.line == Some(line))
        .unwrap_or_else(|| panic!("no diagnostic '{message}' at {}:{line}; got:\n{}", s(file), describe(ds)))
}

fn assert_status(o: &CompileOutcome, status: CompileStatus) {
    assert_eq!(o.status, status, "{}\n{}", describe(&o.diagnostics), o.tool_output.chars().rev().take(500).collect::<String>().chars().rev().collect::<String>());
}

#[test]
fn detection() {
    let d = need_tex!();
    for tool in [Tool::Pdflatex, Tool::Latexmk, Tool::Synctex] {
        assert!(d.has(tool), "{tool:?} found");
    }
    let env = d.process_environment();
    assert!(env.contains(&("max_print_line".to_string(), "10000".to_string())));
    assert!(env.iter().any(|(k, v)| k == "PATH" && v.starts_with(&d.bin_dir)));
    let ipc = d.to_ipc();
    assert!(ipc.latexmk && ipc.synctex);
    assert!(distribution::inspect(Path::new("/nonexistent-dir")).is_none());
}

#[test]
fn live_overlay_then_build() {
    let d = need_tex!();
    let c = compiler(&d, "overlay");
    let project = scratch("two-file");
    let main = project.join("main.tex");
    let intro = project.join("sections/intro.tex");
    write(&main, "\\documentclass{article}\n\\begin{document}\n\\input{sections/intro}\n\\typeout{GTX-MAIN-MARKER}\n\\end{document}\n");
    write(&intro, "Introduction on disk.\n\\typeout{GTX-DISK-MARKER}\n");

    let mut r = req(&project, &main, Engine::Pdflatex, Purpose::Live);
    r.overlays.insert(s(&intro), "Introduction, unsaved.\n\\typeout{GTX-OVERLAY-MARKER}\n\\undefinedinoverlay\n".into());
    let live = c.compile(&r);
    assert_status(&live, CompileStatus::Failed);
    assert!(live.log.contains("GTX-OVERLAY-MARKER") && !live.log.contains("GTX-DISK-MARKER"), "the live compile used the unsaved buffer");
    assert!(live.log.contains("GTX-MAIN-MARKER"));
    let diag = find(&live.diagnostics, "Undefined control sequence \\undefinedinoverlay", &intro, 3);
    assert_eq!(diag.severity, Severity::Error);
    assert_eq!(diag.column, Some(1));
    assert_eq!(diag.length, Some(19));
    let pdf = live.pdf_path.clone().expect("pdf");
    assert!(Path::new(&pdf).exists());
    assert!(pdf.starts_with(&c.shadow_root));
    assert!(live.exported_pdf_path.is_none(), ".live never exports");
    assert!(!project.join("main.pdf").exists(), "project untouched by .live");
    assert_eq!(fs::read_to_string(&intro).unwrap(), "Introduction on disk.\n\\typeout{GTX-DISK-MARKER}\n");
    assert_eq!(live.path_mapper.project_directory, s(&project));

    let build = c.compile(&req(&project, &main, Engine::Pdflatex, Purpose::Build));
    assert_status(&build, CompileStatus::Succeeded);
    assert!(build.log.contains("GTX-DISK-MARKER") && !build.log.contains("GTX-OVERLAY-MARKER"));
    assert_eq!(build.exported_pdf_path.as_deref(), Some(s(&project.join("main.pdf")).as_str()));
    assert!(project.join("main.pdf").exists());
    assert!(build.synctex_path.is_some(), "synctex produced");
    assert_eq!(build.diagnostics.iter().filter(|d| d.severity >= Severity::Warning).count(), 0, "{}", describe(&build.diagnostics));
    let mut left: Vec<String> = fs::read_dir(&project).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, ["main.pdf", "main.tex", "sections"], "no auxiliary files in the project");
    c.clean(&s(&project));
    assert!(!Path::new(&c.shadow_directory(&s(&project))).exists(), "clean removes the workspace");
}

#[test]
fn root_in_subdirectory_with_include() {
    let d = need_tex!();
    let c = compiler(&d, "subdir");
    let project = scratch("subdir");
    let main = project.join("thesis/main.tex");
    let chapter = project.join("thesis/chapters/one.tex");
    write(&main, "\\documentclass{report}\n\\begin{document}\n\\include{chapters/one}\n\\end{document}\n");
    write(&chapter, "\\chapter{One}\nText \\oops here.\n");
    let r = c.compile(&req(&project, &main, Engine::Pdflatex, Purpose::Build));
    assert_status(&r, CompileStatus::Failed);
    let diag = find(&r.diagnostics, "Undefined control sequence \\oops", &chapter, 2);
    assert_eq!((diag.column, diag.length), (Some(6), Some(5)));
    assert!(!r.diagnostics.iter().any(|d| d.message.contains("chapters/one.aux")), "{}", describe(&r.diagnostics));
    assert_eq!(r.exported_pdf_path.as_deref(), Some(s(&project.join("thesis/main.pdf")).as_str()));
}

fn looping(project: &Path) -> PathBuf {
    let main = project.join("main.tex");
    write(&main, "\\documentclass{article}\n\\begin{document}\n\\def\\loop{\\loop}\\loop\n\\end{document}\n");
    main
}

fn pgrep(pattern: &str) -> bool {
    std::process::Command::new("pgrep").args(["-f", pattern]).output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn cancellation_by_newer_compile_and_timeout() {
    let d = need_tex!();
    let c = std::sync::Arc::new(compiler(&d, "cancel"));
    let project = scratch("loop");
    let main = looping(&project);
    let p1 = s(&project);
    let m1 = main.clone();
    let c1 = c.clone();
    let first = std::thread::spawn(move || c1.compile(&req(Path::new(&p1), &m1, Engine::Pdflatex, Purpose::Live)));
    std::thread::sleep(Duration::from_millis(1500));
    write(&main, "\\documentclass{article}\n\\begin{document}\nFixed.\n\\end{document}\n");
    let start = Instant::now();
    // The newer compile supersedes the looping one.
    let next = c.compile(&req(&project, &main, Engine::Pdflatex, Purpose::Live));
    let cancelled = first.join().unwrap();
    assert_eq!(cancelled.status, CompileStatus::Cancelled);
    assert!(start.elapsed() < Duration::from_secs(15), "cancel is prompt ({:?})", start.elapsed());
    assert_status(&next, CompileStatus::Succeeded);

    // Explicit cancel API.
    let main = looping(&project);
    let c2 = c.clone();
    let p2 = s(&project);
    let m2 = main.clone();
    let t = std::thread::spawn(move || c2.compile(&req(Path::new(&p2), &m2, Engine::Pdflatex, Purpose::Live)));
    std::thread::sleep(Duration::from_millis(1500));
    c.cancel(&s(&project));
    assert_eq!(t.join().unwrap().status, CompileStatus::Cancelled);

    // Timeout: the endless loop is stopped and reported.
    let mut r = req(&project, &main, Engine::Pdflatex, Purpose::Live);
    r.timeout = Some(Duration::from_secs(3));
    let timed = c.compile(&r);
    assert_eq!(timed.status, CompileStatus::Failed);
    assert!(timed.diagnostics.iter().any(|d| d.source == Source::Latexmk && d.message.contains("3 seconds")), "{}", describe(&timed.diagnostics));
    std::thread::sleep(Duration::from_millis(300));
    let id = gabetex_lib::core::compiler::ShadowWorkspace::identifier(&s(&project));
    assert!(!pgrep(&format!("pdflatex.*{id}")), "no TeX process after the timeout");
}

#[test]
fn bibtex_and_missing_package() {
    let d = need_tex!();
    let c = compiler(&d, "bib");
    let fixtures = repo().join("TeXToolchain/projects");
    let bib = scratch("bibtex");
    copy_dir(&fixtures.join("bibtex"), &bib);
    let r = c.compile(&req(&bib, &bib.join("main.tex"), Engine::Pdflatex, Purpose::Live));
    assert_status(&r, CompileStatus::Failed);
    let b = find(&r.diagnostics, "I was expecting a `,' or a `}'", &bib.join("refs.bib"), 11);
    assert_eq!((b.severity, b.source), (Severity::Error, Source::Bibtex));
    let w = find(&r.diagnostics, "Citation `nokey' on page 1 undefined.", &bib.join("main.tex"), 3);
    assert_eq!(w.severity, Severity::Warning);

    let pkg = scratch("missingpkg");
    copy_dir(&fixtures.join("missingpkg"), &pkg);
    let m = c.compile(&req(&pkg, &pkg.join("main.tex"), Engine::Pdflatex, Purpose::Build));
    assert_status(&m, CompileStatus::Failed);
    let p = find(&m.diagnostics, "File `thispackagedoesnotexist.sty' not found.", &pkg.join("main.tex"), 3);
    assert_eq!(p.missing_file.as_deref(), Some("thispackagedoesnotexist.sty"));
    assert_eq!(m.missing_files, ["thispackagedoesnotexist.sty"]);
    assert!(m.exported_pdf_path.is_none(), "no pages of output -> nothing exported");

    let missing = c.compile(&req(&pkg, &pkg.join("nope.tex"), Engine::Pdflatex, Purpose::Live));
    assert!(matches!(missing.status, CompileStatus::ToolError(_)), "{:?}", missing.status);
    let outside = c.compile(&req(&pkg, Path::new("/tmp/x.tex"), Engine::Pdflatex, Purpose::Live));
    assert!(matches!(outside.status, CompileStatus::ToolError(_)), "{:?}", outside.status);
    let mut no_latexmk = d.clone();
    no_latexmk.tools.remove(&Tool::Latexmk);
    no_latexmk.tools.remove(&Tool::Pdflatex);
    let broken = LaTeXCompiler::with_shadow_root(no_latexmk, &c.shadow_root).compile(&req(&pkg, &pkg.join("main.tex"), Engine::Pdflatex, Purpose::Live));
    match broken.status {
        CompileStatus::ToolError(m) => assert!(m.contains("latexmk"), "{m}"),
        other => panic!("no latexmk -> toolError, got {other:?}"),
    }
}

#[test]
fn fixture_projects_diagnostics() {
    let d = need_tex!();
    let c = compiler(&d, "fixtures");
    let fixtures = repo().join("TeXToolchain/projects");
    for name in ["multifile", "missinginput", "runaway", "badboxes", "unicode"] {
        let dir = scratch(&format!("fx-{name}"));
        copy_dir(&fixtures.join(name), &dir);
        let o = c.compile(&req(&dir, &dir.join("main.tex"), Engine::Pdflatex, Purpose::Live));
        assert!(!matches!(o.status, CompileStatus::ToolError(_) | CompileStatus::Cancelled), "{name}: {:?}", o.status);
        for diag in &o.diagnostics {
            if let Some(p) = &diag.path {
                assert!(p.starts_with(&s(&dir)), "{name}: diagnostic path leaks outside the project: {p}\n{}", describe(&o.diagnostics));
                assert!(diag.line.unwrap_or(1) >= 1);
            }
        }
        match name {
            "multifile" | "missinginput" | "runaway" => assert!(o.diagnostics.iter().any(|d| d.severity == Severity::Error), "{name}: {}", describe(&o.diagnostics)),
            "badboxes" => assert!(o.diagnostics.iter().any(|d| d.severity == Severity::BadBox), "{}", describe(&o.diagnostics)),
            "unicode" => {
                let chapter = dir.join("Se\u{e7}\u{f5}es/cap\u{ed}tulo 1.tex");
                find(&o.diagnostics, "Undefined control sequence \\comandoinexistente", &chapter, 2);
            }
            _ => {}
        }
    }
}

#[test]
fn symlinked_folder_overlay_leaves_real_file_untouched() {
    #[cfg(unix)]
    {
        let d = need_tex!();
        let c = compiler(&d, "symlink");
        let base = scratch("symlinked-compile");
        let shared = base.join("shared");
        let project = base.join("proj");
        let main = project.join("main.tex");
        write(&main, "\\documentclass{article}\n\\begin{document}\n\\input{chapters/one}\n\\end{document}\n");
        write(&shared.join("chapters/one.tex"), "Saved text.\n\\typeout{GTX-REAL-ONE}\n");
        std::os::unix::fs::symlink("../shared/chapters", project.join("chapters")).unwrap();
        let before = fs::metadata(shared.join("chapters/one.tex")).unwrap().modified().unwrap();
        let one = project.join("chapters/one.tex");
        let mut r = req(&project, &main, Engine::Pdflatex, Purpose::Live);
        r.overlays.insert(s(&one), "Unsaved text.\n\\typeout{GTX-OVERLAY-ONE}\n\\oopsoverlay\n".into());
        let live = c.compile(&r);
        assert_status(&live, CompileStatus::Failed);
        assert!(live.log.contains("GTX-OVERLAY-ONE") && !live.log.contains("GTX-REAL-ONE"));
        find(&live.diagnostics, "Undefined control sequence \\oopsoverlay", &one, 3);
        assert_eq!(fs::read_to_string(shared.join("chapters/one.tex")).unwrap(), "Saved text.\n\\typeout{GTX-REAL-ONE}\n");
        assert_eq!(fs::metadata(shared.join("chapters/one.tex")).unwrap().modified().unwrap(), before);
        assert_eq!(fs::read_dir(shared.join("chapters")).unwrap().count(), 1, "nothing added next to the real file");

        let saved = c.compile(&req(&project, &main, Engine::Pdflatex, Purpose::Live));
        assert_status(&saved, CompileStatus::Succeeded);
        assert!(saved.log.contains("GTX-REAL-ONE"));
        write(&shared.join("chapters/one.tex"), "Saved text.\n\\oopsondisk\n");
        let on_disk = c.compile(&req(&project, &main, Engine::Pdflatex, Purpose::Live));
        find(&on_disk.diagnostics, "Undefined control sequence \\oopsondisk", &one, 2);
    }
}

#[test]
fn ancestors_outside_the_project() {
    let d = need_tex!();
    let c = compiler(&d, "ancestors");
    let base = scratch("ancestors");
    let papers = base.join("papers");
    let project = papers.join("paper1");
    let main = project.join("main.tex");
    let macros = papers.join("macros.tex");
    write(&main, "\\documentclass{article}\n\\input{../macros}\n\\input{../../common/defs}\n\\begin{document}\n\\gtxmacro\\ \\gtxdefs\\ \\cite{knuth}.\n\\bibliographystyle{plain}\n\\bibliography{../refs}\n\\end{document}\n");
    write(&macros, "\\newcommand{\\gtxmacro}{GtxMacroText}\n\\typeout{GTX-MACROS-LOADED}\n\\undefinedinmacros\n");
    write(&base.join("common/defs.tex"), "\\newcommand{\\gtxdefs}{GtxDefsText}\n");
    write(&papers.join("refs.bib"), "@book{knuth, author = {Donald E. Knuth}, title = {The {\\TeX}book},\n  publisher = {Addison-Wesley}, year = {1984}}\n");
    let r = c.compile(&req(&project, &main, Engine::Pdflatex, Purpose::Live));
    assert_status(&r, CompileStatus::Failed);
    assert!(r.log.contains("GTX-MACROS-LOADED"));
    find(&r.diagnostics, "Undefined control sequence \\undefinedinmacros", &macros, 3);

    write(&macros, "\\newcommand{\\gtxmacro}{GtxMacroText}\n\\typeout{GTX-MACROS-LOADED}\n");
    let fixed = c.compile(&req(&project, &main, Engine::Pdflatex, Purpose::Build));
    assert_status(&fixed, CompileStatus::Succeeded);
    assert_eq!(fixed.diagnostics.iter().filter(|d| d.severity >= Severity::Warning).count(), 0, "{}", describe(&fixed.diagnostics));
    let mut names: Vec<String> = fs::read_dir(&papers).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["macros.tex", "paper1", "refs.bib"], "nothing written next to files outside the project");

    let deep = base.join("l1/l2/l3/proj");
    let deep_main = deep.join("main.tex");
    write(&base.join("l1/near.tex"), "Near text.\n\\typeout{GTX-NEAR}\n");
    write(&base.join("far.tex"), "Far text.\n");
    write(&deep_main, "\\documentclass{article}\n\\begin{document}\n\\input{../../../near}\n\\input{../../../../far}\n\\end{document}\n");
    let u = c.compile(&req(&deep, &deep_main, Engine::Pdflatex, Purpose::Live));
    assert_status(&u, CompileStatus::Failed);
    assert!(u.log.contains("GTX-NEAR"), "3 levels up is still reachable");
    let far = s(&base.join("far.tex"));
    let ex = u.diagnostics.iter().find(|d| d.source == Source::Latexmk && d.message.contains(&far)).unwrap_or_else(|| panic!("{}", describe(&u.diagnostics)));
    assert_eq!((ex.path.as_deref(), ex.line), (Some(s(&deep_main).as_str()), Some(4)));
}

#[test]
fn latexmkrc_and_latin1_overlay() {
    let d = need_tex!();
    let c = compiler(&d, "rc");
    let project = scratch("latexmkrc");
    let main = project.join("main.tex");
    let rc = project.join(".latexmkrc");
    write(&rc, "ensure_path('TEXINPUTS', './rcinputs//');\n");
    write(&project.join("rcinputs/gtxrc.sty"), "\\ProvidesPackage{gtxrc}\n\\typeout{GTX-LATEXMKRC-OK}\n");
    write(&main, "\\documentclass{article}\n\\usepackage{gtxrc}\n\\begin{document}\nRC.\n\\end{document}\n");
    let r = c.compile(&req(&project, &main, Engine::Pdflatex, Purpose::Live));
    assert_status(&r, CompileStatus::Succeeded);
    assert!(r.log.contains("GTX-LATEXMKRC-OK"));
    fs::remove_file(&rc).unwrap();
    write(&main, "\\documentclass{article}\n\\usepackage{gtxrc}\n\\begin{document}\nRC again.\n\\end{document}\n");
    let without = c.compile(&req(&project, &main, Engine::Pdflatex, Purpose::Live));
    assert_eq!(without.status, CompileStatus::Failed);
    assert!(without.diagnostics.iter().any(|d| d.missing_file.as_deref() == Some("gtxrc.sty")), "{}", describe(&without.diagnostics));

    let project = scratch("latin1");
    let main = project.join("main.tex");
    write(&main, "% saved\n");
    let source = "\\documentclass{article}\n\\usepackage[T1]{fontenc}\n\\usepackage[latin1]{inputenc}\n\\begin{document}\na\u{e7}\u{e3}o A\u{c7}\u{c3}O\n\\end{document}\n";
    let mut r = req(&project, &main, Engine::Pdflatex, Purpose::Live);
    r.overlays.insert(s(&main), source.into());
    r.overlay_encodings.insert(s(&main), "latin-1".into());
    let ok = c.compile(&r);
    assert_status(&ok, CompileStatus::Succeeded);
    assert!(!ok.diagnostics.iter().any(|d| d.message.contains("inputenc") || d.message.contains("Keyboard character")), "{}", describe(&ok.diagnostics));
    let shadow_main = ok.path_mapper.shadow_path(&s(&main));
    let latin1: Vec<u8> = source.chars().map(|ch| ch as u32 as u8).collect();
    assert_eq!(fs::read(&shadow_main).unwrap(), latin1, "the overlay was written in Latin-1");
    assert_eq!(fs::read_to_string(&main).unwrap(), "% saved\n");

    // Control: UTF-8 bytes are rejected by inputenc{latin1}.
    let mut r = req(&project, &main, Engine::Pdflatex, Purpose::Live);
    r.overlays.insert(s(&main), source.into());
    let utf8 = c.compile(&r);
    assert!(utf8.diagnostics.iter().any(|d| d.message.contains("inputenc") || d.message.contains("Keyboard character")), "{}", describe(&utf8.diagnostics));
}

#[test]
fn other_engines() {
    let d = need_tex!();
    let c = compiler(&d, "engines");
    let project = scratch("engines");
    let main = project.join("main.tex");
    write(&main, "\\documentclass{article}\n\\begin{document}\nHello engines.\n\\oopsengine\n\\end{document}\n");
    for engine in [Engine::Xelatex, Engine::Lualatex] {
        if !d.has(engine.tool()) {
            eprintln!("SKIP {engine:?}: not installed");
            continue;
        }
        let r = c.compile(&req(&project, &main, engine, Purpose::Live));
        assert_status(&r, CompileStatus::Failed);
        find(&r.diagnostics, "Undefined control sequence \\oopsengine", &main, 4);
        assert!(r.pdf_path.is_some());
    }
}

#[test]
fn synctex_round_trip() {
    let d = need_tex!();
    if !d.has(Tool::Synctex) {
        eprintln!("SKIP: synctex missing");
        return;
    }
    let c = compiler(&d, "synctex");
    let project = scratch("synctex");
    let main = project.join("main.tex");
    let body = project.join("sections/body.tex");
    write(&main, "\\documentclass{article}\n\\usepackage[a4paper,margin=2cm]{geometry}\n\\begin{document}\nFIRSTLINE alpha beta gamma.\n\n\\input{sections/body}\n\n\\end{document}\n");
    let lines: Vec<String> = (1..=80).map(|k| format!("Paragraph MARK{k}X text text text.\n")).collect();
    write(&body, &lines.join("\n")); // paragraph k is on line 2k-1
    let r = c.compile(&req(&project, &main, Engine::Pdflatex, Purpose::Live));
    assert_status(&r, CompileStatus::Succeeded);
    let pdf = r.pdf_path.clone().unwrap();
    let sync = SyncTeX::new(&d);
    let mut pages = Vec::new();
    for k in [1u32, 2, 10, 25, 60, 80] {
        let line = 2 * k - 1;
        let boxes = sync.forward(&pdf, &s(&body), line, 1, &r.path_mapper);
        let b = boxes.first().unwrap_or_else(|| panic!("forward found nothing for line {line}"));
        assert!(b.width > 0.0 && b.height > 0.0 && b.x >= 0.0 && b.y >= 0.0, "{b:?}");
        if k == 1 || k == 80 {
            pages.push(b.page);
        }
        let loc = sync.inverse(&pdf, b.page, b.x + 30.0, b.y + b.height / 2.0, &r.path_mapper).unwrap_or_else(|| panic!("inverse nothing for line {line}"));
        assert_eq!(loc.path, s(&body), "inverse file for line {line}");
        assert_eq!(loc.line, line, "inverse(forward(line {line}))");
    }
    assert_eq!(pages, [1, 2], "the document spans two pages and both are found");
    let first = sync.forward(&pdf, &s(&main), 4, 1, &r.path_mapper);
    assert_eq!(first.first().map(|b| b.page), Some(1));
    assert!(sync.inverse(&pdf, 99, 0.0, 0.0, &r.path_mapper).is_none(), "page out of range");
    assert!(sync.forward(&pdf, &s(&project.join("nothere.tex")), 1, 1, &r.path_mapper).is_empty());
}

#[test]
fn chktex_live() {
    let d = need_tex!();
    if !d.has(Tool::Chktex) {
        eprintln!("SKIP: chktex missing");
        return;
    }
    let text = "Ol\u{e1} a\u{e7}\u{e3}o $x^10$ fim.\nSee section \\ref{a} and (0,1].\n``quoted\" text\n";
    let ds = gabetex_lib::core::compiler::chktex_lint(&d, text, "/tmp/sample.tex", &gabetex_lib::core::compiler::CHKTEX_DEFAULT_SUPPRESSIONS);
    let sup = ds.iter().find(|d| d.line == Some(1)).unwrap_or_else(|| panic!("{}", describe(&ds)));
    assert_eq!((sup.column, sup.length), (Some(13), Some(2)), "UTF-16 column of 10 in x^10: {}", describe(&ds));
    assert!(ds.iter().any(|d| d.line == Some(2) && d.message.contains("Non-breaking space")), "{}", describe(&ds));
    assert!(ds.iter().all(|d| d.source == Source::Chktex && d.path.as_deref() == Some("/tmp/sample.tex")));
}

#[test]
fn examples_and_scratch_project() {
    let d = need_tex!();
    let c = compiler(&d, "examples");
    // Shipped example: index, then a real build.
    let ex = scratch("homologia");
    copy_dir(&repo().join("Homologia"), &ex);
    let idx = build_project_index(&ex, &HashMap::new());
    assert_eq!(idx.root_file.as_deref(), Some(s(&ex.join("main.tex")).as_str()));
    assert!(!idx.outline.is_empty() && !idx.includes.is_empty());
    let r = c.compile(&req(&ex, &ex.join("main.tex"), Engine::Pdflatex, Purpose::Build));
    // The example may need packages absent from a minimal TeX install: then they must be reported as missing.
    if r.status != CompileStatus::Succeeded {
        assert!(!r.missing_files.is_empty() || r.diagnostics.iter().any(|d| d.severity == Severity::Error), "{}", describe(&r.diagnostics));
    } else {
        assert!(ex.join("main.pdf").exists());
    }

    // Scratch multi-file project: \input, label/ref, bibtex, deliberate error, missing package.
    let p = scratch("multi");
    let main = p.join("main.tex");
    let a = p.join("sections/a.tex");
    write(&main, "\\documentclass{article}\n\\usepackage{nonexistentpkgxyz}\n\\begin{document}\n\\section{Top}\\label{sec:top}\nSee \\ref{sec:a} \\cite{k1}.\n\\input{sections/a}\n\\bibliographystyle{plain}\n\\bibliography{refs}\n\\end{document}\n");
    write(&a, "\\section{A}\\label{sec:a}\nBack to \\ref{sec:top}.\n\\deliberateerror\n");
    write(&p.join("refs.bib"), "@book{k1, author={A. B}, title={T}, publisher={P}, year={2000}}\n");
    let idx = build_project_index(&p, &HashMap::new());
    assert!(idx.labels.iter().any(|l| l.name == "sec:a") && idx.labels.iter().any(|l| l.name == "sec:top"));
    assert!(idx.bib_keys.iter().any(|b| b.key == "k1"));
    let o = c.compile(&req(&p, &main, Engine::Pdflatex, Purpose::Live));
    assert_eq!(o.status, CompileStatus::Failed);
    assert_eq!(o.missing_files, ["nonexistentpkgxyz.sty"], "{}", describe(&o.diagnostics));
    find(&o.diagnostics, "File `nonexistentpkgxyz.sty' not found.", &main, 2);
    // Fix the package: the deliberate error then lands in the \input'ed file with the right line.
    write(&main, &fs::read_to_string(&main).unwrap().replace("\\usepackage{nonexistentpkgxyz}\n", ""));
    let o = c.compile(&req(&p, &main, Engine::Pdflatex, Purpose::Live));
    assert_eq!(o.status, CompileStatus::Failed);
    find(&o.diagnostics, "Undefined control sequence \\deliberateerror", &a, 3);
    assert!(o.missing_files.is_empty());
}

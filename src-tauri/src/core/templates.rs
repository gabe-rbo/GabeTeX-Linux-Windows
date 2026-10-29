//! Project templates: port of `TeXCore/Models/ProjectTemplate.swift`, `TeXCore/Templates/ProjectTemplates.swift`
//! (data lives in `data/templates.json`) and the Mac app's `ProjectCreator` (new-project flow).
//!
//! * placeholders `{{TITLE}}`, `{{AUTHOR}}`, `{{DATE}}` are replaced **sequentially in that order** (like the
//!   Swift `replacingOccurrences` chain); `{{DATE}}` is the ISO date (`yyyy-MM-dd`, UTC like
//!   `ISO8601DateFormatter` with `.withFullDate`);
//! * title and author are made LaTeX-safe first ([`latex_safe`]);
//! * the target folder must not exist or must be empty (hidden entries aside), otherwise the call refuses.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::core::settings::write_atomic;

const TEMPLATES_JSON: &str = include_str!("data/templates.json");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFile {
    path: String,
    contents: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTemplate {
    id: String,
    name: String,
    description: String,
    symbol: String,
    #[serde(default = "default_engine")]
    engine: String,
    files: Vec<RawFile>,
    #[serde(default = "default_root_file")]
    root_file: String,
    #[serde(default)]
    #[allow(dead_code)]
    placeholders: Vec<String>,
}

fn default_engine() -> String {
    "pdflatex".into()
}

fn default_root_file() -> String {
    "main.tex".into()
}

/// What the "New Project" sheet needs to list a template (no file contents).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    /// SF Symbol name of the Mac app (the frontend maps it to an icon).
    pub symbol: String,
    pub engine: String,
    pub root_file: String,
    /// Relative paths (with `/`) of the files the template creates.
    pub files: Vec<String>,
}

/// Result of [`create_project`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedProject {
    /// Absolute path of the new project folder.
    pub root: String,
    /// Folder name.
    pub name: String,
    /// Absolute path of the template's root document.
    pub root_file: String,
    pub engine: String,
}

fn raw_templates() -> &'static [RawTemplate] {
    static CELL: OnceLock<Vec<RawTemplate>> = OnceLock::new();
    CELL.get_or_init(|| serde_json::from_str(TEMPLATES_JSON).expect("data/templates.json is valid"))
}

/// All templates, in the order of `ProjectTemplates.all`.
pub fn list_templates() -> Vec<TemplateInfo> {
    raw_templates()
        .iter()
        .map(|t| TemplateInfo {
            id: t.id.clone(),
            name: t.name.clone(),
            description: t.description.clone(),
            symbol: t.symbol.clone(),
            engine: t.engine.clone(),
            root_file: t.root_file.clone(),
            files: t.files.iter().map(|f| f.path.clone()).collect(),
        })
        .collect()
}

fn find_template(id: &str) -> Option<&'static RawTemplate> {
    raw_templates().iter().find(|t| t.id == id)
}

/// `ProjectTemplate.instantiate`: the template's files (relative path, contents) with placeholders substituted.
/// `date` is the already formatted ISO date.
pub fn instantiate(template_id: &str, title: &str, author: &str, date: &str) -> Option<Vec<(String, String)>> {
    let t = find_template(template_id)?;
    Some(t.files.iter().map(|f| (f.path.clone(), substitute(&f.contents, title, author, date))).collect())
}

/// Sequential replacement, exactly like the Swift chain (a title that contains `{{AUTHOR}}` is expanded too).
fn substitute(text: &str, title: &str, author: &str, date: &str) -> String {
    text.replace("{{TITLE}}", title).replace("{{AUTHOR}}", author).replace("{{DATE}}", date)
}

/// Today's date as `yyyy-MM-dd` (UTC).
pub fn today_iso() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    iso_date(secs)
}

/// `yyyy-MM-dd` (UTC) of a Unix timestamp in seconds.
pub fn iso_date(epoch_seconds: i64) -> String {
    let days = epoch_seconds.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// True for scalars that attach to the previous character (a grapheme cluster `&` + mark is not `&`).
fn is_extender(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x0610..=0x061A | 0x064B..=0x065F
        | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200C | 0x200D | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0xE0100..=0xE01EF)
}

/// `ProjectCreator.latexSafe`: makes plain text safe inside `\title{…}` / `\author{…}`. Text that already
/// contains math (`$`) or commands (`\`) is taken to be LaTeX and kept.
pub fn latex_safe(text: &str) -> String {
    if text.contains('$') || text.contains('\\') {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    for (i, &c) in chars.iter().enumerate() {
        // Swift iterates grapheme clusters: a special character followed by a combining mark is not special.
        let extended = chars.get(i + 1).is_some_and(|&n| is_extender(n));
        if extended {
            out.push(c);
            continue;
        }
        match c {
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '#' => out.push_str("\\#"),
            '_' => out.push_str("\\_"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            c => out.push(c),
        }
    }
    out
}

/// `ProjectCreator.sanitized`: removes characters that are not allowed (or are awkward) in folder names.
pub fn sanitized(raw: &str) -> String {
    let mapped: String = raw
        .chars()
        .map(|c| {
            let forbidden = matches!(c, '/' | ':' | '\\' | '\0') || c.is_control();
            #[cfg(windows)]
            let forbidden = forbidden || matches!(c, '<' | '>' | '"' | '|' | '?' | '*');
            if forbidden { '-' } else { c }
        })
        .collect();
    let mut result = mapped.trim();
    while let Some(rest) = result.strip_prefix('.') {
        result = rest;
    }
    #[cfg(windows)]
    {
        // Windows silently drops trailing dots and spaces from folder names.
        result = result.trim_end_matches(['.', ' ']);
    }
    result.to_string()
}

/// `ProjectCreator.suggestedFolderName`: a folder name derived from the title ("Untitled Project" when empty).
pub fn suggested_folder_name(title: &str) -> String {
    let cleaned = sanitized(title);
    if cleaned.is_empty() { "Untitled Project".to_string() } else { cleaned }
}

/// `ProjectCreator.write`: writes the template into `folder`, which must not exist or must be empty
/// (hidden files aside). `title` and `author` are plain text (made LaTeX-safe here).
pub fn write_template(template_id: &str, title: &str, author: &str, folder: &Path, date: &str) -> Result<(), String> {
    let template = find_template(template_id).ok_or_else(|| format!("Unknown template “{template_id}”."))?;
    let folder_name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match fs::metadata(folder) {
        Ok(meta) => {
            let non_hidden = if meta.is_dir() {
                fs::read_dir(folder)
                    .map(|rd| rd.filter_map(|e| e.ok()).any(|e| !e.file_name().to_string_lossy().starts_with('.')))
                    .unwrap_or(false)
            } else {
                true
            };
            if !meta.is_dir() || non_hidden {
                return Err(format!(
                    "“{folder_name}” already exists in this location. Choose another name or location."
                ));
            }
        }
        Err(_) => {
            fs::create_dir_all(folder).map_err(|_| {
                "The folder could not be created. Check that you have permission to write in this location.".to_string()
            })?;
        }
    }
    let title = latex_safe(title);
    let author = latex_safe(author);
    for file in &template.files {
        let relative = file.path.as_str();
        let components: Vec<&str> = relative.split('/').filter(|c| !c.is_empty()).collect();
        if components.is_empty() || components.contains(&"..") || relative.starts_with('/') {
            continue;
        }
        let mut path = folder.to_path_buf();
        for c in &components {
            path.push(c);
        }
        let contents = substitute(&file.contents, &title, &author, date);
        let result = (|| -> Result<(), String> {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            write_atomic(&path, contents.as_bytes()).map_err(|e| e.to_string())
        })();
        result.map_err(|e| format!("“{relative}” could not be written: {e}"))?;
    }
    Ok(())
}

/// Creates `<parent_dir>/<folder name>` from a template (the `create_project` command).
///
/// * `name`: folder name (sanitized; "Untitled Project" when empty after sanitizing);
/// * `title`: LaTeX title (defaults to `name` when `None`);
/// * `author`: author (empty when `None`).
pub fn create_project(
    parent_dir: &str,
    name: &str,
    template_id: &str,
    title: Option<&str>,
    author: Option<&str>,
) -> Result<CreatedProject, String> {
    let template = find_template(template_id).ok_or_else(|| format!("Unknown template “{template_id}”."))?;
    let folder_name = suggested_folder_name(name);
    let folder: PathBuf = Path::new(parent_dir).join(&folder_name);
    let title = title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or(name.trim());
    let author = author.map(str::trim).unwrap_or("");
    write_template(template_id, title, author, &folder, &today_iso())?;
    let root_file = {
        let mut p = folder.clone();
        for c in template.root_file.split('/').filter(|c| !c.is_empty()) {
            p.push(c);
        }
        p
    };
    Ok(CreatedProject {
        root: folder.to_string_lossy().into_owned(),
        name: folder_name,
        root_file: root_file.to_string_lossy().into_owned(),
        engine: template.engine.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gabetex-tpl-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_all_six_templates() {
        let all = list_templates();
        let ids: Vec<&str> = all.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["article", "article-sections", "thesis", "beamer", "math-notes", "problem-set"]);
        assert!(all.iter().all(|t| t.files.contains(&t.root_file)));
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso_date(0), "1970-01-01");
        assert_eq!(iso_date(951_782_400), "2000-02-29");
        assert_eq!(iso_date(1_709_164_799), "2024-02-28");
        assert_eq!(iso_date(1_709_164_800), "2024-02-29");
        assert_eq!(iso_date(-1), "1969-12-31");
        assert_eq!(iso_date(4_102_444_800), "2100-01-01");
        assert_eq!(today_iso().len(), 10);
    }

    #[test]
    fn latex_safe_escapes_plain_text_only() {
        assert_eq!(latex_safe("Notes on R & D"), "Notes on R \\& D");
        assert_eq!(latex_safe("100% #1 a_b {x} ~ ^"), "100\\% \\#1 a\\_b \\{x\\} \\textasciitilde{} \\textasciicircum{}");
        assert_eq!(latex_safe("$x_1$ & more"), "$x_1$ & more");
        assert_eq!(latex_safe("\\emph{A} & B"), "\\emph{A} & B");
        assert_eq!(latex_safe("Ação"), "Ação");
        // A combining mark makes `&` a different grapheme cluster: left alone.
        assert_eq!(latex_safe("&\u{301}"), "&\u{301}");
    }

    #[test]
    fn folder_names() {
        assert_eq!(sanitized("a/b:c\\d"), "a-b-c-d");
        assert_eq!(sanitized("  ..hidden  "), "hidden");
        assert_eq!(sanitized("tab\there"), "tab-here");
        assert_eq!(suggested_folder_name(""), "Untitled Project");
        assert_eq!(suggested_folder_name(" . "), "Untitled Project");
        assert_eq!(suggested_folder_name("My Thesis"), "My Thesis");
    }

    #[test]
    fn substitution_is_sequential() {
        assert_eq!(substitute("{{TITLE}}|{{AUTHOR}}|{{DATE}}", "T", "A", "2026-01-02"), "T|A|2026-01-02");
        // A title containing a later placeholder is expanded by the following replacement (as in Swift).
        assert_eq!(substitute("{{TITLE}}", "x{{AUTHOR}}", "Bob", "d"), "xBob");
        assert_eq!(substitute("{{AUTHOR}}", "{{TITLE}}", "A", "d"), "A");
    }

    #[test]
    fn instantiate_replaces_every_placeholder() {
        for t in list_templates() {
            let files = instantiate(&t.id, "My \\& Title", "Ana", "2026-03-04").unwrap();
            assert_eq!(files.len(), t.files.len());
            for (path, text) in &files {
                assert!(!text.contains("{{TITLE}}") && !text.contains("{{AUTHOR}}") && !text.contains("{{DATE}}"), "{}: {path}", t.id);
            }
        }
        let files = instantiate("article", "Hello", "Ana", "2026-03-04").unwrap();
        let main = &files[0].1;
        assert!(main.contains("\\title{Hello}") && main.contains("\\author{Ana}") && main.contains("\\date{2026-03-04}"), "{main}");
        assert!(instantiate("nope", "", "", "").is_none());
    }

    #[test]
    fn creates_project_with_nested_files() {
        let parent = scratch("create");
        let created = create_project(parent.to_str().unwrap(), "My Thesis", "thesis", Some("Notes on R & D"), Some("Ana")).unwrap();
        assert_eq!(created.name, "My Thesis");
        assert_eq!(created.engine, "pdflatex");
        let main = fs::read_to_string(&created.root_file).unwrap();
        assert!(main.contains("Notes on R \\& D") && main.contains("Ana"));
        assert!(Path::new(&created.root).join("chapters").join("introduction.tex").is_file());
        assert!(Path::new(&created.root).join("figures").join("README.txt").is_file());
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn title_defaults_to_folder_name() {
        let parent = scratch("title");
        let created = create_project(parent.to_str().unwrap(), "Plain", "article", None, None).unwrap();
        assert!(fs::read_to_string(&created.root_file).unwrap().contains("\\title{Plain}"));
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn refuses_non_empty_folder_but_allows_hidden_only() {
        let parent = scratch("refuse");
        let folder = parent.join("P");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(".DS_Store"), "x").unwrap();
        write_template("article", "T", "A", &folder, "2026-01-01").unwrap();
        assert!(folder.join("main.tex").is_file());
        let again = write_template("article", "T2", "A", &folder, "2026-01-01");
        assert!(again.unwrap_err().contains("already exists"));
        assert!(fs::read_to_string(folder.join("main.tex")).unwrap().contains("\\title{T}"), "existing project untouched");
        // A plain file in the way is refused too.
        let file = parent.join("F");
        fs::write(&file, "x").unwrap();
        assert!(write_template("article", "T", "A", &file, "d").is_err());
        // Via the command-level API.
        assert!(create_project(parent.to_str().unwrap(), "P", "article", None, None).is_err());
        assert!(create_project(parent.to_str().unwrap(), "Q", "missing", None, None).is_err());
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn empty_existing_folder_is_reused() {
        let parent = scratch("empty");
        fs::create_dir_all(parent.join("E")).unwrap();
        let created = create_project(parent.to_str().unwrap(), "E", "beamer", Some("Talk"), Some("")).unwrap();
        assert!(Path::new(&created.root_file).is_file());
        let _ = fs::remove_dir_all(&parent);
    }
}

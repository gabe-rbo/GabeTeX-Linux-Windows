use crate::core::completion::{
    complete_at, BibInfo, CompletionItem as CompletionEngineItem, CompletionResult, Kind, LabelInfo, ProjectSymbols, SymbolsRef,
};
use crate::core::compiler::{self, LaTeXCompiler, Purpose};
use crate::core::distribution;
use crate::core::log_parser::ShadowPathMapper;
use crate::core::project_index::{build_project_analysis, build_project_index, parent_dir, standardize, to_native, ProjectOptions};
use crate::core::settings::{self, TextFormat};
use crate::core::synctex::SyncTeX;
use crate::core::watcher::{self, WatchHandle};
use crate::core::{packages, templates};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

pub type CmdResult<T> = Result<T, String>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<FileNode>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub root: String,
    pub name: String,
    pub files: Vec<FileNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub text: String,
    /// "utf-8" or "latin-1"
    pub encoding: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    /// "error" | "warning" | "info"
    pub severity: String,
    pub message: String,
    pub path: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// "latex" | "bibtex" | "chktex" | "reference" | "structure"
    pub source: String,
    /// Multi-line context of a TeX error (the `l.12 ...` lines).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Length in UTF-16 units of the offending span starting at `column`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u32>,
    /// File TeX could not find (e.g. `tikz-cd.sty`); the UI offers a package install for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineItem {
    pub level: i32,
    pub title: String,
    pub path: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub name: String,
    pub path: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BibEntry {
    pub key: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub entry_type: Option<String>,
    pub path: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStats {
    pub words: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIndex {
    pub outline: Vec<OutlineItem>,
    pub labels: Vec<Label>,
    pub citations: Vec<String>,
    pub bib_keys: Vec<BibEntry>,
    pub includes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_file: Option<String>,
    pub warnings: Vec<Diagnostic>,
    pub stats: ProjectStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Snippet syntax, e.g. `\\frac{${1:num}}{${2:den}}`.
    pub insert_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeXDistribution {
    pub name: String,
    pub bin_dir: String,
    pub latexmk: bool,
    pub tlmgr: bool,
    pub synctex: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pdf_path: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub missing_packages: Vec<String>,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncBox {
    pub page: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InverseResult {
    pub path: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub ok: bool,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProject {
    pub path: String,
    pub name: String,
    /// Unix epoch milliseconds.
    pub last_opened: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Payload of the `fs-changed` event (emit with `app.emit("fs-changed", FsChanged { paths })`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsChanged {
    pub paths: Vec<String>,
}

/// Unsaved editor buffers: absolute path -> text.
pub type Overrides = HashMap<String, String>;

/// Full answer of `complete`: the items plus the document range they replace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionResponse {
    /// UTF-16 offset in the document where the replacement starts.
    pub from: usize,
    /// UTF-16 offset where it ends (may lie past the cursor, e.g. over an auto-inserted `}`).
    pub to: usize,
    pub prefix: String,
    /// "command" | "environment" | "endEnvironment" | "label" | "citation" | "package" | "file".
    pub context: String,
    pub in_math: bool,
    pub items: Vec<CompletionItem>,
}

/// Tauri managed state: running file watchers (one per project root) and the shadow path mapper of
/// every PDF a compile produced (needed to translate SyncTeX paths).
#[derive(Default)]
pub struct AppState {
    watchers: Mutex<HashMap<String, WatchHandle>>,
    mappers: Mutex<HashMap<String, ShadowPathMapper>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runs blocking work off the UI thread.
async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> CmdResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| format!("background task failed: {e}"))?
}

fn dir_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn project_info(root: &Path, root_file: Option<String>) -> ProjectInfo {
    let root_str = root.to_string_lossy().into_owned();
    let root_file = root_file.or_else(|| build_project_index(root, &HashMap::new()).root_file);
    ProjectInfo { name: dir_name(root), files: settings::list_files(&root_str), root: root_str, root_file }
}

#[tauri::command]
pub async fn open_project(path: String) -> CmdResult<ProjectInfo> {
    blocking(move || {
        let p = PathBuf::from(&path);
        if p.is_file() {
            let dir = p.parent().map(Path::to_path_buf).ok_or("The file has no folder.")?;
            Ok(project_info(&dir, Some(path)))
        } else if p.is_dir() {
            Ok(project_info(&p, None))
        } else {
            Err(format!("Not found: {path}"))
        }
    })
    .await
}

#[tauri::command]
pub async fn list_files(root: String) -> CmdResult<Vec<FileNode>> {
    blocking(move || Ok(settings::list_files(&root))).await
}

#[tauri::command]
pub async fn read_file(path: String) -> CmdResult<FileContent> {
    blocking(move || {
        let loaded = settings::read_text_file(Path::new(&path), false)?;
        Ok(FileContent { text: loaded.text, encoding: loaded.format.name().to_string() })
    })
    .await
}

#[tauri::command]
pub async fn write_file(path: String, text: String, encoding: String) -> CmdResult<()> {
    blocking(move || {
        let format = TextFormat::parse(&encoding).ok_or_else(|| format!("Unknown encoding “{encoding}”."))?;
        settings::write_text_file(Path::new(&path), &text, format)
    })
    .await
}

#[tauri::command]
pub async fn create_project(parent_dir: String, name: String, template: String) -> CmdResult<ProjectInfo> {
    blocking(move || {
        let author = settings::load_settings().get("author").and_then(|v| v.as_str()).map(str::to_string);
        let created = templates::create_project(&parent_dir, &name, &template, None, author.as_deref())?;
        Ok(project_info(Path::new(&created.root), Some(created.root_file)))
    })
    .await
}

#[tauri::command]
pub async fn project_index(root: String, overrides: Overrides) -> CmdResult<ProjectIndex> {
    blocking(move || Ok(build_project_index(Path::new(&root), &overrides))).await
}

/// Project symbols (labels, bib entries, custom commands, files) for the completion engine.
fn project_symbols(root: &str, path: &str, text: &str) -> ProjectSymbols {
    let mut overrides = HashMap::new();
    overrides.insert(path.to_string(), text.to_string());
    let options = ProjectOptions { current_file: Some(path.to_string()), ..Default::default() };
    let mut symbols = ProjectSymbols::default();
    let Some(analysis) = build_project_analysis(Path::new(root), &overrides, &options) else { return symbols };
    let model = &analysis.model;
    symbols.labels = model
        .labels
        .iter()
        .map(|l| LabelInfo { key: l.key.clone(), path: to_native(&l.file), line: l.line as u32, context: l.context.clone() })
        .collect();
    symbols.bib_entries = model
        .bib_entries
        .iter()
        .map(|b| BibInfo {
            key: b.key.clone(),
            entry_type: Some(b.entry_type.clone()),
            title: b.title.clone(),
            author: b.author.clone(),
            year: b.year.clone(),
        })
        .collect();
    symbols.custom_commands = model
        .custom_commands
        .iter()
        .map(|c| CompletionEngineItem::new(Kind::Command, &c.label, Some(&c.insert_text), c.detail.as_deref(), None))
        .collect();
    symbols.custom_environments = model.custom_environments.clone();
    // Files relative to the directory of the root document, `/`-separated.
    let base = parent_dir(&model.root_file).to_string();
    let prefix = format!("{}/", base.trim_end_matches('/'));
    fn flatten(nodes: &[FileNode], out: &mut Vec<String>) {
        for n in nodes {
            if n.is_dir {
                flatten(n.children.as_deref().unwrap_or(&[]), out);
            } else {
                out.push(n.path.replace('\\', "/"));
            }
        }
    }
    let mut files = Vec::new();
    flatten(&settings::list_files(root), &mut files);
    symbols.project_files = files
        .into_iter()
        .filter_map(|f| {
            let fold = |s: &str| if cfg!(windows) { s.to_lowercase() } else { s.to_string() };
            if fold(&f).starts_with(&fold(&prefix)) {
                f.get(prefix.len()..).map(str::to_string)
            } else {
                None
            }
        })
        .collect();
    symbols
}

fn complete_blocking(root: &str, path: &str, text: &str, line: u32, column: u32) -> Option<CompletionResult> {
    // A first pass without project symbols tells what is being completed; the (costlier) project
    // analysis only runs for contexts that use it.
    let quick = complete_at(text, line, column, &SymbolsRef::default())?;
    if quick.context == "package" {
        return Some(quick);
    }
    let symbols = project_symbols(root, path, text);
    complete_at(text, line, column, &symbols.as_ref())
}

fn to_ipc_item(item: CompletionEngineItem) -> CompletionItem {
    CompletionItem { label: item.label, kind: item.kind.as_str().to_string(), detail: item.detail, insert_text: item.insert_text }
}

#[tauri::command]
pub async fn completions(root: String, path: String, text: String, line: u32, column: u32) -> CmdResult<Vec<CompletionItem>> {
    blocking(move || {
        Ok(complete_blocking(&root, &path, &text, line, column)
            .map(|r| r.items.into_iter().map(to_ipc_item).collect())
            .unwrap_or_default())
    })
    .await
}

/// Like `completions` but also returns the document range to replace (UTF-16 offsets).
#[tauri::command]
pub async fn complete(root: String, path: String, text: String, line: u32, column: u32) -> CmdResult<Option<CompletionResponse>> {
    blocking(move || {
        Ok(complete_blocking(&root, &path, &text, line, column).map(|r| CompletionResponse {
            from: r.from,
            to: r.to,
            prefix: r.prefix,
            context: r.context,
            in_math: r.in_math,
            items: r.items.into_iter().map(to_ipc_item).collect(),
        }))
    })
    .await
}

#[tauri::command]
pub async fn detect_tex() -> CmdResult<Option<TeXDistribution>> {
    blocking(|| Ok(distribution::detect_cached().map(|d| d.to_ipc()))).await
}

/// Compiles live. A newer compile of the same project cancels the one in flight (the compiler's
/// per-project generation counter), and the superseded call returns a cancelled result.
#[tauri::command]
pub async fn compile(
    state: State<'_, AppState>,
    root: String,
    root_file: String,
    engine: Option<String>,
    overrides: Overrides,
) -> CmdResult<CompileResult> {
    let key_root = root.clone();
    let result = blocking(move || compiler::compile_ipc(&root, &root_file, engine.as_deref(), &overrides, Purpose::Live)).await?;
    if let (Some(pdf), Some(dist)) = (&result.pdf_path, distribution::detect_cached()) {
        lock(&state.mappers).insert(pdf.clone(), LaTeXCompiler::new(dist).path_mapper(&key_root));
    }
    Ok(result)
}

/// Stops the compile in flight of a project (also happens implicitly when a newer one starts).
#[tauri::command]
pub fn cancel_compile(root: String) -> CmdResult<()> {
    compiler::cancel_compile(&root, None);
    Ok(())
}

fn mapper_for(state: &AppState, pdf_path: &str, tex_path: Option<&str>) -> ShadowPathMapper {
    if let Some(m) = lock(&state.mappers).get(pdf_path) {
        return m.clone();
    }
    // Unknown PDF (e.g. a previous session): map nothing.
    let dir = tex_path.map(|t| parent_dir(&standardize(t)).to_string()).unwrap_or_default();
    ShadowPathMapper::new(&dir, &dir)
}

#[tauri::command]
pub async fn synctex_forward(
    state: State<'_, AppState>,
    pdf_path: String,
    tex_path: String,
    line: u32,
    column: u32,
) -> CmdResult<Vec<SyncBox>> {
    let mapper = mapper_for(&state, &pdf_path, Some(&tex_path));
    blocking(move || {
        let Some(dist) = distribution::detect_cached() else { return Ok(Vec::new()) };
        Ok(SyncTeX::new(&dist).forward(&pdf_path, &tex_path, line, column, &mapper))
    })
    .await
}

#[tauri::command]
pub async fn synctex_inverse(
    state: State<'_, AppState>,
    pdf_path: String,
    page: u32,
    x: f64,
    y: f64,
) -> CmdResult<Option<InverseResult>> {
    let mapper = mapper_for(&state, &pdf_path, None);
    blocking(move || {
        let Some(dist) = distribution::detect_cached() else { return Ok(None) };
        Ok(SyncTeX::new(&dist).inverse(&pdf_path, page, x, y, &mapper))
    })
    .await
}

#[tauri::command]
pub async fn install_packages(packages: Vec<String>) -> CmdResult<InstallResult> {
    blocking(move || Ok(packages::install_packages(&packages))).await
}

#[tauri::command]
pub fn watch_project(app: AppHandle, state: State<'_, AppState>, root: String) -> CmdResult<()> {
    let emitter = app.clone();
    let handle = watcher::start(&root, move |paths| {
        let _ = emitter.emit("fs-changed", FsChanged { paths });
    })?;
    // Replacing an existing handle stops the old watcher (dropping a handle stops it).
    lock(&state.watchers).insert(root, handle);
    Ok(())
}

#[tauri::command]
pub fn unwatch_project(state: State<'_, AppState>, root: String) -> CmdResult<()> {
    if let Some(handle) = lock(&state.watchers).remove(&root) {
        handle.stop();
    }
    Ok(())
}

#[tauri::command]
pub async fn reveal_in_file_manager(path: String) -> CmdResult<()> {
    blocking(move || settings::reveal_in_file_manager(&path)).await
}

#[tauri::command]
pub async fn load_settings() -> CmdResult<serde_json::Value> {
    blocking(|| Ok(settings::load_settings())).await
}

#[tauri::command]
pub async fn save_settings(settings: serde_json::Value) -> CmdResult<serde_json::Value> {
    blocking(move || settings::save_settings(settings)).await
}

#[tauri::command]
pub async fn recent_projects() -> CmdResult<Vec<RecentProject>> {
    blocking(|| Ok(settings::recent_projects())).await
}

#[tauri::command]
pub async fn add_recent_project(path: String, name: Option<String>) -> CmdResult<Vec<RecentProject>> {
    blocking(move || settings::add_recent_project(&path, name.as_deref())).await
}

#[tauri::command]
pub async fn set_project_label(path: String, label: Option<String>) -> CmdResult<Vec<RecentProject>> {
    blocking(move || settings::set_project_label(&path, label.as_deref())).await
}

/// The bytes of a PDF for the preview, sent raw (no JSON array) so large documents stay cheap to transfer.
#[tauri::command]
pub async fn read_pdf(path: String) -> CmdResult<tauri::ipc::Response> {
    blocking(move || {
        let bytes = std::fs::read(&path).map_err(|e| format!("could not read {path}: {e}"))?;
        Ok(tauri::ipc::Response::new(bytes))
    })
    .await
}

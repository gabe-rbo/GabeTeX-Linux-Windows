use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type CmdResult<T> = Result<T, String>;

fn todo<T>(name: &str) -> CmdResult<T> {
    Err(format!("{name}: not implemented yet"))
}

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

#[tauri::command]
pub fn open_project(path: String) -> CmdResult<ProjectInfo> {
    let _ = &path;
    todo("open_project")
}

#[tauri::command]
pub fn list_files(root: String) -> CmdResult<Vec<FileNode>> {
    let _ = &root;
    todo("list_files")
}

#[tauri::command]
pub fn read_file(path: String) -> CmdResult<FileContent> {
    let _ = &path;
    todo("read_file")
}

#[tauri::command]
pub fn write_file(path: String, text: String, encoding: String) -> CmdResult<()> {
    let _ = &path;
    let _ = &text;
    let _ = &encoding;
    todo("write_file")
}

#[tauri::command]
pub fn create_project(parent_dir: String, name: String, template: String) -> CmdResult<ProjectInfo> {
    let _ = &parent_dir;
    let _ = &name;
    let _ = &template;
    todo("create_project")
}

#[tauri::command]
pub fn project_index(root: String, overrides: Overrides) -> CmdResult<ProjectIndex> {
    let _ = &root;
    let _ = &overrides;
    todo("project_index")
}

#[tauri::command]
pub fn completions(root: String, path: String, text: String, line: u32, column: u32) -> CmdResult<Vec<CompletionItem>> {
    let _ = &root;
    let _ = &path;
    let _ = &text;
    let _ = &line;
    let _ = &column;
    todo("completions")
}

#[tauri::command]
pub fn detect_tex() -> CmdResult<Option<TeXDistribution>> {
    todo("detect_tex")
}

#[tauri::command]
pub fn compile(root: String, root_file: String, engine: Option<String>, overrides: Overrides) -> CmdResult<CompileResult> {
    let _ = &root;
    let _ = &root_file;
    let _ = &engine;
    let _ = &overrides;
    todo("compile")
}

#[tauri::command]
pub fn synctex_forward(pdf_path: String, tex_path: String, line: u32, column: u32) -> CmdResult<Vec<SyncBox>> {
    let _ = &pdf_path;
    let _ = &tex_path;
    let _ = &line;
    let _ = &column;
    todo("synctex_forward")
}

#[tauri::command]
pub fn synctex_inverse(pdf_path: String, page: u32, x: f64, y: f64) -> CmdResult<Option<InverseResult>> {
    let _ = &pdf_path;
    let _ = &page;
    let _ = &x;
    let _ = &y;
    todo("synctex_inverse")
}

#[tauri::command]
pub fn install_packages(packages: Vec<String>) -> CmdResult<InstallResult> {
    let _ = &packages;
    todo("install_packages")
}

#[tauri::command]
pub fn watch_project(root: String) -> CmdResult<()> {
    let _ = &root;
    todo("watch_project")
}

#[tauri::command]
pub fn unwatch_project(root: String) -> CmdResult<()> {
    let _ = &root;
    todo("unwatch_project")
}

#[tauri::command]
pub fn reveal_in_file_manager(path: String) -> CmdResult<()> {
    let _ = &path;
    todo("reveal_in_file_manager")
}

#[tauri::command]
pub fn load_settings() -> CmdResult<serde_json::Value> {
    todo("load_settings")
}

#[tauri::command]
pub fn save_settings(settings: serde_json::Value) -> CmdResult<serde_json::Value> {
    let _ = &settings;
    todo("save_settings")
}

#[tauri::command]
pub fn recent_projects() -> CmdResult<Vec<RecentProject>> {
    todo("recent_projects")
}

#[tauri::command]
pub fn add_recent_project(path: String, name: Option<String>) -> CmdResult<Vec<RecentProject>> {
    let _ = &path;
    let _ = &name;
    todo("add_recent_project")
}

#[tauri::command]
pub fn set_project_label(path: String, label: Option<String>) -> CmdResult<Vec<RecentProject>> {
    let _ = &path;
    let _ = &label;
    todo("set_project_label")
}


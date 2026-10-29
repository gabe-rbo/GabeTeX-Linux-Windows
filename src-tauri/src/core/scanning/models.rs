//! Models of `Outline.swift` and `Symbols.swift` (Diagnostic lives in `log_parser`).

pub use super::syntax::OutlineKind;
use crate::commands::CompletionItem;
use std::ops::Range;
use std::sync::Arc;

/// Absolute, standardized path of a source file (`/`-separated internally, see `project_index::standardize`).
pub type FilePath = Arc<str>;

/// A structural element found in a source file: a sectioning command, a beamer frame, or a label.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OutlineEntry {
    pub kind: OutlineKind,
    /// Heading title (LaTeX source of the argument, lightly cleaned) or the label key.
    pub title: String,
    pub is_starred: bool,
    pub file: FilePath,
    /// 1-based line of the command.
    pub line: usize,
    /// UTF-16 offset of the command's backslash in its file.
    pub offset: usize,
}

/// Tree node of the outline. `children == None` for leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineNode {
    pub entry: OutlineEntry,
    pub children: Option<Vec<OutlineNode>>,
}

impl OutlineNode {
    /// Stable across re-scans as long as the entry does not move.
    pub fn id(&self) -> String {
        format!("{}#{}#{}#{}", self.entry.file, self.entry.line, self.entry.kind.raw(), self.entry.title)
    }
}

/// A `\label{key}` definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LabelDefinition {
    pub key: String,
    pub file: FilePath,
    /// 1-based line.
    pub line: usize,
    /// Short human context, e.g. the enclosing environment or the nearest heading's title.
    pub context: Option<String>,
}

/// An entry of a `.bib` file (Swift `BibEntry`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BibRecord {
    pub key: String,
    /// "article", "book", ... (lowercased, without "@")
    pub entry_type: String,
    pub title: Option<String>,
    pub author: Option<String>,
    pub year: Option<String>,
    pub file: FilePath,
    /// 1-based line of the "@type{".
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IncludeKind {
    /// `\input{file}`: resolved relative to the root file's directory
    Input,
    Include,
    /// relative to the *including* file's directory
    Subfile,
    /// `\import{dir}{file}`
    ImportFile,
    Subimport,
    Bibliography,
    Addbibresource,
    Includegraphics,
    Includepdf,
    Lstinputlisting,
    Inputminted,
    Usepackage,
    Documentclass,
}

impl IncludeKind {
    /// Swift raw value.
    pub fn raw(self) -> &'static str {
        match self {
            IncludeKind::Input => "input",
            IncludeKind::Include => "include",
            IncludeKind::Subfile => "subfile",
            IncludeKind::ImportFile => "importFile",
            IncludeKind::Subimport => "subimport",
            IncludeKind::Bibliography => "bibliography",
            IncludeKind::Addbibresource => "addbibresource",
            IncludeKind::Includegraphics => "includegraphics",
            IncludeKind::Includepdf => "includepdf",
            IncludeKind::Lstinputlisting => "lstinputlisting",
            IncludeKind::Inputminted => "inputminted",
            IncludeKind::Usepackage => "usepackage",
            IncludeKind::Documentclass => "documentclass",
        }
    }

    /// Kinds whose target is a LaTeX source that the outline follows.
    pub fn follows_include(self) -> bool {
        matches!(
            self,
            IncludeKind::Input | IncludeKind::Include | IncludeKind::Subfile | IncludeKind::ImportFile | IncludeKind::Subimport
        )
    }
}

/// A reference from one source file to another file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IncludeReference {
    pub kind: IncludeKind,
    /// The argument exactly as written (one item for comma lists), e.g. "sections/intro".
    pub argument: String,
    /// For `\import` / `\subimport`: the directory argument.
    pub directory: Option<String>,
    /// 1-based line.
    pub line: usize,
    /// UTF-16 range of `argument` in the source text.
    pub range: Range<usize>,
}

/// TeXShop-style magic comments found in the first lines of a file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct MagicComments {
    pub root: Option<String>,
    pub program: Option<String>,
    pub spellcheck: Option<String>,
    /// `% !BIB program = biber` / `% !TEX bib = biber`
    pub bib_program: Option<String>,
}

/// Everything project-specific the completion engine needs.
#[derive(Debug, Clone, Default)]
pub struct ProjectSymbols {
    pub labels: Vec<LabelDefinition>,
    pub bib_entries: Vec<BibRecord>,
    pub custom_commands: Vec<CompletionItem>,
    pub custom_environments: Vec<String>,
    /// Project files, relative to the root file's directory, using "/".
    pub project_files: Vec<String>,
}

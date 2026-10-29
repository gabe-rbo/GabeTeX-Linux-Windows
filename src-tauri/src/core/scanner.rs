//! LaTeX source scanning: a faithful port of `Sources/TeXCore/Scanning/*`, `Text/LineIndex.swift`,
//! `Models/{Outline,Diagnostic,Symbols}.swift`, `Syntax/LaTeXSyntax.swift` and `Statistics/*`.
//!
//! Everything works on UTF-16 buffers (`&[u16]`), so offsets, columns and lengths are the units CodeMirror and
//! the Swift app use. The sub-modules live under `core/scanning/`:
//!
//! | module | Swift |
//! |---|---|
//! | [`syntax`] | `LaTeXSyntax.swift` (helpers, verbatim helpers, command classification) |
//! | [`masked`] | `MaskedSource.swift`, `LineIndex.swift` |
//! | [`arguments`] | `ArgumentParser.swift` (`TeXArguments`) |
//! | [`definitions`] | `ScannerDefinitions.swift` |
//! | [`latex_scanner`] | `LaTeXScanner.swift` |
//! | [`bib`] | `BibParser.swift` |
//! | [`references`] | `ReferenceScanner.swift`, `ReferenceLinter.swift` |
//! | [`walker`] | `StructureWalker.swift`, `StructureLinter.swift` |
//! | [`statistics`] | `Statistics/*` (word / formula counts, page attribution) |
//! | [`models`] | `Outline.swift`, `Symbols.swift` |
//!
//! The cross-file index (`ProjectIndex.swift`) is in [`crate::core::project_index`], the highlighter's
//! tokenizer in [`crate::core::tokenizer`].

#[path = "scanning/syntax.rs"]
pub mod syntax;
#[path = "scanning/masked.rs"]
pub mod masked;
#[path = "scanning/arguments.rs"]
pub mod arguments;
#[path = "scanning/models.rs"]
pub mod models;
#[path = "scanning/definitions.rs"]
pub mod definitions;
#[path = "scanning/latex_scanner.rs"]
pub mod latex_scanner;
#[path = "scanning/bib.rs"]
pub mod bib;
#[path = "scanning/references.rs"]
pub mod references;
#[path = "scanning/walker.rs"]
pub mod walker;
#[path = "scanning/statistics.rs"]
pub mod statistics;

pub use latex_scanner::{magic_comments, FileScan, ScanParts};
pub use masked::{LineIndex, MaskedSource, SourcePosition};
pub use models::*;

#[cfg(test)]
#[path = "scanning/tests_common.rs"]
pub(crate) mod tests_common;
#[cfg(test)]
#[path = "scanning/tests_scanner.rs"]
mod tests_scanner;
#[cfg(test)]
#[path = "scanning/tests_index.rs"]
mod tests_index;
#[cfg(test)]
#[path = "scanning/tests_linter.rs"]
mod tests_linter;
#[cfg(test)]
#[path = "scanning/tests_refs.rs"]
mod tests_refs;
#[cfg(test)]
#[path = "scanning/tests_stats.rs"]
mod tests_stats;
#[cfg(test)]
#[path = "scanning/tests_disk.rs"]
mod tests_disk;

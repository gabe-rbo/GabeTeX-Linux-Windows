//! Rust port of TeXCore / TeXToolchain. Each submodule is owned by the core agent; keep the
//! command layer (`commands.rs`) thin and put logic here so it is unit-testable.
pub mod compiler;
pub mod completion;
pub mod distribution;
pub mod log_parser;
pub mod packages;
pub mod project_index;
pub mod scanner;
pub mod settings;
pub mod synctex;
pub mod templates;
pub mod tokenizer;
pub mod watcher;

//! Library surface of the Pkl language server. The `pkl-lsp` binary is a
//! thin wrapper around this crate so integration tests can exercise the
//! server end-to-end over an in-memory duplex.

pub mod backend;
pub mod capabilities;
pub mod code_actions;
pub mod completion;
pub mod config;
pub mod document;
pub mod folding;
pub mod formatting;
pub mod goto;
pub mod highlights;
pub mod hover;
pub mod import_paths;
pub mod inlay_hints;
pub mod references;
pub mod rename;
pub mod selection_range;
pub mod semantic_tokens;
pub mod signature_help;
pub mod symbols;
pub mod uri;
pub mod workspace_symbols;

pub use backend::Backend;

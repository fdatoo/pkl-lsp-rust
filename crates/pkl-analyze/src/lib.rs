//! Semantic analysis for Pkl.
//!
//! The analyzer is intentionally split into small, composable pieces:
//!
//! * [`symbols::Symbol`] / [`symbols::SymbolTable`] — every declaration the
//!   analyzer knows about, addressed by [`symbols::SymbolId`].
//! * [`scopes::Scope`] / [`scopes::ScopeArena`] — lexical scope tree.
//! * [`resolver::Resolution`] — output of one analysis pass over a file:
//!   the symbol table, every recorded reference site, and a fast
//!   span-keyed lookup.
//! * [`hover::hover_markdown`] — render a hover string for an LSP client.
//!
//! Type checking and cross-file module resolution still live in the future.

pub mod hover;
pub mod infer;
pub mod loader;
pub mod module_graph;
pub mod pretty;
pub mod resolver;
pub mod scopes;
pub mod subtyping;
pub mod symbols;
pub mod types;
pub mod workspace_files;

use pkl_syntax::cst::{AstNode, Module};
use pkl_syntax::{SyntaxDiagnostic, SyntaxNode};

pub use infer::{infer_module, Inference, MemberRef};
#[cfg(feature = "remote")]
pub use loader::RemoteLoader;
pub use loader::{
    ChainedLoader, FsLoader, FsLoaderConfig, LoadError, ModuleLoader, ModuleUri, StdlibLoader,
};
pub use module_graph::{ImportError, ModuleEntry, ModuleGraph};
pub use resolver::{resolve_module, ImportInfo, Reference, Resolution};
pub use symbols::{Origin, Symbol, SymbolId, SymbolKind, SymbolTable};
pub use types::Ty;
pub use workspace_files::{ImportCompletion, ImportCompletionKind, WorkspaceIndex};

/// Convenience: every output the LSP needs from a single file.
pub struct Analysis {
    pub resolution: Resolution,
    pub inference: Inference,
    pub diagnostics: Vec<SyntaxDiagnostic>,
}

/// Run the resolver + inferrer over `syntax` (the lossless tree's root)
/// and bundle the outputs with the parser's syntax diagnostics.
pub fn analyze(syntax: &SyntaxNode, syntax_diagnostics: Vec<SyntaxDiagnostic>) -> Analysis {
    let module = Module::cast(syntax.clone()).expect("syntax root must be a Module node");
    let resolution = resolve_module(&module);
    let inference = infer_module(&module, &resolution);
    let mut diagnostics = syntax_diagnostics;
    diagnostics.extend(resolution.diagnostics.iter().cloned());
    diagnostics.extend(inference.diagnostics.iter().cloned());
    Analysis {
        resolution,
        inference,
        diagnostics,
    }
}

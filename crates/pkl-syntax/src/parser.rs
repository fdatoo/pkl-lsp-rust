//! Public parse entry point.
//!
//! Wraps the lossless parser ([`green::parse_green`]) and exposes its
//! output to downstream consumers. Every byte of the source is captured
//! by the green tree; the typed views in [`crate::cst`] walk that tree
//! on demand.

use rowan::GreenNode;

use crate::diagnostic::SyntaxDiagnostic;
use crate::green::parse_green;
use crate::syntax::SyntaxNode;

/// Result of a parse.
///
/// Carries two artefacts produced from the same source pass:
///
/// * `green` — the lossless rowan tree (immutable, `Send + Sync`),
///   where every byte of the original source (trivia included) is
///   represented. Use [`ParseResult::syntax`] to obtain a thread-local
///   red-tree root for walking.
/// * `diagnostics` — parser-time errors.
pub struct ParseResult {
    pub green: GreenNode,
    pub diagnostics: Vec<SyntaxDiagnostic>,
}

impl ParseResult {
    /// Build a fresh red-tree root over the immutable green tree.
    ///
    /// Rowan red nodes are reference-counted (`Rc` internally) so they
    /// are `!Send` / `!Sync`. To allow `ParseResult` itself to live in
    /// shared LSP state we keep only the green tree on the struct and
    /// hand out fresh red roots on demand. This is cheap — the only
    /// allocation is the root's own NodeData.
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }
}

pub fn parse(src: &str) -> ParseResult {
    let lossless = parse_green(src);
    ParseResult {
        green: lossless.syntax.green().into_owned(),
        diagnostics: lossless.diagnostics,
    }
}

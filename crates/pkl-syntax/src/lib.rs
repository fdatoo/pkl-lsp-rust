//! Lexer, parser, and lossless syntax tree for the Pkl configuration
//! language.
//!
//! The crate is layered:
//!
//! ```text
//! span ──► token ──► lexer ──► green ──► cst (typed view)
//!                            └──► diagnostic
//! ```
//!
//! Public entry points:
//!
//! * [`tokenize`] for raw token streams (preserves trivia).
//! * [`parse`] for a full lossless tree plus syntax diagnostics. The
//!   resulting [`ParseResult`] holds the green tree; call
//!   [`ParseResult::syntax`] to get a fresh red root for walking.
//! * [`cst::Module::cast`] (and friends) to wrap a [`SyntaxNode`] in a
//!   typed view.

pub mod cst;
pub mod diagnostic;
pub mod format;
pub mod green;
pub mod kind;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod syntax;
pub mod token;

pub use syntax::{PklLang, SyntaxElement, SyntaxElementRef, SyntaxNode, SyntaxToken};

pub use cst::AstNode;
pub use diagnostic::{Severity, SyntaxDiagnostic};
pub use green::{parse_green, LosslessParse};
pub use kind::SyntaxKind;
pub use lexer::tokenize;
pub use parser::{parse, ParseResult};
pub use span::Span;
pub use token::Token;

/// Walk a typed [`cst::Module`] and call `visit` for every property/method
/// declaration that has a name. A tiny utility used by the LSP layer to
/// produce `textDocument/documentSymbol` responses without dragging a
/// full visitor pattern into the analyzer.
pub fn walk_named_items(module: &cst::Module, mut visit: impl FnMut(&str, Span, NamedItemKind)) {
    for item in module.items() {
        match item {
            cst::Item::Class(c) => {
                if let Some(name_tok) = c.name() {
                    visit(
                        &cst::ident_text(&name_tok),
                        cst::significant_span(c.syntax()),
                        NamedItemKind::Class,
                    );
                }
                if let Some(body) = c.body() {
                    for member in body.members() {
                        match member {
                            cst::ClassMember::Property(p) => {
                                if let Some(name_tok) = p.name() {
                                    visit(
                                        &cst::ident_text(&name_tok),
                                        cst::significant_span(p.syntax()),
                                        NamedItemKind::Property,
                                    );
                                }
                            }
                            cst::ClassMember::Method(m) => {
                                if let Some(name_tok) = m.name() {
                                    visit(
                                        &cst::ident_text(&name_tok),
                                        cst::significant_span(m.syntax()),
                                        NamedItemKind::Method,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            cst::Item::TypeAlias(t) => {
                if let Some(name_tok) = t.name() {
                    visit(
                        &cst::ident_text(&name_tok),
                        cst::significant_span(t.syntax()),
                        NamedItemKind::TypeAlias,
                    );
                }
            }
            cst::Item::Property(p) => {
                if let Some(name_tok) = p.name() {
                    visit(
                        &cst::ident_text(&name_tok),
                        cst::significant_span(p.syntax()),
                        NamedItemKind::Property,
                    );
                }
            }
            cst::Item::Method(m) => {
                if let Some(name_tok) = m.name() {
                    visit(
                        &cst::ident_text(&name_tok),
                        cst::significant_span(m.syntax()),
                        NamedItemKind::Method,
                    );
                }
            }
            cst::Item::Error(_) => {}
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NamedItemKind {
    Class,
    TypeAlias,
    Property,
    Method,
}

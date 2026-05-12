//! Lexer, parser, and AST for the Pkl configuration language.
//!
//! The crate is deliberately layered:
//!
//! ```text
//! span ──► token ──► lexer ──► parser ──► ast
//!                                     └─► diagnostic
//! ```
//!
//! Public entry points:
//!
//! * [`tokenize`] for raw token streams (preserves trivia).
//! * [`parse`] for a full [`ast::Module`] plus syntax diagnostics.

pub mod ast;
pub mod diagnostic;
pub mod format;
pub mod kind;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod token;

pub use diagnostic::{Severity, SyntaxDiagnostic};
pub use kind::SyntaxKind;
pub use lexer::tokenize;
pub use parser::{parse, ParseResult};
pub use span::Span;
pub use token::Token;

/// Walk the AST and call `visit` for every property/method declaration that
/// has a name. This is a tiny utility used by the LSP layer to produce
/// `textDocument/documentSymbol` responses without dragging a full visitor
/// pattern into the analyzer.
pub fn walk_named_items<'a>(
    module: &'a ast::Module,
    mut visit: impl FnMut(&'a str, Span, NamedItemKind),
) {
    for item in &module.items {
        match item {
            ast::Item::Class(c) => {
                visit(&c.name.name, c.span, NamedItemKind::Class);
                if let Some(body) = &c.body {
                    for member in &body.members {
                        match member {
                            ast::ClassMember::Property(p) => {
                                visit(&p.name.name, p.span, NamedItemKind::Property);
                            }
                            ast::ClassMember::Method(m) => {
                                visit(&m.name.name, m.span, NamedItemKind::Method);
                            }
                        }
                    }
                }
            }
            ast::Item::TypeAlias(t) => visit(&t.name.name, t.span, NamedItemKind::TypeAlias),
            ast::Item::Property(p) => visit(&p.name.name, p.span, NamedItemKind::Property),
            ast::Item::Method(m) => visit(&m.name.name, m.span, NamedItemKind::Method),
            ast::Item::Error(_) => {}
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

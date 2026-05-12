//! Raw token stream produced by the lexer.

use crate::kind::SyntaxKind;
use crate::span::Span;

/// A single token. `text` borrows from the source so the lexer does not
/// allocate per-token storage.
#[derive(Clone, Debug)]
pub struct Token<'src> {
    pub kind: SyntaxKind,
    pub span: Span,
    pub text: &'src str,
}

impl<'src> Token<'src> {
    #[inline]
    pub fn new(kind: SyntaxKind, span: Span, text: &'src str) -> Self {
        Self { kind, span, text }
    }

    #[inline]
    pub fn is_trivia(&self) -> bool {
        self.kind.is_trivia()
    }
}

/// Owned variant used when we need to store a token outside the lifetime of
/// the source buffer (e.g. attaching diagnostics that outlive a parse).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedToken {
    pub kind: SyntaxKind,
    pub span: Span,
    pub text: String,
}

impl<'src> From<&Token<'src>> for OwnedToken {
    fn from(t: &Token<'src>) -> Self {
        OwnedToken {
            kind: t.kind,
            span: t.span,
            text: t.text.to_owned(),
        }
    }
}

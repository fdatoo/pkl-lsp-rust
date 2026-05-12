//! Parser/lexer diagnostics.

use crate::span::Span;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxDiagnostic {
    pub span: Span,
    pub severity: Severity,
    pub message: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl SyntaxDiagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            severity: Severity::Error,
            message: message.into(),
        }
    }
}

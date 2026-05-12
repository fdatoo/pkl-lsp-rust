//! `textDocument/foldingRange` handler.
//!
//! Walks the parsed AST and emits one fold per multi-line span we know
//! to be a sensible folding target: class bodies, object bodies, method
//! bodies, multi-line literals, block comments.

use pkl_syntax::ast::*;
use pkl_syntax::span::Span;
use pkl_syntax::SyntaxKind;
use tower_lsp::lsp_types::{FoldingRange, FoldingRangeKind, Position};

use crate::document::{byte_to_position, Document};

pub fn folding_ranges(doc: &Document) -> Vec<FoldingRange> {
    let mut out = Vec::new();
    push_module_ranges(doc, &doc.analysis.resolution.symbols, &mut out);
    push_ast_ranges(doc, &doc.parsed.module, &mut out);
    push_comment_ranges(doc, &mut out);
    out
}

fn push_module_ranges(
    _doc: &Document,
    _symbols: &pkl_analyze::SymbolTable,
    _out: &mut Vec<FoldingRange>,
) {
    // Placeholder for future: collapse the `import` block at module
    // header. Skipped for now to keep the output noise-free.
}

fn push_ast_ranges(doc: &Document, module: &Module, out: &mut Vec<FoldingRange>) {
    for item in &module.items {
        match item {
            Item::Class(c) => {
                if let Some(body) = &c.body {
                    add_range(doc, body.span, FoldingRangeKind::Region, out);
                    for m in &body.members {
                        match m {
                            ClassMember::Property(p) => push_property(doc, p, out),
                            ClassMember::Method(m) => push_method(doc, m, out),
                        }
                    }
                }
            }
            Item::Property(p) => push_property(doc, p, out),
            Item::Method(m) => push_method(doc, m, out),
            _ => {}
        }
    }
}

fn push_property(doc: &Document, p: &PropertyDecl, out: &mut Vec<FoldingRange>) {
    if let Some(PropertyValue::ObjectBody(body)) = &p.value {
        add_range(doc, body.span, FoldingRangeKind::Region, out);
        push_object_body(doc, body, out);
    }
}

fn push_method(doc: &Document, m: &MethodDecl, out: &mut Vec<FoldingRange>) {
    if let Some(Expr::New { body, .. }) = &m.body {
        add_range(doc, body.span, FoldingRangeKind::Region, out);
        push_object_body(doc, body, out);
    }
}

fn push_object_body(doc: &Document, body: &ObjectBody, out: &mut Vec<FoldingRange>) {
    for member in &body.members {
        match member {
            ObjectMember::Property(p) => push_property(doc, p, out),
            ObjectMember::Method(m) => push_method(doc, m, out),
            ObjectMember::When {
                then_body,
                else_body,
                ..
            } => {
                add_range(doc, then_body.span, FoldingRangeKind::Region, out);
                push_object_body(doc, then_body, out);
                if let Some(b) = else_body {
                    add_range(doc, b.span, FoldingRangeKind::Region, out);
                    push_object_body(doc, b, out);
                }
            }
            ObjectMember::For { body, .. } => {
                add_range(doc, body.span, FoldingRangeKind::Region, out);
                push_object_body(doc, body, out);
            }
            _ => {}
        }
    }
}

fn push_comment_ranges(doc: &Document, out: &mut Vec<FoldingRange>) {
    let text = doc.rope.to_string();
    for token in pkl_syntax::tokenize(&text) {
        match token.kind {
            SyntaxKind::BlockComment | SyntaxKind::MultilineString => {
                add_range(doc, token.span, FoldingRangeKind::Comment, out);
            }
            _ => {}
        }
    }
}

fn add_range(doc: &Document, span: Span, kind: FoldingRangeKind, out: &mut Vec<FoldingRange>) {
    let start: Position = byte_to_position(&doc.rope, span.start as usize);
    let end: Position = byte_to_position(&doc.rope, span.end as usize);
    if end.line <= start.line {
        return;
    }
    out.push(FoldingRange {
        start_line: start.line,
        start_character: Some(start.character),
        end_line: end.line,
        end_character: Some(end.character),
        kind: Some(kind),
        collapsed_text: None,
    });
}

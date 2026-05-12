//! `textDocument/selectionRange` handler.
//!
//! For each requested position, walks the AST top-down to find the
//! smallest node containing the cursor and produces a chain of widening
//! ranges via the LSP `parent` linking.

use pkl_syntax::ast::*;
use pkl_syntax::span::Span;
use tower_lsp::lsp_types::{Position, SelectionRange};

use crate::document::Document;

pub fn selection_ranges(doc: &Document, positions: Vec<Position>) -> Vec<SelectionRange> {
    positions
        .into_iter()
        .map(|p| selection_range_at(doc, p))
        .collect()
}

fn selection_range_at(doc: &Document, position: Position) -> SelectionRange {
    let offset = doc.position_to_offset(position);
    // Build a list of spans containing `offset`, smallest first.
    let mut spans: Vec<Span> = Vec::new();
    collect_spans_containing(&doc.parsed.module, offset, &mut spans);

    // Always end with the full module span as the outermost fallback.
    spans.push(Span::new(0, doc.rope.len_bytes() as u32));

    // Build SelectionRange chain from outermost → innermost so children
    // reference their parent.
    let mut chain: Option<SelectionRange> = None;
    for span in spans.iter().rev() {
        let r = doc.span_to_range(*span);
        chain = Some(SelectionRange {
            range: r,
            parent: chain.map(Box::new),
        });
    }
    chain.unwrap_or_else(|| SelectionRange {
        range: doc.span_to_range(Span::new(0, 0)),
        parent: None,
    })
}

fn collect_spans_containing(module: &Module, offset: u32, out: &mut Vec<Span>) {
    for item in &module.items {
        if !item.span().contains(offset) {
            continue;
        }
        out.push(item.span());
        match item {
            Item::Class(c) => {
                if let Some(body) = &c.body {
                    if body.span.contains(offset) {
                        out.push(body.span);
                        for m in &body.members {
                            if m.span().contains(offset) {
                                out.push(m.span());
                                match m {
                                    ClassMember::Property(p) => walk_property(p, offset, out),
                                    ClassMember::Method(m) => walk_method(m, offset, out),
                                }
                            }
                        }
                    }
                }
            }
            Item::Property(p) => walk_property(p, offset, out),
            Item::Method(m) => walk_method(m, offset, out),
            _ => {}
        }
    }
    out.reverse();
}

fn walk_property(p: &PropertyDecl, offset: u32, out: &mut Vec<Span>) {
    if let Some(value) = &p.value {
        match value {
            PropertyValue::Expr(e) => walk_expr(e, offset, out),
            PropertyValue::ObjectBody(body) => walk_object_body(body, offset, out),
        }
    }
}

fn walk_method(m: &MethodDecl, offset: u32, out: &mut Vec<Span>) {
    if let Some(body) = &m.body {
        walk_expr(body, offset, out);
    }
}

fn walk_object_body(body: &ObjectBody, offset: u32, out: &mut Vec<Span>) {
    if !body.span.contains(offset) {
        return;
    }
    out.push(body.span);
    for member in &body.members {
        if member.span().contains(offset) {
            out.push(member.span());
            match member {
                ObjectMember::Property(p) => walk_property(p, offset, out),
                ObjectMember::Method(m) => walk_method(m, offset, out),
                ObjectMember::Element(e) => walk_expr(e, offset, out),
                ObjectMember::Entry { value, .. } => match value {
                    PropertyValue::Expr(e) => walk_expr(e, offset, out),
                    PropertyValue::ObjectBody(b) => walk_object_body(b, offset, out),
                },
                ObjectMember::When {
                    then_body,
                    else_body,
                    ..
                } => {
                    walk_object_body(then_body, offset, out);
                    if let Some(b) = else_body {
                        walk_object_body(b, offset, out);
                    }
                }
                ObjectMember::For { body, .. } => walk_object_body(body, offset, out),
                ObjectMember::Spread { expr, .. } => walk_expr(expr, offset, out),
            }
        }
    }
}

fn walk_expr(expr: &Expr, offset: u32, out: &mut Vec<Span>) {
    if !expr.span().contains(offset) {
        return;
    }
    out.push(expr.span());
    match expr {
        Expr::Paren { inner, .. } | Expr::NonNull { operand: inner, .. } => {
            walk_expr(inner, offset, out)
        }
        Expr::Unary { operand, .. } => walk_expr(operand, offset, out),
        Expr::Binary { lhs, rhs, .. } => {
            walk_expr(lhs, offset, out);
            walk_expr(rhs, offset, out);
        }
        Expr::TypeCheck { operand, .. } | Expr::TypeCast { operand, .. } => {
            walk_expr(operand, offset, out)
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            walk_expr(cond, offset, out);
            walk_expr(then_branch, offset, out);
            walk_expr(else_branch, offset, out);
        }
        Expr::Let { value, body, .. } => {
            walk_expr(value, offset, out);
            walk_expr(body, offset, out);
        }
        Expr::Lambda { body, .. } => walk_expr(body, offset, out),
        Expr::Call { callee, args, .. } => {
            walk_expr(callee, offset, out);
            for a in args {
                walk_expr(a, offset, out);
            }
        }
        Expr::Index {
            receiver, index, ..
        } => {
            walk_expr(receiver, offset, out);
            walk_expr(index, offset, out);
        }
        Expr::Member { receiver, .. } => walk_expr(receiver, offset, out),
        Expr::New { body, .. } => walk_object_body(body, offset, out),
        Expr::AmendsObject { base, body, .. } => {
            walk_expr(base, offset, out);
            walk_object_body(body, offset, out);
        }
        Expr::Throw { argument, .. }
        | Expr::Trace { argument, .. }
        | Expr::Read { argument, .. } => walk_expr(argument, offset, out),
        _ => {}
    }
}

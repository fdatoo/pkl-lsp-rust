//! `textDocument/selectionRange` handler.
//!
//! For each requested position, walks the CST top-down to find the
//! smallest node containing the cursor and produces a chain of widening
//! ranges via the LSP `parent` linking.

use pkl_syntax::cst::{
    self, significant_span, AstNode, ClassMember, Expr, Item, MethodDecl, Module, ObjectBody,
    ObjectMember, PropertyDecl, PropertyValue,
};
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
    collect_spans_containing(&doc.module(), offset, &mut spans);

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
    for item in module.items() {
        let item_span = significant_span(item.syntax());
        if !item_span.contains(offset) {
            continue;
        }
        out.push(item_span);
        match item {
            Item::Class(c) => {
                if let Some(body) = c.body() {
                    let body_span = significant_span(body.syntax());
                    if body_span.contains(offset) {
                        out.push(body_span);
                        for m in body.members() {
                            let m_span = significant_span(m.syntax());
                            if m_span.contains(offset) {
                                out.push(m_span);
                                match m {
                                    ClassMember::Property(p) => {
                                        walk_class_property(&p, offset, out)
                                    }
                                    ClassMember::Method(m) => walk_class_method(&m, offset, out),
                                }
                            }
                        }
                    }
                }
            }
            Item::Property(p) => walk_property(&p, offset, out),
            Item::Method(m) => walk_method(&m, offset, out),
            _ => {}
        }
    }
    out.reverse();
}

fn walk_property(p: &PropertyDecl, offset: u32, out: &mut Vec<Span>) {
    match p.value() {
        Some(PropertyValue::Expr(e)) => walk_expr(&e, offset, out),
        Some(PropertyValue::ObjectBody(body)) => walk_object_body(&body, offset, out),
        None => {}
    }
}

fn walk_class_property(p: &cst::ClassPropertyDecl, offset: u32, out: &mut Vec<Span>) {
    match p.value() {
        Some(PropertyValue::Expr(e)) => walk_expr(&e, offset, out),
        Some(PropertyValue::ObjectBody(body)) => walk_object_body(&body, offset, out),
        None => {}
    }
}

fn walk_object_property(p: &cst::ObjectProperty, offset: u32, out: &mut Vec<Span>) {
    match p.value() {
        Some(PropertyValue::Expr(e)) => walk_expr(&e, offset, out),
        Some(PropertyValue::ObjectBody(body)) => walk_object_body(&body, offset, out),
        None => {}
    }
}

fn walk_method(m: &MethodDecl, offset: u32, out: &mut Vec<Span>) {
    if let Some(body) = m.body() {
        walk_expr(&body, offset, out);
    }
}

fn walk_class_method(m: &cst::ClassMethodDecl, offset: u32, out: &mut Vec<Span>) {
    if let Some(body) = m.body() {
        walk_expr(&body, offset, out);
    }
}

fn walk_object_method(m: &cst::ObjectMethod, offset: u32, out: &mut Vec<Span>) {
    if let Some(body) = m.body() {
        walk_expr(&body, offset, out);
    }
}

fn walk_object_body(body: &ObjectBody, offset: u32, out: &mut Vec<Span>) {
    let body_span = significant_span(body.syntax());
    if !body_span.contains(offset) {
        return;
    }
    out.push(body_span);
    for member in body.members() {
        let m_span = significant_span(member.syntax());
        if m_span.contains(offset) {
            out.push(m_span);
            match member {
                ObjectMember::Property(p) => walk_object_property(&p, offset, out),
                ObjectMember::Method(m) => walk_object_method(&m, offset, out),
                ObjectMember::Element(e) => {
                    if let Some(expr) = e.expr() {
                        walk_expr(&expr, offset, out);
                    }
                }
                ObjectMember::Entry(e) => match e.value() {
                    Some(PropertyValue::Expr(expr)) => walk_expr(&expr, offset, out),
                    Some(PropertyValue::ObjectBody(b)) => walk_object_body(&b, offset, out),
                    None => {}
                },
                ObjectMember::When(w) => {
                    if let Some(then_body) = w.then_body() {
                        walk_object_body(&then_body, offset, out);
                    }
                    if let Some(b) = w.else_body() {
                        walk_object_body(&b, offset, out);
                    }
                }
                ObjectMember::For(f) => {
                    if let Some(body) = f.body() {
                        walk_object_body(&body, offset, out);
                    }
                }
                ObjectMember::Spread(s) => {
                    if let Some(expr) = s.expr() {
                        walk_expr(&expr, offset, out);
                    }
                }
            }
        }
    }
}

fn walk_expr(expr: &Expr, offset: u32, out: &mut Vec<Span>) {
    let span = significant_span(expr.syntax());
    if !span.contains(offset) {
        return;
    }
    out.push(span);
    match expr {
        Expr::Paren(p) => {
            if let Some(inner) = p.inner() {
                walk_expr(&inner, offset, out);
            }
        }
        Expr::NonNull(n) => {
            if let Some(operand) = n.operand() {
                walk_expr(&operand, offset, out);
            }
        }
        Expr::Unary(u) => {
            if let Some(operand) = u.operand() {
                walk_expr(&operand, offset, out);
            }
        }
        Expr::Binary(b) => {
            if let Some(lhs) = b.lhs() {
                walk_expr(&lhs, offset, out);
            }
            if let Some(rhs) = b.rhs() {
                walk_expr(&rhs, offset, out);
            }
        }
        Expr::NullCoalesce(n) => {
            if let Some(lhs) = n.lhs() {
                walk_expr(&lhs, offset, out);
            }
            if let Some(rhs) = n.rhs() {
                walk_expr(&rhs, offset, out);
            }
        }
        Expr::TypeCheck(t) => {
            if let Some(operand) = t.operand() {
                walk_expr(&operand, offset, out);
            }
        }
        Expr::TypeCast(t) => {
            if let Some(operand) = t.operand() {
                walk_expr(&operand, offset, out);
            }
        }
        Expr::If(i) => {
            if let Some(c) = i.condition() {
                walk_expr(&c, offset, out);
            }
            if let Some(t) = i.then_branch() {
                walk_expr(&t, offset, out);
            }
            if let Some(e) = i.else_branch() {
                walk_expr(&e, offset, out);
            }
        }
        Expr::Let(l) => {
            if let Some(v) = l.value() {
                walk_expr(&v, offset, out);
            }
            if let Some(b) = l.body() {
                walk_expr(&b, offset, out);
            }
        }
        Expr::Lambda(lam) => {
            if let Some(body) = lam.body() {
                walk_expr(&body, offset, out);
            }
        }
        Expr::Call(c) => {
            if let Some(callee) = c.callee() {
                walk_expr(&callee, offset, out);
            }
            for a in c.args() {
                walk_expr(&a, offset, out);
            }
        }
        Expr::Index(i) => {
            if let Some(receiver) = i.receiver() {
                walk_expr(&receiver, offset, out);
            }
            if let Some(index) = i.index() {
                walk_expr(&index, offset, out);
            }
        }
        Expr::Member(m) => {
            if let Some(receiver) = m.receiver() {
                walk_expr(&receiver, offset, out);
            }
        }
        Expr::New(n) => {
            if let Some(body) = n.body() {
                walk_object_body(&body, offset, out);
            }
        }
        Expr::Amends(a) => {
            if let Some(base) = a.base() {
                walk_expr(&base, offset, out);
            }
            if let Some(body) = a.body() {
                walk_object_body(&body, offset, out);
            }
        }
        Expr::Throw(t) => {
            if let Some(arg) = t.argument() {
                walk_expr(&arg, offset, out);
            }
        }
        Expr::Trace(t) => {
            if let Some(arg) = t.argument() {
                walk_expr(&arg, offset, out);
            }
        }
        Expr::Read(r) => {
            if let Some(arg) = r.argument() {
                walk_expr(&arg, offset, out);
            }
        }
        Expr::Import(i) => {
            if let Some(arg) = i.argument() {
                walk_expr(&arg, offset, out);
            }
        }
        _ => {}
    }
}

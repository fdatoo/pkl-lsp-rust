//! `textDocument/signatureHelp` handler.
//!
//! Finds the innermost `Call` enclosing the cursor and renders its
//! callee's signature. The active parameter is derived from the count of
//! top-level commas between the opening paren and the cursor.

use pkl_analyze::SymbolKind;
use pkl_syntax::cst::{
    self, ident_text, significant_span, token_span, AstNode, ClassMember, Expr, Item, MethodDecl,
    ObjectBody, ObjectMember, PropertyDecl, PropertyValue,
};
use tower_lsp::lsp_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, Position,
    SignatureHelp, SignatureInformation,
};

use crate::document::Document;

pub fn signature_help_at(doc: &Document, position: Position) -> Option<SignatureHelp> {
    let offset = doc.position_to_offset(position);
    let module = doc.module();
    let mut best: Option<Expr> = None;
    for item in module.items() {
        walk_item(&item, offset, &mut best);
    }
    let call_expr = best?;
    let Expr::Call(call) = &call_expr else {
        return None;
    };
    let callee = call.callee()?;
    let args = call.args();
    let span = significant_span(call.syntax());

    let (signature, doc_text) = describe_callee(doc, &callee)?;
    let mut info = SignatureInformation {
        label: signature.clone(),
        documentation: doc_text.map(|d| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: d,
            })
        }),
        parameters: extract_parameters(&signature),
        active_parameter: None,
    };

    // Active parameter: how many top-level commas precede the cursor
    // between the call's opening paren and `offset`.
    let text = doc.rope.to_string();
    let bytes = text.as_bytes();
    let mut commas = 0u32;
    let mut depth_paren = 0i32;
    let mut depth_lt = 0i32;
    let start = span.start as usize;
    let end = offset as usize;
    let scan_start = bytes[start..end]
        .iter()
        .position(|&b| b == b'(')
        .map(|i| start + i + 1)
        .unwrap_or(start);
    for &b in &bytes[scan_start..end] {
        match b {
            b'(' => depth_paren += 1,
            b')' if depth_paren > 0 => depth_paren -= 1,
            b'<' => depth_lt += 1,
            b'>' if depth_lt > 0 => depth_lt -= 1,
            b',' if depth_paren == 0 && depth_lt == 0 => commas += 1,
            _ => {}
        }
    }
    if !args.is_empty() {
        info.active_parameter = Some(commas);
    }

    Some(SignatureHelp {
        signatures: vec![info],
        active_signature: Some(0),
        active_parameter: Some(commas),
    })
}

fn walk_item(item: &Item, offset: u32, best: &mut Option<Expr>) {
    match item {
        Item::Class(c) => {
            if let Some(body) = c.body() {
                for m in body.members() {
                    match m {
                        ClassMember::Property(p) => walk_class_property(&p, offset, best),
                        ClassMember::Method(m) => walk_class_method(&m, offset, best),
                    }
                }
            }
        }
        Item::Property(p) => walk_property(p, offset, best),
        Item::Method(m) => walk_method(m, offset, best),
        _ => {}
    }
}

fn walk_property(p: &PropertyDecl, offset: u32, best: &mut Option<Expr>) {
    match p.value() {
        Some(PropertyValue::Expr(e)) => walk_expr(&e, offset, best),
        Some(PropertyValue::ObjectBody(body)) => walk_object_body(&body, offset, best),
        None => {}
    }
}

fn walk_class_property(p: &cst::ClassPropertyDecl, offset: u32, best: &mut Option<Expr>) {
    match p.value() {
        Some(PropertyValue::Expr(e)) => walk_expr(&e, offset, best),
        Some(PropertyValue::ObjectBody(body)) => walk_object_body(&body, offset, best),
        None => {}
    }
}

fn walk_object_property(p: &cst::ObjectProperty, offset: u32, best: &mut Option<Expr>) {
    match p.value() {
        Some(PropertyValue::Expr(e)) => walk_expr(&e, offset, best),
        Some(PropertyValue::ObjectBody(body)) => walk_object_body(&body, offset, best),
        None => {}
    }
}

fn walk_method(m: &MethodDecl, offset: u32, best: &mut Option<Expr>) {
    if let Some(body) = m.body() {
        walk_expr(&body, offset, best);
    }
}

fn walk_class_method(m: &cst::ClassMethodDecl, offset: u32, best: &mut Option<Expr>) {
    if let Some(body) = m.body() {
        walk_expr(&body, offset, best);
    }
}

fn walk_object_method(m: &cst::ObjectMethod, offset: u32, best: &mut Option<Expr>) {
    if let Some(body) = m.body() {
        walk_expr(&body, offset, best);
    }
}

fn walk_object_body(body: &ObjectBody, offset: u32, best: &mut Option<Expr>) {
    let body_span = significant_span(body.syntax());
    if !body_span.contains(offset) {
        return;
    }
    for member in body.members() {
        match member {
            ObjectMember::Property(p) => walk_object_property(&p, offset, best),
            ObjectMember::Method(m) => walk_object_method(&m, offset, best),
            ObjectMember::Element(e) => {
                if let Some(expr) = e.expr() {
                    walk_expr(&expr, offset, best);
                }
            }
            ObjectMember::Entry(e) => match e.value() {
                Some(PropertyValue::Expr(expr)) => walk_expr(&expr, offset, best),
                Some(PropertyValue::ObjectBody(b)) => walk_object_body(&b, offset, best),
                None => {}
            },
            ObjectMember::When(w) => {
                if let Some(then_body) = w.then_body() {
                    walk_object_body(&then_body, offset, best);
                }
                if let Some(b) = w.else_body() {
                    walk_object_body(&b, offset, best);
                }
            }
            ObjectMember::For(f) => {
                if let Some(body) = f.body() {
                    walk_object_body(&body, offset, best);
                }
            }
            ObjectMember::Spread(s) => {
                if let Some(expr) = s.expr() {
                    walk_expr(&expr, offset, best);
                }
            }
        }
    }
}

fn walk_expr(expr: &Expr, offset: u32, best: &mut Option<Expr>) {
    let span = significant_span(expr.syntax());
    if !span.contains(offset) {
        return;
    }
    if matches!(expr, Expr::Call(_)) {
        *best = Some(expr.clone());
    }
    match expr {
        Expr::Paren(p) => {
            if let Some(inner) = p.inner() {
                walk_expr(&inner, offset, best);
            }
        }
        Expr::NonNull(n) => {
            if let Some(operand) = n.operand() {
                walk_expr(&operand, offset, best);
            }
        }
        Expr::Unary(u) => {
            if let Some(operand) = u.operand() {
                walk_expr(&operand, offset, best);
            }
        }
        Expr::Binary(b) => {
            if let Some(lhs) = b.lhs() {
                walk_expr(&lhs, offset, best);
            }
            if let Some(rhs) = b.rhs() {
                walk_expr(&rhs, offset, best);
            }
        }
        Expr::NullCoalesce(n) => {
            if let Some(lhs) = n.lhs() {
                walk_expr(&lhs, offset, best);
            }
            if let Some(rhs) = n.rhs() {
                walk_expr(&rhs, offset, best);
            }
        }
        Expr::TypeCheck(t) => {
            if let Some(operand) = t.operand() {
                walk_expr(&operand, offset, best);
            }
        }
        Expr::TypeCast(t) => {
            if let Some(operand) = t.operand() {
                walk_expr(&operand, offset, best);
            }
        }
        Expr::If(i) => {
            if let Some(c) = i.condition() {
                walk_expr(&c, offset, best);
            }
            if let Some(t) = i.then_branch() {
                walk_expr(&t, offset, best);
            }
            if let Some(e) = i.else_branch() {
                walk_expr(&e, offset, best);
            }
        }
        Expr::Let(l) => {
            if let Some(v) = l.value() {
                walk_expr(&v, offset, best);
            }
            if let Some(b) = l.body() {
                walk_expr(&b, offset, best);
            }
        }
        Expr::Lambda(lam) => {
            if let Some(body) = lam.body() {
                walk_expr(&body, offset, best);
            }
        }
        Expr::Call(c) => {
            if let Some(callee) = c.callee() {
                walk_expr(&callee, offset, best);
            }
            for a in c.args() {
                walk_expr(&a, offset, best);
            }
        }
        Expr::Index(i) => {
            if let Some(receiver) = i.receiver() {
                walk_expr(&receiver, offset, best);
            }
            if let Some(index) = i.index() {
                walk_expr(&index, offset, best);
            }
        }
        Expr::Member(m) => {
            if let Some(receiver) = m.receiver() {
                walk_expr(&receiver, offset, best);
            }
        }
        Expr::New(n) => {
            if let Some(body) = n.body() {
                walk_object_body(&body, offset, best);
            }
        }
        Expr::Amends(a) => {
            if let Some(base) = a.base() {
                walk_expr(&base, offset, best);
            }
            if let Some(body) = a.body() {
                walk_object_body(&body, offset, best);
            }
        }
        Expr::Throw(t) => {
            if let Some(arg) = t.argument() {
                walk_expr(&arg, offset, best);
            }
        }
        Expr::Trace(t) => {
            if let Some(arg) = t.argument() {
                walk_expr(&arg, offset, best);
            }
        }
        Expr::Read(r) => {
            if let Some(arg) = r.argument() {
                walk_expr(&arg, offset, best);
            }
        }
        Expr::Import(i) => {
            if let Some(arg) = i.argument() {
                walk_expr(&arg, offset, best);
            }
        }
        _ => {}
    }
}

fn describe_callee(doc: &Document, callee: &Expr) -> Option<(String, Option<String>)> {
    // 1. Member call: `expr.method(...)` — consult the inference's MemberRef.
    if let Expr::Member(m) = callee {
        if let Some(name_tok) = m.name() {
            let name_span = token_span(&name_tok);
            if let Some(member) = doc.analysis.inference.member_refs.get(&name_span.start) {
                if let Some(sm) = member.stdlib_member {
                    return Some((sm.signature.to_string(), Some(sm.doc.to_string())));
                }
                if let Some(user_id) = member.user_member {
                    let sym = doc.analysis.resolution.symbol(user_id);
                    let sig = sym.signature.clone().unwrap_or_else(|| sym.name.clone());
                    return Some((sig, sym.doc.clone()));
                }
            }
        }
    }

    // 2. Ident call: top-level function symbol.
    if let Expr::Ident(id) = callee {
        if id.special().is_none() {
            if let Some(tok) = id.token() {
                let span = token_span(&tok);
                if let Some(sym_id) = doc.analysis.resolution.by_span_start.get(&span.start) {
                    let sym = doc.analysis.resolution.symbol(*sym_id);
                    if matches!(sym.kind, SymbolKind::Method) {
                        let sig = sym
                            .signature
                            .clone()
                            .unwrap_or_else(|| format!("{}(...)", sym.name));
                        return Some((sig, sym.doc.clone()));
                    }
                }
            }
        }
    }

    // Use ident_text just to keep the import referenced if symbol path doesn't.
    let _ = ident_text;

    None
}

/// Split a signature string into its parameter-label ranges so editors
/// can underline the active one. Returns one entry per top-level comma-
/// separated piece between the outermost `(` and `)`.
fn extract_parameters(signature: &str) -> Option<Vec<ParameterInformation>> {
    let bytes = signature.as_bytes();
    let lparen = bytes.iter().position(|&b| b == b'(')?;
    let rparen = signature.rfind(')')?;
    if lparen >= rparen {
        return None;
    }
    let inner = &signature[lparen + 1..rparen];
    let mut out = Vec::new();
    let mut depth_paren = 0i32;
    let mut depth_lt = 0i32;
    let mut start = 0usize;
    for (i, &b) in inner.as_bytes().iter().enumerate() {
        match b {
            b'(' => depth_paren += 1,
            b')' if depth_paren > 0 => depth_paren -= 1,
            b'<' => depth_lt += 1,
            b'>' if depth_lt > 0 => depth_lt -= 1,
            b',' if depth_paren == 0 && depth_lt == 0 => {
                let trimmed = inner[start..i].trim();
                if !trimmed.is_empty() {
                    out.push(ParameterInformation {
                        label: ParameterLabel::Simple(trimmed.to_string()),
                        documentation: None,
                    });
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    let trimmed = inner[start..].trim();
    if !trimmed.is_empty() {
        out.push(ParameterInformation {
            label: ParameterLabel::Simple(trimmed.to_string()),
            documentation: None,
        });
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn active_parameter_counts_from_argument_list() {
        let src = "endpointHost: String = \"api\"\n\
function render(host: String, port: Int): String = host\n\
description = render(endpointHost, 443)\n";
        let doc = Document::new(src.to_string(), 1);
        let position = Position {
            line: 2,
            character: "description = render(endpointHost, 4".len() as u32,
        };

        let help = signature_help_at(&doc, position).expect("signature help");

        assert_eq!(help.active_parameter, Some(1));
        assert_eq!(help.signatures[0].active_parameter, Some(1));
    }
}

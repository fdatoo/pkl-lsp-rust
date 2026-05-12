//! `textDocument/signatureHelp` handler.
//!
//! Finds the innermost `Call` enclosing the cursor and renders its
//! callee's signature. The active parameter is derived from the count of
//! top-level commas between the opening paren and the cursor.

use pkl_analyze::SymbolKind;
use pkl_syntax::ast::*;
use tower_lsp::lsp_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, Position,
    SignatureHelp, SignatureInformation,
};

use crate::document::Document;

pub fn signature_help_at(doc: &Document, position: Position) -> Option<SignatureHelp> {
    let offset = doc.position_to_offset(position);
    let module = &doc.parsed.module;
    let mut best: Option<&Expr> = None;
    for item in &module.items {
        walk_item(item, offset, &mut best);
    }
    let call = best?;
    let Expr::Call {
        callee, args, span, ..
    } = call
    else {
        return None;
    };

    let (signature, doc_text) = describe_callee(doc, callee)?;
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
    for (i, &b) in bytes[start..offset as usize].iter().enumerate() {
        if i == 0 && b == b'(' {
            continue;
        }
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

fn walk_item<'a>(item: &'a Item, offset: u32, best: &mut Option<&'a Expr>) {
    match item {
        Item::Class(c) => {
            if let Some(body) = &c.body {
                for m in &body.members {
                    match m {
                        ClassMember::Property(p) => walk_property(p, offset, best),
                        ClassMember::Method(m) => walk_method(m, offset, best),
                    }
                }
            }
        }
        Item::Property(p) => walk_property(p, offset, best),
        Item::Method(m) => walk_method(m, offset, best),
        _ => {}
    }
}

fn walk_property<'a>(p: &'a PropertyDecl, offset: u32, best: &mut Option<&'a Expr>) {
    if let Some(value) = &p.value {
        match value {
            PropertyValue::Expr(e) => walk_expr(e, offset, best),
            PropertyValue::ObjectBody(body) => walk_object_body(body, offset, best),
        }
    }
}

fn walk_method<'a>(m: &'a MethodDecl, offset: u32, best: &mut Option<&'a Expr>) {
    if let Some(body) = &m.body {
        walk_expr(body, offset, best);
    }
}

fn walk_object_body<'a>(body: &'a ObjectBody, offset: u32, best: &mut Option<&'a Expr>) {
    if !body.span.contains(offset) {
        return;
    }
    for member in &body.members {
        match member {
            ObjectMember::Property(p) => walk_property(p, offset, best),
            ObjectMember::Method(m) => walk_method(m, offset, best),
            ObjectMember::Element(e) => walk_expr(e, offset, best),
            ObjectMember::Entry { value, .. } => match value {
                PropertyValue::Expr(e) => walk_expr(e, offset, best),
                PropertyValue::ObjectBody(b) => walk_object_body(b, offset, best),
            },
            ObjectMember::When {
                then_body,
                else_body,
                ..
            } => {
                walk_object_body(then_body, offset, best);
                if let Some(b) = else_body {
                    walk_object_body(b, offset, best);
                }
            }
            ObjectMember::For { body, .. } => walk_object_body(body, offset, best),
            ObjectMember::Spread { expr, .. } => walk_expr(expr, offset, best),
        }
    }
}

fn walk_expr<'a>(expr: &'a Expr, offset: u32, best: &mut Option<&'a Expr>) {
    if !expr.span().contains(offset) {
        return;
    }
    if matches!(expr, Expr::Call { .. }) {
        *best = Some(expr);
    }
    match expr {
        Expr::Paren { inner, .. } | Expr::NonNull { operand: inner, .. } => {
            walk_expr(inner, offset, best)
        }
        Expr::Unary { operand, .. } => walk_expr(operand, offset, best),
        Expr::Binary { lhs, rhs, .. } => {
            walk_expr(lhs, offset, best);
            walk_expr(rhs, offset, best);
        }
        Expr::TypeCheck { operand, .. } | Expr::TypeCast { operand, .. } => {
            walk_expr(operand, offset, best)
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            walk_expr(cond, offset, best);
            walk_expr(then_branch, offset, best);
            walk_expr(else_branch, offset, best);
        }
        Expr::Let { value, body, .. } => {
            walk_expr(value, offset, best);
            walk_expr(body, offset, best);
        }
        Expr::Lambda { body, .. } => walk_expr(body, offset, best),
        Expr::Call { callee, args, .. } => {
            walk_expr(callee, offset, best);
            for a in args {
                walk_expr(a, offset, best);
            }
        }
        Expr::Index {
            receiver, index, ..
        } => {
            walk_expr(receiver, offset, best);
            walk_expr(index, offset, best);
        }
        Expr::Member { receiver, .. } => walk_expr(receiver, offset, best),
        Expr::New { body, .. } => walk_object_body(body, offset, best),
        Expr::AmendsObject { base, body, .. } => {
            walk_expr(base, offset, best);
            walk_object_body(body, offset, best);
        }
        Expr::Throw { argument, .. }
        | Expr::Trace { argument, .. }
        | Expr::Read { argument, .. } => walk_expr(argument, offset, best),
        _ => {}
    }
}

fn describe_callee(doc: &Document, callee: &Expr) -> Option<(String, Option<String>)> {
    // 1. Member call: `expr.method(...)` — consult the inference's MemberRef.
    if let Expr::Member { name, .. } = callee {
        if let Some(member) = doc.analysis.inference.member_refs.get(&name.span.start) {
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

    // 2. Ident call: top-level function symbol.
    if let Expr::Ident(id) = callee {
        if let Some(sym_id) = doc.analysis.resolution.by_span_start.get(&id.span.start) {
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

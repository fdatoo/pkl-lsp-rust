//! `textDocument/inlayHint` handler.
//!
//! Emits a `: Type` hint after every property declaration whose
//! annotation was omitted but whose inferred type is known. Method
//! parameters and let-bindings are skipped for now — many of them are
//! contextual and the hint signal-to-noise can drop fast.

use pkl_analyze::Ty;
use pkl_syntax::cst::{
    self, significant_span, token_span, AstNode, ClassMember, Item, PropertyValue,
};
use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Range};

use crate::document::Document;

pub fn inlay_hints(doc: &Document, range: Range) -> Vec<InlayHint> {
    let mut out = Vec::new();
    let module = doc.module();

    let view_start = doc.position_to_offset(range.start);
    let view_end = doc.position_to_offset(range.end);

    for item in module.items() {
        match item {
            Item::Property(p) => maybe_hint_property(doc, &p, view_start, view_end, &mut out),
            Item::Class(c) => {
                if let Some(body) = c.body() {
                    for m in body.members() {
                        if let ClassMember::Property(p) = m {
                            maybe_hint_class_property(doc, &p, view_start, view_end, &mut out);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    out
}

fn maybe_hint_property(
    doc: &Document,
    p: &cst::PropertyDecl,
    view_start: u32,
    view_end: u32,
    out: &mut Vec<InlayHint>,
) {
    if p.ty().is_some() {
        return;
    }
    let span = significant_span(p.syntax());
    if span.end <= view_start || span.start >= view_end {
        return;
    }
    let Some(PropertyValue::Expr(e)) = p.value() else {
        return;
    };
    let expr_span = significant_span(e.syntax());
    let Some(ty) = doc.analysis.inference.type_of(expr_span.start) else {
        return;
    };
    if matches!(ty, Ty::Unknown) {
        return;
    }
    let Some(name_tok) = p.name() else { return };
    let pos = doc.span_to_range(token_span(&name_tok)).end;
    out.push(InlayHint {
        position: pos,
        label: InlayHintLabel::String(format!(": {}", ty)),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(false),
        padding_right: Some(true),
        data: None,
    });
}

fn maybe_hint_class_property(
    doc: &Document,
    p: &cst::ClassPropertyDecl,
    view_start: u32,
    view_end: u32,
    out: &mut Vec<InlayHint>,
) {
    if p.ty().is_some() {
        return;
    }
    let span = significant_span(p.syntax());
    if span.end <= view_start || span.start >= view_end {
        return;
    }
    let Some(PropertyValue::Expr(e)) = p.value() else {
        return;
    };
    let expr_span = significant_span(e.syntax());
    let Some(ty) = doc.analysis.inference.type_of(expr_span.start) else {
        return;
    };
    if matches!(ty, Ty::Unknown) {
        return;
    }
    let Some(name_tok) = p.name() else { return };
    let pos = doc.span_to_range(token_span(&name_tok)).end;
    out.push(InlayHint {
        position: pos,
        label: InlayHintLabel::String(format!(": {}", ty)),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(false),
        padding_right: Some(true),
        data: None,
    });
}

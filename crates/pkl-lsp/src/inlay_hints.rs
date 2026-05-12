//! `textDocument/inlayHint` handler.
//!
//! Emits a `: Type` hint after every property declaration whose
//! annotation was omitted but whose inferred type is known. Method
//! parameters and let-bindings are skipped for now — many of them are
//! contextual and the hint signal-to-noise can drop fast.

use pkl_analyze::Ty;
use pkl_syntax::ast::*;
use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Range};

use crate::document::Document;

pub fn inlay_hints(doc: &Document, range: Range) -> Vec<InlayHint> {
    let mut out = Vec::new();
    let module = &doc.parsed.module;

    let view_start = doc.position_to_offset(range.start);
    let view_end = doc.position_to_offset(range.end);

    for item in &module.items {
        match item {
            Item::Property(p) => maybe_hint_property(doc, p, view_start, view_end, &mut out),
            Item::Class(c) => {
                if let Some(body) = &c.body {
                    for m in &body.members {
                        if let ClassMember::Property(p) = m {
                            maybe_hint_property(doc, p, view_start, view_end, &mut out);
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
    p: &PropertyDecl,
    view_start: u32,
    view_end: u32,
    out: &mut Vec<InlayHint>,
) {
    if p.ty.is_some() {
        return;
    }
    if p.span.end <= view_start || p.span.start >= view_end {
        return;
    }
    let Some(PropertyValue::Expr(e)) = &p.value else {
        return;
    };
    let Some(ty) = doc.analysis.inference.type_of(e.span().start) else {
        return;
    };
    if matches!(ty, Ty::Unknown) {
        return;
    }
    let label = format!(": {}", ty);
    let pos = doc.span_to_range(p.name.span).end;
    out.push(InlayHint {
        position: pos,
        label: InlayHintLabel::String(label),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(false),
        padding_right: Some(true),
        data: None,
    });
}

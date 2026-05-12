//! `textDocument/codeAction` handler.
//!
//! Currently offers a single quick-fix: when a property declaration has
//! no type annotation but the inferrer worked out its type, suggest
//! inserting `: Type` after the name. More actions will follow.

use std::collections::HashMap;

use pkl_analyze::Ty;
use pkl_syntax::ast::*;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionResponse, Range, TextEdit, Url,
    WorkspaceEdit,
};

use crate::document::Document;

pub fn code_actions_at(uri: &Url, doc: &Document, range: Range) -> Option<CodeActionResponse> {
    let mut out: Vec<CodeActionOrCommand> = Vec::new();
    let start = doc.position_to_offset(range.start);
    let end = doc.position_to_offset(range.end);

    for item in &doc.parsed.module.items {
        if let Item::Property(p) = item {
            collect_property_annotation_action(uri, doc, p, start, end, &mut out);
        }
        if let Item::Class(c) = item {
            if let Some(body) = &c.body {
                for m in &body.members {
                    if let ClassMember::Property(p) = m {
                        collect_property_annotation_action(uri, doc, p, start, end, &mut out);
                    }
                }
            }
        }
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn collect_property_annotation_action(
    uri: &Url,
    doc: &Document,
    p: &PropertyDecl,
    range_start: u32,
    range_end: u32,
    out: &mut Vec<CodeActionOrCommand>,
) {
    if p.ty.is_some() {
        return;
    }
    // Cursor / selection must touch the property.
    if p.span.end <= range_start || p.span.start >= range_end {
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
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    let insert_at = doc.span_to_range(p.name.span).end;
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range {
                start: insert_at,
                end: insert_at,
            },
            new_text: format!(": {}", ty),
        }],
    );
    out.push(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Annotate `{}: {}`", p.name.name, ty),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }),
        command: None,
        is_preferred: Some(true),
        disabled: None,
        data: None,
    }));
}

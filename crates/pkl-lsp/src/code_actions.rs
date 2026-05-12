//! `textDocument/codeAction` handler.
//!
//! Currently offers a single quick-fix: when a property declaration has
//! no type annotation but the inferrer worked out its type, suggest
//! inserting `: Type` after the name. More actions will follow.

use std::collections::HashMap;

use pkl_analyze::Ty;
use pkl_syntax::cst::{
    self, ident_text, significant_span, token_span, AstNode, ClassMember, Item, PropertyValue,
};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionResponse, Range, TextEdit, Url,
    WorkspaceEdit,
};

use crate::document::Document;

pub fn code_actions_at(uri: &Url, doc: &Document, range: Range) -> Option<CodeActionResponse> {
    let mut out: Vec<CodeActionOrCommand> = Vec::new();
    let start = doc.position_to_offset(range.start);
    let end = doc.position_to_offset(range.end);

    for item in doc.module().items() {
        if let Item::Property(p) = &item {
            collect_property_action(uri, doc, p, start, end, &mut out);
        }
        if let Item::Class(c) = &item {
            if let Some(body) = c.body() {
                for m in body.members() {
                    if let ClassMember::Property(p) = m {
                        collect_class_property_action(uri, doc, &p, start, end, &mut out);
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

fn collect_property_action(
    uri: &Url,
    doc: &Document,
    p: &cst::PropertyDecl,
    range_start: u32,
    range_end: u32,
    out: &mut Vec<CodeActionOrCommand>,
) {
    if p.ty().is_some() {
        return;
    }
    let span = significant_span(p.syntax());
    if span.end <= range_start || span.start >= range_end {
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
    let name_text = ident_text(&name_tok);
    let insert_at = doc.span_to_range(token_span(&name_tok)).end;
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
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
        title: format!("Annotate `{}: {}`", name_text, ty),
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

fn collect_class_property_action(
    uri: &Url,
    doc: &Document,
    p: &cst::ClassPropertyDecl,
    range_start: u32,
    range_end: u32,
    out: &mut Vec<CodeActionOrCommand>,
) {
    if p.ty().is_some() {
        return;
    }
    let span = significant_span(p.syntax());
    if span.end <= range_start || span.start >= range_end {
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
    let name_text = ident_text(&name_tok);
    let insert_at = doc.span_to_range(token_span(&name_tok)).end;
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
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
        title: format!("Annotate `{}: {}`", name_text, ty),
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

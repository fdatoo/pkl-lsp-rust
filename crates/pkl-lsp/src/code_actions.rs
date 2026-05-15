//! `textDocument/codeAction` handler.
//!
//! Currently offers a single quick-fix: when a property declaration has
//! no type annotation but the inferrer worked out its type, suggest
//! inserting `: Type` after the name. More actions will follow.

use std::collections::HashMap;

use pkl_analyze::{ModuleGraph, Ty};
use pkl_syntax::cst::{
    self, ident_text, significant_span, token_span, AstNode, ClassMember, Item, PropertyValue,
};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionResponse, Diagnostic, Position,
    Range, TextEdit, Url, WorkspaceEdit,
};

use crate::document::Document;
use crate::uri::{module_uri_to_url, url_to_module_uri};

pub fn code_actions_at(
    uri: &Url,
    doc: &Document,
    graph: &ModuleGraph,
    range: Range,
    diagnostics: &[Diagnostic],
) -> Option<CodeActionResponse> {
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
    collect_create_property_actions(uri, diagnostics, &mut out);
    collect_imported_member_actions(uri, graph, diagnostics, &mut out);

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

fn collect_create_property_actions(
    uri: &Url,
    diagnostics: &[Diagnostic],
    out: &mut Vec<CodeActionOrCommand>,
) {
    for diagnostic in diagnostics {
        let Some(name) = diagnostic
            .message
            .strip_prefix("unknown identifier `")
            .and_then(|rest| rest.split_once('`').map(|(name, _)| name))
        else {
            continue;
        };
        let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
        changes.insert(
            uri.clone(),
            vec![TextEdit {
                range: Range {
                    start: Position::new(0, 0),
                    end: Position::new(0, 0),
                },
                new_text: format!("{} = null\n", name),
            }],
        );
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Create property `{}`", name),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(false),
            disabled: None,
            data: None,
        }));
    }
}

fn collect_imported_member_actions(
    uri: &Url,
    graph: &ModuleGraph,
    diagnostics: &[Diagnostic],
    out: &mut Vec<CodeActionOrCommand>,
) {
    let module_uri = url_to_module_uri(uri);
    for diagnostic in diagnostics {
        let Some((member_name, alias)) = parse_missing_imported_member(&diagnostic.message) else {
            continue;
        };
        let Some(imported) = graph.imported_module(&module_uri, alias) else {
            continue;
        };
        let Some(target_url) = module_uri_to_url(&imported.uri) else {
            continue;
        };
        let end = end_position(&imported.source);
        let prefix = if imported.source.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
        changes.insert(
            target_url.clone(),
            vec![TextEdit {
                range: Range { start: end, end },
                new_text: format!("{}{} = null\n", prefix, member_name),
            }],
        );
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Create `{}` in imported module `{}`", member_name, alias),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(false),
            disabled: None,
            data: None,
        }));
    }
}

fn parse_missing_imported_member(message: &str) -> Option<(&str, &str)> {
    let rest = message.strip_prefix("no member `")?;
    let (member_name, rest) = rest.split_once('`')?;
    let rest = rest.strip_prefix(" in imported module `")?;
    let (alias, _) = rest.split_once('`')?;
    Some((member_name, alias))
}

fn end_position(source: &str) -> Position {
    let mut line = 0u32;
    let mut col = 0u32;
    for c in source.chars() {
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += c.len_utf16() as u32;
        }
    }
    Position::new(line, col)
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

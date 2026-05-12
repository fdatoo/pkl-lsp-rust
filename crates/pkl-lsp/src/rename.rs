//! `textDocument/rename` and `textDocument/prepareRename` handlers.
//!
//! Renames are computed against the per-file resolver output: the
//! symbol's name span plus every recorded reference becomes a text edit.
//! We refuse to rename stdlib symbols (no source location) and silently
//! refuse on unresolved cursors. Cross-file rename is future work — the
//! graph would need to surface references across modules first.

use std::collections::HashMap;

use tower_lsp::lsp_types::{Position, PrepareRenameResponse, TextEdit, Url, WorkspaceEdit};

use crate::document::Document;

/// Validate that the cursor is on a rename-able symbol and return the
/// range to highlight. `OneOf::Left(range)` is the simplest of the LSP
/// prepare-rename response shapes.
pub fn prepare_rename_at(doc: &Document, position: Position) -> Option<PrepareRenameResponse> {
    let offset = doc.position_to_offset(position);
    let symbol_id = doc.analysis.resolution.symbol_at_offset(offset)?;
    let symbol = doc.analysis.resolution.symbol(symbol_id);
    if symbol.origin.is_stdlib() {
        return None;
    }
    Some(PrepareRenameResponse::Range(
        doc.span_to_range(symbol.name_span),
    ))
}

/// Compute a `WorkspaceEdit` that renames the symbol at `position` to
/// `new_name`. Returns `None` when the cursor isn't on a rename-able
/// symbol (e.g. stdlib or unresolved).
pub fn rename_at(
    uri: &Url,
    doc: &Document,
    position: Position,
    new_name: String,
) -> Option<WorkspaceEdit> {
    let offset = doc.position_to_offset(position);
    let symbol_id = doc.analysis.resolution.symbol_at_offset(offset)?;
    let symbol = doc.analysis.resolution.symbol(symbol_id);
    if symbol.origin.is_stdlib() {
        return None;
    }

    let mut edits: Vec<TextEdit> = Vec::new();
    edits.push(TextEdit {
        range: doc.span_to_range(symbol.name_span),
        new_text: new_name.clone(),
    });
    for reference in &doc.analysis.resolution.references {
        if reference.symbol == symbol_id {
            edits.push(TextEdit {
                range: doc.span_to_range(reference.span),
                new_text: new_name.clone(),
            });
        }
    }

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), edits);

    Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    })
}

//! `textDocument/rename` and `textDocument/prepareRename` handlers.
//!
//! Renames are computed against the per-file resolver output for the
//! originating file, then extended across the workspace via the module
//! graph: every dependent module that accesses the symbol through an
//! `imported.Member` form gets a text edit too.
//!
//! Stdlib symbols and unresolved cursors are refused. Renaming an import
//! alias only edits the importing file — the alias is a local name, not
//! the imported module's identity.

use std::collections::HashMap;

use dashmap::DashMap;
use tower_lsp::lsp_types::{Position, PrepareRenameResponse, Range, TextEdit, Url, WorkspaceEdit};

use pkl_analyze::{ModuleGraph, SymbolKind};
use pkl_syntax::Span;

use crate::document::Document;
use crate::uri::{module_uri_to_url, url_to_module_uri};

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
///
/// The edits cover:
/// 1. The symbol's defining name span in the originating file.
/// 2. Every local reference site in the originating file.
/// 3. For each dependent module that accesses this symbol through an
///    `alias.<name>` form, the corresponding member-name span.
///
/// Renaming an import alias intentionally stays local — the alias is a
/// name choice belonging to the importing file, not the imported module.
pub fn rename_at(
    uri: &Url,
    doc: &Document,
    documents: &DashMap<Url, Document>,
    graph: &ModuleGraph,
    position: Position,
    new_name: String,
) -> Option<WorkspaceEdit> {
    let offset = doc.position_to_offset(position);
    let symbol_id = doc.analysis.resolution.symbol_at_offset(offset)?;
    let symbol = doc.analysis.resolution.symbol(symbol_id);
    if symbol.origin.is_stdlib() {
        return None;
    }

    let mut local_edits: Vec<TextEdit> = Vec::new();
    local_edits.push(TextEdit {
        range: doc.span_to_range(symbol.name_span),
        new_text: new_name.clone(),
    });
    for reference in &doc.analysis.resolution.references {
        if reference.symbol == symbol_id {
            local_edits.push(TextEdit {
                range: doc.span_to_range(reference.span),
                new_text: new_name.clone(),
            });
        }
    }

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(uri.clone(), local_edits);

    // Cross-module pass: skipped for import aliases (a local-only name)
    // and for symbols nested inside containers (private to their parent).
    let is_import_alias = matches!(symbol.kind, SymbolKind::Import { .. });
    let is_top_level = symbol.container.is_none();
    if !is_import_alias && is_top_level {
        let module_uri = url_to_module_uri(uri);
        for (dep_uri, span) in graph.references_to(&module_uri, &symbol.name) {
            let Some(dep_url) = module_uri_to_url(&dep_uri) else {
                continue;
            };
            // Don't double-edit the originating file even if it imports
            // itself (rare but legal — the loader resolves it to the
            // same canonical URI).
            if dep_url == *uri {
                continue;
            }
            let range = range_for_span(documents, graph, &dep_url, &dep_uri, span);
            let Some(range) = range else {
                continue;
            };
            changes.entry(dep_url).or_default().push(TextEdit {
                range,
                new_text: new_name.clone(),
            });
        }
    }

    Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    })
}

/// Map a byte-offset `Span` belonging to module `module_uri` (and its
/// LSP `Url` mirror `module_url`) into an LSP `Range`. Prefers the open
/// document's rope (UTF-16 accurate via `Rope`); falls back to the
/// graph's cached source text when the dependent is not open.
pub(crate) fn range_for_span(
    documents: &DashMap<Url, Document>,
    graph: &ModuleGraph,
    module_url: &Url,
    module_uri: &str,
    span: Span,
) -> Option<Range> {
    if let Some(doc) = documents.get(module_url) {
        return Some(doc.span_to_range(span));
    }
    let entry = graph.get(module_uri)?;
    Some(span_to_range_in_source(&entry.source, span))
}

/// UTF-16-aware byte-span → LSP `Range` when we only have the raw
/// source string (no rope). Mirrors `goto::span_to_range_in`.
pub(crate) fn span_to_range_in_source(source: &str, span: Span) -> Range {
    Range {
        start: position_in_source(source, span.start as usize),
        end: position_in_source(source, span.end as usize),
    }
}

fn position_in_source(source: &str, byte_offset: usize) -> Position {
    let bytes = source.as_bytes();
    let cap = byte_offset.min(bytes.len());
    let mut line: u32 = 0;
    let mut line_start: usize = 0;
    for (i, b) in bytes.iter().enumerate().take(cap) {
        if *b == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    let line_text = &source[line_start..cap];
    let utf16_col: u32 = line_text.chars().map(|c| c.len_utf16() as u32).sum();
    Position {
        line,
        character: utf16_col,
    }
}

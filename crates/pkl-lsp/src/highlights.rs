//! `textDocument/documentHighlight` handler.
//!
//! Same data as references, but scoped to the current document and shaped
//! as `DocumentHighlight` (range + kind) for editors that draw "occurrences"
//! highlights when the cursor is on a symbol.

use tower_lsp::lsp_types::{DocumentHighlight, DocumentHighlightKind, Position};

use crate::document::Document;

pub fn highlights_at(doc: &Document, position: Position) -> Option<Vec<DocumentHighlight>> {
    let offset = doc.position_to_offset(position);
    let symbol_id = doc.analysis.resolution.symbol_at_offset(offset)?;
    let symbol = doc.analysis.resolution.symbol(symbol_id);

    let mut out: Vec<DocumentHighlight> = Vec::new();
    if !symbol.origin.is_stdlib() {
        out.push(DocumentHighlight {
            range: doc.span_to_range(symbol.name_span),
            kind: Some(DocumentHighlightKind::WRITE),
        });
    }
    for reference in &doc.analysis.resolution.references {
        if reference.symbol == symbol_id {
            out.push(DocumentHighlight {
                range: doc.span_to_range(reference.span),
                kind: Some(DocumentHighlightKind::READ),
            });
        }
    }
    Some(out)
}

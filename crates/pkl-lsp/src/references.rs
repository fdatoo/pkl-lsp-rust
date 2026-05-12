//! `textDocument/references` handler.
//!
//! Given a cursor position, returns every reference span pointing at the
//! same symbol, optionally including the definition site. Cross-file
//! references are not surfaced yet — the module graph would need a
//! reverse-import index.

use tower_lsp::lsp_types::{Location, Position, Url};

use crate::document::Document;

pub fn references_at(
    uri: &Url,
    doc: &Document,
    position: Position,
    include_declaration: bool,
) -> Option<Vec<Location>> {
    let offset = doc.position_to_offset(position);
    let symbol_id = doc.analysis.resolution.symbol_at_offset(offset)?;
    let symbol = doc.analysis.resolution.symbol(symbol_id);

    let mut locations = Vec::new();
    if include_declaration && !symbol.origin.is_stdlib() {
        locations.push(Location {
            uri: uri.clone(),
            range: doc.span_to_range(symbol.name_span),
        });
    }
    for reference in &doc.analysis.resolution.references {
        if reference.symbol == symbol_id {
            locations.push(Location {
                uri: uri.clone(),
                range: doc.span_to_range(reference.span),
            });
        }
    }
    Some(locations)
}

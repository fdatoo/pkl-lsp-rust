//! `textDocument/references` handler.
//!
//! Given a cursor position, returns every reference span pointing at the
//! same symbol — within the originating file (resolver-recorded
//! references) and across every dependent module that accesses it via
//! `alias.<name>` (member-access references threaded through the module
//! graph).

use dashmap::DashMap;
use tower_lsp::lsp_types::{Location, Position, Url};

use pkl_analyze::{ModuleGraph, SymbolKind};

use crate::document::Document;
use crate::rename::range_for_span;
use crate::uri::{module_uri_to_url, url_to_module_uri};

pub fn references_at(
    uri: &Url,
    doc: &Document,
    documents: &DashMap<Url, Document>,
    graph: &ModuleGraph,
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

    // Cross-module references — only meaningful for top-level user
    // symbols. Import aliases live in the importing file's namespace, so
    // there's no graph-wide "alias" to find.
    let is_import_alias = matches!(symbol.kind, SymbolKind::Import { .. });
    let is_top_level = symbol.container.is_none();
    if !symbol.origin.is_stdlib() && !is_import_alias && is_top_level {
        let module_uri = url_to_module_uri(uri);
        for (dep_uri, span) in graph.references_to(&module_uri, &symbol.name) {
            let Some(dep_url) = module_uri_to_url(&dep_uri) else {
                continue;
            };
            // Avoid double-counting if a module imports itself.
            if dep_url == *uri {
                continue;
            }
            let Some(range) = range_for_span(documents, graph, &dep_url, &dep_uri, span) else {
                continue;
            };
            locations.push(Location {
                uri: dep_url,
                range,
            });
        }
    }
    Some(locations)
}

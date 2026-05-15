//! `textDocument/definition` handler.

use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, Position, Range, Url};

use pkl_analyze::ModuleGraph;

use crate::document::Document;
use crate::uri::{module_uri_to_url, url_to_module_uri};

pub fn definition_at(
    uri: &Url,
    doc: &Document,
    graph: &ModuleGraph,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let offset = doc.position_to_offset(position);
    let module_uri = url_to_module_uri(uri);

    if let Some(target_url) = crate::import_paths::import_target_at(doc, offset, |local_name| {
        graph
            .imported_module(&module_uri, local_name)
            .and_then(|entry| module_uri_to_url(&entry.uri))
    }) {
        return Some(GotoDefinitionResponse::Scalar(Location {
            uri: target_url,
            range: Range::default(),
        }));
    }

    // 1. Member access — try cross-file first, then user-class members.
    if let Some(member) = doc.analysis.inference.member_ref_touching(offset) {
        // Is the receiver an import alias whose target we know?
        if let Some(receiver_sym_id) = doc
            .analysis
            .resolution
            .symbol_at_offset(member.receiver_span.start)
        {
            let receiver_sym = doc.analysis.resolution.symbol(receiver_sym_id);
            if matches!(receiver_sym.kind, pkl_analyze::SymbolKind::Import { .. }) {
                if let Some(imported) = graph.imported_module(&module_uri, &receiver_sym.name) {
                    if let Some(sym) = graph.lookup_top_level(imported, &member.member_name) {
                        if let Some(target_url) = module_uri_to_url(&imported.uri) {
                            return Some(GotoDefinitionResponse::Scalar(Location {
                                uri: target_url,
                                range: span_to_range_in(&imported.source, sym.name_span),
                            }));
                        }
                    }
                }
            }
        }
        // User-class member: jump within this file.
        if let Some(user_member) = member.user_member {
            let sym = doc.analysis.resolution.symbol(user_member);
            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: doc.span_to_range(sym.name_span),
            }));
        }
        // Stdlib member or unresolved — no source location.
        return None;
    }

    // 2. Plain symbol.
    let symbol_id = doc.analysis.resolution.symbol_at_offset(offset)?;
    let symbol = doc.analysis.resolution.symbol(symbol_id);
    if symbol.origin.is_stdlib() {
        return None;
    }

    // Import alias: jump into the imported file.
    if let pkl_analyze::SymbolKind::Import { .. } = symbol.kind {
        if let Some(imported) = graph.imported_module(&module_uri, &symbol.name) {
            if let Some(target_url) = module_uri_to_url(&imported.uri) {
                return Some(GotoDefinitionResponse::Scalar(Location {
                    uri: target_url,
                    range: Range::default(),
                }));
            }
        }
        // Fall through to the in-file definition (the alias' own span).
    }

    let location = Location {
        uri: uri.clone(),
        range: doc.span_to_range(symbol.name_span),
    };
    Some(GotoDefinitionResponse::Scalar(location))
}

/// Convert a byte-offset span to an LSP `Range` against the given source
/// string. Used when the target module wasn't opened by the editor — we
/// don't have a rope, so do the line/column math directly on the string.
fn span_to_range_in(source: &str, span: pkl_syntax::Span) -> Range {
    let start = position_in_source(source, span.start as usize);
    let end = position_in_source(source, span.end as usize);
    Range { start, end }
}

fn position_in_source(source: &str, byte_offset: usize) -> tower_lsp::lsp_types::Position {
    use tower_lsp::lsp_types::Position;
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
    // Convert chars on the line up to cap into UTF-16 code units.
    let line_text = &source[line_start..cap];
    let utf16_col: u32 = line_text.chars().map(|c| c.len_utf16() as u32).sum();
    Position {
        line,
        character: utf16_col,
    }
}

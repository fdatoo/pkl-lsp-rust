//! `workspace/symbol` handler.
//!
//! Aggregates every top-level user-defined symbol from every module in
//! the graph (open or transitively loaded) and filters by the requested
//! query string.

use pkl_analyze::{ModuleGraph, Origin};
use tower_lsp::lsp_types::{Location, SymbolInformation, SymbolKind as LspSymbolKind};

use crate::uri::module_uri_to_url;

pub fn workspace_symbols(graph: &ModuleGraph, query: &str) -> Vec<SymbolInformation> {
    let query_lower = query.to_ascii_lowercase();
    let mut out = Vec::new();
    for entry in graph.iter() {
        for sym in entry.analysis.resolution.symbols.iter() {
            if !matches!(sym.origin, Origin::User) {
                continue;
            }
            if sym.container.is_some() {
                // Skip class members — they're discoverable via the
                // containing class.
                continue;
            }
            if !query.is_empty() && !sym.name.to_ascii_lowercase().contains(&query_lower) {
                continue;
            }
            let Some(uri) = module_uri_to_url(&entry.uri) else {
                continue;
            };
            let kind = lsp_kind(sym.kind);
            #[allow(deprecated)]
            out.push(SymbolInformation {
                name: sym.name.clone(),
                kind,
                tags: None,
                deprecated: None,
                location: Location {
                    uri,
                    range: span_to_range_in(&entry.source, sym.name_span),
                },
                container_name: None,
            });
        }
    }
    out
}

fn lsp_kind(kind: pkl_analyze::SymbolKind) -> LspSymbolKind {
    use pkl_analyze::SymbolKind;
    match kind {
        SymbolKind::Class => LspSymbolKind::CLASS,
        SymbolKind::TypeAlias => LspSymbolKind::INTERFACE,
        SymbolKind::Property | SymbolKind::ObjectParameter => LspSymbolKind::PROPERTY,
        SymbolKind::Method => LspSymbolKind::FUNCTION,
        SymbolKind::Parameter | SymbolKind::LetBinding | SymbolKind::ForBinding => {
            LspSymbolKind::VARIABLE
        }
        SymbolKind::TypeParameter => LspSymbolKind::TYPE_PARAMETER,
        SymbolKind::Import { .. } | SymbolKind::Module => LspSymbolKind::MODULE,
    }
}

// Mirror the goto.rs helper — without a rope on hand we compute the
// position from the raw source string.
fn span_to_range_in(source: &str, span: pkl_syntax::Span) -> tower_lsp::lsp_types::Range {
    use tower_lsp::lsp_types::{Position, Range};
    let position = |offset: usize| -> Position {
        let cap = offset.min(source.len());
        let bytes = source.as_bytes();
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
    };
    Range {
        start: position(span.start as usize),
        end: position(span.end as usize),
    }
}

//! Hover handler implementation.
//!
//! The hover path consults three sources, in priority order:
//!
//! 1. **Member-access cross-file resolution**: cursor is on `imported.name`
//!    and the graph knows the imported module — render hover from that
//!    file's top-level symbol.
//! 2. **Member-access stdlib resolution**: cursor is on `expr.name` where
//!    `expr` has a known stdlib type. Render from the catalogue.
//! 3. **Plain symbol resolution**: cursor is on an identifier resolved by
//!    the per-file resolver (locals, parameters, declarations, imports).

use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Url};

use pkl_analyze::hover::{hover_markdown, member_hover_markdown};
use pkl_analyze::ModuleGraph;

use crate::document::Document;
use crate::uri::url_to_module_uri;

pub fn hover_at(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    position: Position,
) -> Option<Hover> {
    let offset = doc.position_to_offset(position);

    // 1. & 2. Member access — try cross-file first, then stdlib.
    if let Some(member) = doc.analysis.inference.member_ref_touching(offset) {
        // Cross-file: is the receiver an import alias?
        if let Some(imported_symbol) = imported_member_symbol(doc, graph, uri, member) {
            let value = hover_markdown(
                &imported_symbol.module.analysis.resolution,
                imported_symbol.symbol,
            );
            let range = doc.span_to_range(member.member_name_span);
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: Some(range),
            });
        }
        // Otherwise the stdlib renderer handles it (including the
        // user-class fallback and the unresolved case).
        let value = member_hover_markdown(member, &doc.analysis.resolution);
        let range = doc.span_to_range(member.member_name_span);
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(range),
        });
    }

    // 3. Plain symbol.
    let symbol_id = doc.analysis.resolution.symbol_at_offset(offset)?;
    let symbol = doc.analysis.resolution.symbol(symbol_id);
    let value = hover_markdown(&doc.analysis.resolution, symbol);

    let range = if symbol.origin.is_stdlib() {
        doc.analysis
            .resolution
            .references
            .iter()
            .find(|r| r.symbol == symbol_id && r.span.touches(offset))
            .map(|r| doc.span_to_range(r.span))
    } else {
        Some(doc.span_to_range(symbol.name_span))
    };

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range,
    })
}

/// Borrowed handle to a top-level symbol in another module.
struct ImportedSymbol<'a> {
    module: &'a pkl_analyze::ModuleEntry,
    symbol: &'a pkl_analyze::Symbol,
}

/// Try to resolve a member access whose receiver is an imported alias to
/// the corresponding top-level symbol in the imported file.
fn imported_member_symbol<'a>(
    doc: &Document,
    graph: &'a ModuleGraph,
    uri: &Url,
    member: &pkl_analyze::MemberRef,
) -> Option<ImportedSymbol<'a>> {
    // Receiver must resolve to an import symbol.
    let receiver_sym_id = doc
        .analysis
        .resolution
        .symbol_at_offset(member.receiver_span.start)?;
    let receiver_sym = doc.analysis.resolution.symbol(receiver_sym_id);
    if !matches!(receiver_sym.kind, pkl_analyze::SymbolKind::Import { .. }) {
        return None;
    }
    let module_uri = url_to_module_uri(uri);
    let imported = graph.imported_module(&module_uri, &receiver_sym.name)?;
    let sym = graph.lookup_top_level(imported, &member.member_name)?;
    Some(ImportedSymbol {
        module: imported,
        symbol: sym,
    })
}

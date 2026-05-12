//! `textDocument/documentSymbol` support.

use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind};

use pkl_syntax::ast::{ClassMember, Item, Module};

use crate::document::Document;

/// Build a hierarchical `DocumentSymbol` tree for a parsed module.
pub fn document_symbols(doc: &Document) -> Vec<DocumentSymbol> {
    let mut out = Vec::new();
    let module = &doc.parsed.module;
    if let Some(symbol) = module_symbol(doc, module) {
        out.push(symbol);
    }
    for item in &module.items {
        out.extend(item_symbol(doc, item));
    }
    out
}

#[allow(deprecated)] // `DocumentSymbol::deprecated` is deprecated but the type still requires it.
fn module_symbol(doc: &Document, module: &Module) -> Option<DocumentSymbol> {
    let header = module.header.as_ref()?;
    let name = header
        .name
        .as_ref()
        .map(|q| {
            q.segments
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(".")
        })
        .or_else(|| {
            header.clause.as_ref().map(|c| match c {
                pkl_syntax::ast::ExtendsAmendsClause::Amends { .. } => "amends".to_string(),
                pkl_syntax::ast::ExtendsAmendsClause::Extends { .. } => "extends".to_string(),
            })
        })?;
    let range = doc.span_to_range(header.span);
    Some(DocumentSymbol {
        name,
        detail: None,
        kind: SymbolKind::MODULE,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: None,
    })
}

#[allow(deprecated)]
fn item_symbol(doc: &Document, item: &Item) -> Option<DocumentSymbol> {
    match item {
        Item::Class(c) => {
            let range = doc.span_to_range(c.span);
            let selection_range = doc.span_to_range(c.name.span);
            let mut children = Vec::new();
            if let Some(body) = &c.body {
                for m in &body.members {
                    children.extend(member_symbol(doc, m));
                }
            }
            Some(DocumentSymbol {
                name: c.name.name.clone(),
                detail: None,
                kind: SymbolKind::CLASS,
                tags: None,
                deprecated: None,
                range,
                selection_range,
                children: if children.is_empty() {
                    None
                } else {
                    Some(children)
                },
            })
        }
        Item::TypeAlias(t) => Some(DocumentSymbol {
            name: t.name.name.clone(),
            detail: None,
            kind: SymbolKind::INTERFACE,
            tags: None,
            deprecated: None,
            range: doc.span_to_range(t.span),
            selection_range: doc.span_to_range(t.name.span),
            children: None,
        }),
        Item::Property(p) => Some(DocumentSymbol {
            name: p.name.name.clone(),
            detail: None,
            kind: SymbolKind::PROPERTY,
            tags: None,
            deprecated: None,
            range: doc.span_to_range(p.span),
            selection_range: doc.span_to_range(p.name.span),
            children: None,
        }),
        Item::Method(m) => Some(DocumentSymbol {
            name: m.name.name.clone(),
            detail: None,
            kind: SymbolKind::FUNCTION,
            tags: None,
            deprecated: None,
            range: doc.span_to_range(m.span),
            selection_range: doc.span_to_range(m.name.span),
            children: None,
        }),
        Item::Error(_) => None,
    }
}

#[allow(deprecated)]
fn member_symbol(doc: &Document, member: &ClassMember) -> Option<DocumentSymbol> {
    match member {
        ClassMember::Property(p) => Some(DocumentSymbol {
            name: p.name.name.clone(),
            detail: None,
            kind: SymbolKind::PROPERTY,
            tags: None,
            deprecated: None,
            range: doc.span_to_range(p.span),
            selection_range: doc.span_to_range(p.name.span),
            children: None,
        }),
        ClassMember::Method(m) => Some(DocumentSymbol {
            name: m.name.name.clone(),
            detail: None,
            kind: SymbolKind::METHOD,
            tags: None,
            deprecated: None,
            range: doc.span_to_range(m.span),
            selection_range: doc.span_to_range(m.name.span),
            children: None,
        }),
    }
}

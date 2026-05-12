//! `textDocument/documentSymbol` support.

use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind};

use pkl_syntax::cst::{
    self, ident_text, significant_span, token_span, AstNode, ClauseKind, Item, Module,
};

use crate::document::Document;

/// Build a hierarchical `DocumentSymbol` tree for a parsed module.
pub fn document_symbols(doc: &Document) -> Vec<DocumentSymbol> {
    let mut out = Vec::new();
    let module = doc.module();
    if let Some(symbol) = module_symbol(doc, &module) {
        out.push(symbol);
    }
    for item in module.items() {
        out.extend(item_symbol(doc, &item));
    }
    out
}

#[allow(deprecated)] // `DocumentSymbol::deprecated` is deprecated but the type still requires it.
fn module_symbol(doc: &Document, module: &Module) -> Option<DocumentSymbol> {
    let header = module.header()?;
    let name = header.name().map(|q| q.text_joined()).or_else(|| {
        header.clause().map(|c| match c.kind() {
            ClauseKind::Amends => "amends".to_string(),
            ClauseKind::Extends => "extends".to_string(),
        })
    })?;
    let span = significant_span(header.syntax());
    let range = doc.span_to_range(span);
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
            let range = doc.span_to_range(significant_span(c.syntax()));
            let name_tok = c.name()?;
            let selection_range = doc.span_to_range(token_span(&name_tok));
            let mut children = Vec::new();
            if let Some(body) = c.body() {
                for m in body.members() {
                    children.extend(member_symbol(doc, &m));
                }
            }
            Some(DocumentSymbol {
                name: ident_text(&name_tok),
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
        Item::TypeAlias(t) => {
            let name_tok = t.name()?;
            Some(DocumentSymbol {
                name: ident_text(&name_tok),
                detail: None,
                kind: SymbolKind::INTERFACE,
                tags: None,
                deprecated: None,
                range: doc.span_to_range(significant_span(t.syntax())),
                selection_range: doc.span_to_range(token_span(&name_tok)),
                children: None,
            })
        }
        Item::Property(p) => {
            let name_tok = p.name()?;
            Some(DocumentSymbol {
                name: ident_text(&name_tok),
                detail: None,
                kind: SymbolKind::PROPERTY,
                tags: None,
                deprecated: None,
                range: doc.span_to_range(significant_span(p.syntax())),
                selection_range: doc.span_to_range(token_span(&name_tok)),
                children: None,
            })
        }
        Item::Method(m) => {
            let name_tok = m.name()?;
            Some(DocumentSymbol {
                name: ident_text(&name_tok),
                detail: None,
                kind: SymbolKind::FUNCTION,
                tags: None,
                deprecated: None,
                range: doc.span_to_range(significant_span(m.syntax())),
                selection_range: doc.span_to_range(token_span(&name_tok)),
                children: None,
            })
        }
        Item::Error(_) => None,
    }
}

#[allow(deprecated)]
fn member_symbol(doc: &Document, member: &cst::ClassMember) -> Option<DocumentSymbol> {
    match member {
        cst::ClassMember::Property(p) => {
            let name_tok = p.name()?;
            Some(DocumentSymbol {
                name: ident_text(&name_tok),
                detail: None,
                kind: SymbolKind::PROPERTY,
                tags: None,
                deprecated: None,
                range: doc.span_to_range(significant_span(p.syntax())),
                selection_range: doc.span_to_range(token_span(&name_tok)),
                children: None,
            })
        }
        cst::ClassMember::Method(m) => {
            let name_tok = m.name()?;
            Some(DocumentSymbol {
                name: ident_text(&name_tok),
                detail: None,
                kind: SymbolKind::METHOD,
                tags: None,
                deprecated: None,
                range: doc.span_to_range(significant_span(m.syntax())),
                selection_range: doc.span_to_range(token_span(&name_tok)),
                children: None,
            })
        }
    }
}

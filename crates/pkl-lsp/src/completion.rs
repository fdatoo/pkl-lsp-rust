//! `textDocument/completion` handler.
//!
//! The provider distinguishes three contexts based on what precedes the
//! cursor:
//!
//! 1. **Member access** — the prior non-whitespace byte is `.`. Suggest
//!    members of the receiver expression's type (stdlib and user-defined).
//! 2. **Import path** — the cursor is inside the string literal of an
//!    `import` statement. Suggest configured namespace prefixes and the
//!    bundled `pkl:` modules.
//! 3. **Top-level identifier** — anything else. Suggest every symbol
//!    visible in the file plus Pkl keywords.

use pkl_analyze::infer::{stdlib_members_of, user_members_of};
use pkl_analyze::ModuleGraph;
use pkl_stdlib::MemberKind;
use tower_lsp::lsp_types::*;

use crate::document::Document;

const KEYWORDS: &[&str] = &[
    "abstract",
    "amends",
    "as",
    "class",
    "else",
    "extends",
    "external",
    "false",
    "fixed",
    "for",
    "function",
    "hidden",
    "if",
    "import",
    "in",
    "is",
    "let",
    "local",
    "module",
    "new",
    "null",
    "open",
    "out",
    "outer",
    "read",
    "super",
    "this",
    "throw",
    "trace",
    "true",
    "typealias",
    "unknown",
    "when",
];

pub fn complete_at(
    doc: &Document,
    graph: &ModuleGraph,
    position: Position,
) -> Option<CompletionResponse> {
    let offset = doc.position_to_offset(position);
    let text = doc.rope.to_string();
    let context = detect_context(&text, offset as usize);

    let items = match context {
        Context::Member { dot_pos } => member_completions(doc, dot_pos as u32),
        Context::ImportPath => import_path_completions(graph),
        Context::TopLevel => top_level_completions(doc),
    };
    Some(CompletionResponse::Array(items))
}

#[derive(Debug)]
enum Context {
    Member { dot_pos: usize },
    ImportPath,
    TopLevel,
}

/// Look at the bytes immediately before `offset` to classify the cursor.
fn detect_context(text: &str, offset: usize) -> Context {
    let bytes = text.as_bytes();
    let prefix = &bytes[..offset.min(bytes.len())];

    // Inside an import string?
    if is_inside_import_string(prefix) {
        return Context::ImportPath;
    }

    // Member access: walk backwards past whitespace, looking for `.` or `?.`
    let mut i = prefix.len();
    while i > 0 && prefix[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    if i > 0 && prefix[i - 1] == b'.' {
        // Note: if i >= 2 and prefix[i-2] == b'?', this is `?.` — same
        // receiver, just nullable access.
        return Context::Member { dot_pos: i - 1 };
    }

    Context::TopLevel
}

fn is_inside_import_string(prefix: &[u8]) -> bool {
    // Walk back, counting `"` boundaries. If we're inside a string that
    // sits on the same line as an `import` keyword, treat as import path.
    let mut line_start = prefix.len();
    while line_start > 0 && prefix[line_start - 1] != b'\n' {
        line_start -= 1;
    }
    let line = &prefix[line_start..];
    let quotes = line.iter().filter(|&&b| b == b'"').count();
    if quotes % 2 != 1 {
        return false;
    }
    // The line so far contains an odd number of quotes — we're inside a
    // string. Check for `import` earlier on the line.
    let line_text = std::str::from_utf8(line).unwrap_or("");
    line_text.contains("import")
}

// ----------------------------------------------------------------------
// Top-level completions: every user-defined symbol visible plus stdlib
// types, top-level constructors, and Pkl keywords.

fn top_level_completions(doc: &Document) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for sym in doc.analysis.resolution.symbols.iter() {
        if !seen.insert(sym.name.clone()) {
            continue;
        }
        items.push(symbol_completion(sym));
    }

    for kw in KEYWORDS {
        items.push(CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        });
    }

    items
}

fn symbol_completion(sym: &pkl_analyze::Symbol) -> CompletionItem {
    use pkl_analyze::SymbolKind;
    let kind = Some(match sym.kind {
        SymbolKind::Class => CompletionItemKind::CLASS,
        SymbolKind::TypeAlias => CompletionItemKind::INTERFACE,
        SymbolKind::Property | SymbolKind::ObjectParameter => CompletionItemKind::PROPERTY,
        SymbolKind::Method => CompletionItemKind::FUNCTION,
        SymbolKind::Parameter => CompletionItemKind::VARIABLE,
        SymbolKind::TypeParameter => CompletionItemKind::TYPE_PARAMETER,
        SymbolKind::LetBinding | SymbolKind::ForBinding => CompletionItemKind::VARIABLE,
        SymbolKind::Import { .. } => CompletionItemKind::MODULE,
        SymbolKind::Module => CompletionItemKind::MODULE,
    });
    CompletionItem {
        label: sym.name.clone(),
        kind,
        detail: sym.signature.clone(),
        documentation: sym.doc.as_ref().map(|d| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: d.clone(),
            })
        }),
        ..Default::default()
    }
}

// ----------------------------------------------------------------------
// Member completions: receiver type → members.

fn member_completions(doc: &Document, dot_pos: u32) -> Vec<CompletionItem> {
    let inference = &doc.analysis.inference;
    let Some(receiver_ty) = inference.type_ending_at(dot_pos) else {
        return Vec::new();
    };

    let mut items = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Stdlib members (inherited members included via the extends walk).
    for (m, owner) in stdlib_members_of(receiver_ty) {
        if !seen.insert(m.name.to_string()) {
            continue;
        }
        items.push(CompletionItem {
            label: m.name.to_string(),
            kind: Some(match m.kind {
                MemberKind::Property => CompletionItemKind::PROPERTY,
                MemberKind::Method => CompletionItemKind::METHOD,
            }),
            detail: Some(m.signature.to_string()),
            documentation: if m.doc.is_empty() {
                None
            } else {
                Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("from `{}`\n\n{}", owner.module, m.doc),
                }))
            },
            ..Default::default()
        });
    }

    // User-defined members.
    for sym_id in user_members_of(&doc.analysis.resolution, receiver_ty) {
        let sym = doc.analysis.resolution.symbol(sym_id);
        if !seen.insert(sym.name.clone()) {
            continue;
        }
        items.push(symbol_completion(sym));
    }

    items
}

// ----------------------------------------------------------------------
// Import-path completions: namespace prefixes and `pkl:` modules.

fn import_path_completions(_graph: &ModuleGraph) -> Vec<CompletionItem> {
    // Bundled `pkl:` modules. The graph knows about user-configured
    // namespaces too, but those aren't queryable through its current
    // API — they live on the loader.
    pkl_stdlib::vendored::MODULES
        .iter()
        .map(|m| CompletionItem {
            label: format!("pkl:{}", m.name),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some(m.module.to_string()),
            ..Default::default()
        })
        .collect()
}

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
use pkl_analyze::{ModuleGraph, WorkspaceIndex};
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
    workspace_index: &WorkspaceIndex,
    uri: &Url,
    position: Position,
) -> Option<CompletionResponse> {
    let offset = doc.position_to_offset(position);
    let text = doc.rope.to_string();
    let context = detect_context(&text, offset as usize);

    let items = match context {
        Context::Member { dot_pos } => member_completions(doc, dot_pos as u32),
        Context::ImportPath { quote_start } => import_path_completions(
            doc,
            graph,
            workspace_index,
            uri,
            quote_start,
            offset as usize,
        ),
        Context::TopLevel => top_level_completions(doc),
    };
    Some(CompletionResponse::Array(items))
}

#[derive(Debug)]
enum Context {
    Member {
        dot_pos: usize,
    },
    /// Cursor is inside an `import "..."` string literal. `quote_start`
    /// is the byte offset of the opening `"`, so the in-quotes prefix
    /// is `text[quote_start + 1 .. cursor]`.
    ImportPath {
        quote_start: usize,
    },
    TopLevel,
}

/// Look at the bytes immediately before `offset` to classify the cursor.
fn detect_context(text: &str, offset: usize) -> Context {
    let bytes = text.as_bytes();
    let prefix = &bytes[..offset.min(bytes.len())];

    // Inside an import string?
    if let Some(quote_start) = is_inside_import_string(prefix) {
        return Context::ImportPath { quote_start };
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

/// Returns `Some(absolute_offset_of_opening_quote)` if the cursor sits
/// inside an `import "..."` string literal on the current line.
fn is_inside_import_string(prefix: &[u8]) -> Option<usize> {
    // Walk back, counting `"` boundaries. If we're inside a string that
    // sits on the same line as an `import` keyword, treat as import path.
    let mut line_start = prefix.len();
    while line_start > 0 && prefix[line_start - 1] != b'\n' {
        line_start -= 1;
    }
    let line = &prefix[line_start..];
    let mut last_quote: Option<usize> = None;
    let mut quote_count = 0usize;
    for (i, &b) in line.iter().enumerate() {
        if b == b'"' {
            last_quote = Some(line_start + i);
            quote_count += 1;
        }
    }
    if quote_count % 2 != 1 {
        return None;
    }
    // The line so far contains an odd number of quotes — we're inside a
    // string. Check for `import` earlier on the line.
    let line_text = std::str::from_utf8(line).unwrap_or("");
    if !line_text.contains("import") {
        return None;
    }
    last_quote
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
// Import-path completions: `pkl:` stdlib modules and workspace files.

fn import_path_completions(
    doc: &Document,
    _graph: &ModuleGraph,
    workspace_index: &WorkspaceIndex,
    uri: &Url,
    quote_start: usize,
    cursor: usize,
) -> Vec<CompletionItem> {
    // Recover the in-quotes prefix and the LSP range covering it so the
    // editor can replace the user's partial text with the picked path.
    let text = doc.rope.to_string();
    let text_len = text.len();
    let in_quotes_start = (quote_start + 1).min(text_len);
    let cursor = cursor.min(text_len);
    let prefix = if cursor > in_quotes_start {
        &text[in_quotes_start..cursor]
    } else {
        ""
    };

    // (1) `pkl:` stdlib branch — unchanged behaviour, no text_edit so the
    // existing IDE UX is preserved.
    if prefix.starts_with("pkl:") {
        return stdlib_import_completions();
    }

    // (2) `package:` is out of scope; bail without offering anything.
    if prefix.starts_with("package:") {
        return Vec::new();
    }

    // (3) Anything else: workspace files. Resolve the current document's
    // filesystem path through the URI; non-file schemes have nothing to
    // offer.
    let Ok(current_path) = uri.to_file_path() else {
        // Still surface stdlib options as a fallback — the user might
        // be typing `pkl:` next.
        return stdlib_import_completions();
    };

    let range_start_pos = crate::document::byte_to_position(&doc.rope, in_quotes_start);
    let range_end_pos = crate::document::byte_to_position(&doc.rope, cursor);
    let replace_range = Range {
        start: range_start_pos,
        end: range_end_pos,
    };

    let workspace_candidates = workspace_index.completions_for(&current_path, prefix);
    let mut items: Vec<CompletionItem> = workspace_candidates
        .into_iter()
        .enumerate()
        .map(|(idx, c)| CompletionItem {
            label: c.display.clone(),
            kind: Some(CompletionItemKind::FILE),
            detail: Some(c.insert.clone()),
            filter_text: Some(c.insert.clone()),
            // `sort_text` is a lexicographic key; pad the index so 10
            // doesn't sort ahead of 2.
            sort_text: Some(format!("{:05}", idx)),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: replace_range,
                new_text: c.insert,
            })),
            ..Default::default()
        })
        .collect();

    // Always also offer the `pkl:` modules so they're discoverable from a
    // blank import — but rank them below workspace files.
    let stdlib_offset = items.len();
    for (i, item) in stdlib_import_completions().into_iter().enumerate() {
        items.push(CompletionItem {
            sort_text: Some(format!("z{:05}", stdlib_offset + i)),
            ..item
        });
    }

    items
}

fn stdlib_import_completions() -> Vec<CompletionItem> {
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

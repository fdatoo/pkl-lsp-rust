//! `textDocument/formatting` handler.
//!
//! Re-parses the document and asks `pkl_syntax::format::format_module`
//! to produce canonical text. The result is sent back as one whole-file
//! `TextEdit`. If the parse has errors we skip formatting to avoid
//! erasing user code.

use pkl_syntax::cst::{AstNode, Module};
use pkl_syntax::format::format_module;
use tower_lsp::lsp_types::{Position, Range, TextEdit};

use crate::document::Document;

pub fn format_document(doc: &Document) -> Option<Vec<TextEdit>> {
    if !doc.parsed.diagnostics.is_empty() {
        return None;
    }
    let module = Module::cast(doc.parsed.syntax())?;
    let formatted = format_module(&module);
    let current = doc.rope.to_string();
    if formatted == current {
        return Some(Vec::new());
    }
    // Compute the document's full range (end of last line).
    let last_line = doc.rope.len_lines().saturating_sub(1) as u32;
    let last_line_chars = doc
        .rope
        .line(last_line as usize)
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();
    Some(vec![TextEdit {
        range: Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: last_line,
                character: last_line_chars,
            },
        },
        new_text: formatted,
    }])
}

//! Per-document state held by the language server.
//!
//! Each open document owns a [`Rope`] for efficient incremental edits plus a
//! cached parse tree. The rope also gives us an O(log n) mapping between byte
//! offsets and LSP `Position` (line, UTF-16 column) values.

use ropey::Rope;
use tower_lsp::lsp_types::{Position, Range};

use pkl_analyze::{analyze, Analysis};
use pkl_syntax::parse;
use pkl_syntax::span::Span;
use pkl_syntax::ParseResult;

/// In-memory state for one open Pkl document.
pub struct Document {
    /// Rope of the current contents.
    pub rope: Rope,
    /// Latest parse result. Re-computed whenever the rope changes.
    pub parsed: ParseResult,
    /// Latest semantic analysis output. Re-computed alongside the parse.
    pub analysis: Analysis,
    /// Latest version number from `textDocument/didChange`.
    pub version: i32,
}

impl Document {
    pub fn new(text: String, version: i32) -> Self {
        let rope = Rope::from_str(&text);
        let parsed = parse(&text);
        let analysis = analyze(&parsed.module, parsed.diagnostics.clone());
        Self {
            rope,
            parsed,
            analysis,
            version,
        }
    }

    /// Replace the entire contents (sent by clients that don't support
    /// incremental sync). Reparses and re-analyses eagerly.
    pub fn replace(&mut self, text: String, version: i32) {
        self.rope = Rope::from_str(&text);
        self.reparse();
        self.version = version;
    }

    /// Apply an incremental edit. `range` is in LSP coordinates (line + UTF-16
    /// column). After the edit completes, the document is reparsed.
    pub fn apply_change(&mut self, range: Range, new_text: &str, version: i32) {
        let start = position_to_char_index(&self.rope, range.start);
        let end = position_to_char_index(&self.rope, range.end);
        self.rope.remove(start..end);
        self.rope.insert(start, new_text);
        self.reparse();
        self.version = version;
    }

    fn reparse(&mut self) {
        let text = self.rope.to_string();
        self.parsed = parse(&text);
        self.analysis = analyze(&self.parsed.module, self.parsed.diagnostics.clone());
    }

    /// Convert a byte-offset [`Span`] into an LSP [`Range`].
    pub fn span_to_range(&self, span: Span) -> Range {
        let start = byte_to_position(&self.rope, span.start as usize);
        let end = byte_to_position(&self.rope, span.end as usize);
        Range { start, end }
    }

    /// Convert an LSP `Position` (UTF-16 column) into a byte offset into the
    /// source text. Used by hover / goto-def to look up the symbol the
    /// cursor is on.
    pub fn position_to_offset(&self, pos: Position) -> u32 {
        let char_idx = position_to_char_index(&self.rope, pos);
        self.rope.char_to_byte(char_idx) as u32
    }
}

/// Convert a byte offset into the rope into an LSP `Position` (line, UTF-16
/// column). Clamps to end-of-document if the offset is out of range so we
/// never panic on a stale span.
pub fn byte_to_position(rope: &Rope, byte_offset: usize) -> Position {
    let byte_offset = byte_offset.min(rope.len_bytes());
    let char_idx = rope.byte_to_char(byte_offset);
    let line = rope.char_to_line(char_idx);
    let line_start_char = rope.line_to_char(line);
    let col_chars = char_idx - line_start_char;
    // Convert column-in-chars to column-in-UTF-16-code-units.
    let line_slice = rope.line(line);
    let mut utf16_col = 0u32;
    for c in line_slice.chars().take(col_chars) {
        utf16_col += c.len_utf16() as u32;
    }
    Position {
        line: line as u32,
        character: utf16_col,
    }
}

/// Convert an LSP `Position` (UTF-16) into a character index for rope edits.
fn position_to_char_index(rope: &Rope, pos: Position) -> usize {
    let line = (pos.line as usize).min(rope.len_lines().saturating_sub(1));
    let line_start_char = rope.line_to_char(line);
    let mut remaining_utf16 = pos.character as usize;
    let mut chars_in_line = 0usize;
    for c in rope.line(line).chars() {
        if remaining_utf16 == 0 {
            break;
        }
        let units = c.len_utf16();
        if units > remaining_utf16 {
            break;
        }
        remaining_utf16 -= units;
        chars_in_line += 1;
    }
    (line_start_char + chars_in_line).min(rope.len_chars())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_byte_offset_to_position() {
        let rope = Rope::from_str("ab\ncd\nef");
        let p = byte_to_position(&rope, 4);
        assert_eq!(p.line, 1);
        assert_eq!(p.character, 1);
    }

    #[test]
    fn utf16_column_counts_surrogate_pair_as_two() {
        // U+1F600 ("😀") is 4 bytes UTF-8, 2 UTF-16 units.
        let rope = Rope::from_str("😀x");
        let p = byte_to_position(&rope, "😀".len());
        assert_eq!(p.character, 2);
    }
}

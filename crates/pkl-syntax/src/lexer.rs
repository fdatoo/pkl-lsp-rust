//! Hand-rolled lexer for Pkl.
//!
//! The lexer streams [`Token`]s over a borrowed `&str`. It never allocates
//! per-token; the text slice is borrowed from the source. Trivia (whitespace,
//! newlines, comments) is emitted as its own tokens so the parser can choose
//! to skip or preserve them for tools like a formatter.
//!
//! Notes on intentional scope of this initial implementation:
//!
//! * Pkl string interpolation (`"\(expr)"`) is not yet re-entered: the entire
//!   `"..."` is emitted as a single `String` token whose interior is parsed
//!   later. The same applies to multi-line strings and custom-delimited
//!   strings (`#"..."#`).
//! * The lexer does not yet validate the textual content of numbers beyond
//!   their shape; the parser/analyzer is responsible for converting them.

use crate::kind::{keyword_from_ident, SyntaxKind};
use crate::span::Span;
use crate::token::Token;

/// Stateful character cursor over the input.
pub struct Lexer<'src> {
    src: &'src str,
    bytes: &'src [u8],
    pos: usize,
}

impl<'src> Lexer<'src> {
    pub fn new(src: &'src str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    /// Consume the lexer and return every token, including trivia, terminated
    /// by a single [`SyntaxKind::Eof`] token.
    pub fn collect_all(mut self) -> Vec<Token<'src>> {
        let mut out = Vec::with_capacity(self.src.len() / 4 + 1);
        while let Some(tok) = self.next_token() {
            let is_eof = tok.kind == SyntaxKind::Eof;
            out.push(tok);
            if is_eof {
                break;
            }
        }
        out
    }

    pub fn next_token(&mut self) -> Option<Token<'src>> {
        if self.pos > self.src.len() {
            return None;
        }
        if self.pos == self.src.len() {
            let span = Span::new(self.pos as u32, self.pos as u32);
            self.pos += 1; // step past so subsequent calls return None
            return Some(Token::new(SyntaxKind::Eof, span, ""));
        }

        let start = self.pos;
        let b = self.bytes[self.pos];

        let kind = match b {
            b' ' | b'\t' | b'\r' => self.lex_whitespace(),
            b'\n' => {
                self.pos += 1;
                SyntaxKind::Newline
            }
            b'/' => match self.peek_byte(1) {
                Some(b'/') => self.lex_line_comment(),
                Some(b'*') => self.lex_block_comment(),
                _ => {
                    self.pos += 1;
                    SyntaxKind::Slash
                }
            },
            // Custom-delimited strings `#"..."#`, `##"..."##`, etc.
            b'#' if matches!(self.peek_byte(1), Some(b'#') | Some(b'"')) => {
                self.lex_custom_string()
            }
            b'"' => {
                if self.starts_with(b"\"\"\"") {
                    self.lex_multiline_string()
                } else {
                    self.lex_string()
                }
            }
            b'`' => self.lex_quoted_ident(),
            b'0'..=b'9' => self.lex_number(),
            // identifiers — ASCII fast path; for non-ASCII we fall through to
            // the full Unicode branch below.
            b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' => self.lex_ident_or_keyword(),
            _ => {
                // Either a punctuation token or a non-ASCII identifier start.
                if let Some(kind) = self.try_lex_punct() {
                    kind
                } else if is_xid_start_unicode(self.src[self.pos..].chars().next().unwrap()) {
                    self.lex_ident_or_keyword()
                } else {
                    // Unknown byte — emit a single-byte Error token.
                    let ch_len = self.utf8_char_len();
                    self.pos += ch_len;
                    SyntaxKind::Error
                }
            }
        };

        let end = self.pos;
        let span = Span::new(start as u32, end as u32);
        let text = &self.src[start..end];
        Some(Token::new(kind, span, text))
    }

    // ------------------------------------------------------------------
    // Helpers

    #[inline]
    fn peek_byte(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    #[inline]
    fn starts_with(&self, needle: &[u8]) -> bool {
        self.bytes
            .get(self.pos..self.pos + needle.len())
            .is_some_and(|s| s == needle)
    }

    /// Number of bytes occupied by the UTF-8 character at `self.pos`.
    /// A stray continuation byte is treated as one byte so the lexer can
    /// emit a single-byte Error token for it instead of getting stuck.
    fn utf8_char_len(&self) -> usize {
        let b = self.bytes[self.pos];
        if b < 0xC0 {
            1
        } else if b < 0xE0 {
            2
        } else if b < 0xF0 {
            3
        } else {
            4
        }
    }

    // ------------------------------------------------------------------
    // Lex methods

    fn lex_whitespace(&mut self) -> SyntaxKind {
        while let Some(b) = self.peek_byte(0) {
            match b {
                b' ' | b'\t' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
        SyntaxKind::Whitespace
    }

    fn lex_line_comment(&mut self) -> SyntaxKind {
        // Already verified `//` at self.pos.
        // `///` is a doc comment.
        let is_doc =
            matches!(self.peek_byte(2), Some(b'/')) && !matches!(self.peek_byte(3), Some(b'/'));
        self.pos += 2;
        while let Some(b) = self.peek_byte(0) {
            if b == b'\n' {
                break;
            }
            self.pos += 1;
        }
        if is_doc {
            SyntaxKind::DocComment
        } else {
            SyntaxKind::LineComment
        }
    }

    fn lex_block_comment(&mut self) -> SyntaxKind {
        // Pkl block comments nest.
        self.pos += 2; // consume /*
        let mut depth = 1usize;
        while depth > 0 {
            match (self.peek_byte(0), self.peek_byte(1)) {
                (None, _) => return SyntaxKind::BlockComment,
                (Some(b'/'), Some(b'*')) => {
                    self.pos += 2;
                    depth += 1;
                }
                (Some(b'*'), Some(b'/')) => {
                    self.pos += 2;
                    depth -= 1;
                }
                _ => self.pos += 1,
            }
        }
        SyntaxKind::BlockComment
    }

    fn lex_string(&mut self) -> SyntaxKind {
        // self.bytes[self.pos] == '"'
        self.pos += 1;
        while let Some(b) = self.peek_byte(0) {
            match b {
                b'"' => {
                    self.pos += 1;
                    return SyntaxKind::String;
                }
                b'\\' => {
                    // Skip the escape's source bytes; we don't decode here.
                    // Handles \n, \t, \", \\, \( (interpolation start), etc.
                    self.pos += 1;
                    if self.pos < self.bytes.len() {
                        self.pos += self.utf8_char_len();
                    }
                }
                b'\n' => {
                    // Unterminated single-line string; stop at the newline.
                    return SyntaxKind::Error;
                }
                _ => self.pos += self.utf8_char_len(),
            }
        }
        SyntaxKind::Error
    }

    fn lex_multiline_string(&mut self) -> SyntaxKind {
        // self.bytes[self.pos..self.pos+3] == b"\"\"\""
        self.pos += 3;
        while self.pos < self.bytes.len() {
            if self.starts_with(b"\"\"\"") {
                self.pos += 3;
                // Pkl allows additional `"` after the closing triple in some
                // contexts; we leave that to the parser.
                return SyntaxKind::MultilineString;
            }
            match self.peek_byte(0).unwrap() {
                b'\\' => {
                    self.pos += 1;
                    if self.pos < self.bytes.len() {
                        self.pos += self.utf8_char_len();
                    }
                }
                _ => self.pos += self.utf8_char_len(),
            }
        }
        SyntaxKind::Error
    }

    /// Handle `#"..."#`, `##"..."##`, etc. The number of `#` on the opening
    /// must match the closing fence.
    fn lex_custom_string(&mut self) -> SyntaxKind {
        let mut hashes = 0usize;
        while matches!(self.peek_byte(hashes), Some(b'#')) {
            hashes += 1;
        }
        if !matches!(self.peek_byte(hashes), Some(b'"')) {
            // Lone `#` — treat as error (Pkl has no `#` operator at the
            // token level outside of custom strings).
            self.pos += hashes;
            return SyntaxKind::Error;
        }
        self.pos += hashes;
        let triple = self.starts_with(b"\"\"\"");
        if triple {
            self.pos += 3;
            let close_needle: Vec<u8> = b"\"\"\""
                .iter()
                .copied()
                .chain(std::iter::repeat(b'#').take(hashes))
                .collect();
            while self.pos < self.bytes.len() {
                if self.bytes[self.pos..].starts_with(&close_needle) {
                    self.pos += close_needle.len();
                    return SyntaxKind::MultilineString;
                }
                self.pos += self.utf8_char_len();
            }
            SyntaxKind::Error
        } else {
            self.pos += 1; // opening "
            let close_needle: Vec<u8> = std::iter::once(b'"')
                .chain(std::iter::repeat(b'#').take(hashes))
                .collect();
            while self.pos < self.bytes.len() {
                if self.bytes[self.pos..].starts_with(&close_needle) {
                    self.pos += close_needle.len();
                    return SyntaxKind::String;
                }
                if self.bytes[self.pos] == b'\n' {
                    return SyntaxKind::Error;
                }
                self.pos += self.utf8_char_len();
            }
            SyntaxKind::Error
        }
    }

    fn lex_quoted_ident(&mut self) -> SyntaxKind {
        // self.bytes[self.pos] == '`'
        self.pos += 1;
        while let Some(b) = self.peek_byte(0) {
            self.pos += 1;
            if b == b'`' {
                return SyntaxKind::QuotedIdent;
            }
            if b == b'\n' {
                return SyntaxKind::Error;
            }
        }
        SyntaxKind::Error
    }

    fn lex_number(&mut self) -> SyntaxKind {
        // Handle 0x, 0b, 0o prefixes.
        if self.bytes[self.pos] == b'0' {
            match self.peek_byte(1) {
                Some(b'x') | Some(b'X') => {
                    self.pos += 2;
                    while let Some(b) = self.peek_byte(0) {
                        if b.is_ascii_hexdigit() || b == b'_' {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    return SyntaxKind::HexNumber;
                }
                Some(b'b') | Some(b'B') => {
                    self.pos += 2;
                    while let Some(b) = self.peek_byte(0) {
                        if matches!(b, b'0' | b'1' | b'_') {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    return SyntaxKind::BinNumber;
                }
                Some(b'o') | Some(b'O') => {
                    self.pos += 2;
                    while let Some(b) = self.peek_byte(0) {
                        if matches!(b, b'0'..=b'7' | b'_') {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    return SyntaxKind::OctNumber;
                }
                _ => {}
            }
        }

        // Decimal integer part.
        while let Some(b) = self.peek_byte(0) {
            if b.is_ascii_digit() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }

        let mut is_float = false;

        // Fractional part: only consume `.` if followed by a digit so we
        // don't gobble `foo.bar` member access.
        if self.peek_byte(0) == Some(b'.') && matches!(self.peek_byte(1), Some(b'0'..=b'9')) {
            is_float = true;
            self.pos += 1;
            while let Some(b) = self.peek_byte(0) {
                if b.is_ascii_digit() || b == b'_' {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }

        // Exponent.
        if matches!(self.peek_byte(0), Some(b'e' | b'E')) {
            let mut look = 1usize;
            if matches!(self.peek_byte(look), Some(b'+' | b'-')) {
                look += 1;
            }
            if matches!(self.peek_byte(look), Some(b'0'..=b'9')) {
                is_float = true;
                self.pos += look;
                while let Some(b) = self.peek_byte(0) {
                    if b.is_ascii_digit() || b == b'_' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
            }
        }

        if is_float {
            SyntaxKind::FloatNumber
        } else {
            SyntaxKind::IntNumber
        }
    }

    fn lex_ident_or_keyword(&mut self) -> SyntaxKind {
        let start = self.pos;
        // First character was already validated.
        self.pos += self.utf8_char_len();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b < 0x80 {
                if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                    self.pos += 1;
                } else {
                    break;
                }
            } else {
                let ch = self.src[self.pos..].chars().next().unwrap();
                if is_xid_continue_unicode(ch) {
                    self.pos += ch.len_utf8();
                } else {
                    break;
                }
            }
        }
        let text = &self.src[start..self.pos];
        if let Some(kw) = keyword_from_ident(text) {
            return kw;
        }
        // Multi-character keywords with punctuation: `import*`, `read?`, `read*`.
        // The base `import` / `read` were already matched above; we only get
        // here for the plain ident form.
        SyntaxKind::Ident
    }

    /// Attempt to lex a single punctuation token. Returns `None` if the
    /// current byte does not begin any known operator. Always advances `pos`
    /// when it returns `Some`.
    fn try_lex_punct(&mut self) -> Option<SyntaxKind> {
        let b = self.bytes[self.pos];
        let b1 = self.peek_byte(1);
        let b2 = self.peek_byte(2);

        // Longest match first.
        let (kind, consume) = match (b, b1, b2) {
            (b'.', Some(b'.'), Some(b'.')) => (SyntaxKind::Ellipsis, 3),
            (b'[', Some(b'['), _) => (SyntaxKind::LDoubleBracket, 2),
            (b']', Some(b']'), _) => (SyntaxKind::RDoubleBracket, 2),
            (b'.', Some(b'.'), _) => (SyntaxKind::DotDot, 2),
            (b':', Some(b':'), _) => (SyntaxKind::ColonColon, 2),
            (b'-', Some(b'>'), _) => (SyntaxKind::Arrow, 2),
            (b'=', Some(b'>'), _) => (SyntaxKind::FatArrow, 2),
            (b'=', Some(b'='), _) => (SyntaxKind::EqEq, 2),
            (b'!', Some(b'='), _) => (SyntaxKind::BangEq, 2),
            (b'<', Some(b'='), _) => (SyntaxKind::LtEq, 2),
            (b'>', Some(b'='), _) => (SyntaxKind::GtEq, 2),
            (b'?', Some(b'.'), _) => (SyntaxKind::QuestionDot, 2),
            (b'?', Some(b'?'), _) => (SyntaxKind::QuestionQuestion, 2),
            (b'|', Some(b'>'), _) => (SyntaxKind::PipeGt, 2),
            (b'*', Some(b'*'), _) => (SyntaxKind::StarStar, 2),
            (b'{', _, _) => (SyntaxKind::LBrace, 1),
            (b'}', _, _) => (SyntaxKind::RBrace, 1),
            (b'(', _, _) => (SyntaxKind::LParen, 1),
            (b')', _, _) => (SyntaxKind::RParen, 1),
            (b'[', _, _) => (SyntaxKind::LBracket, 1),
            (b']', _, _) => (SyntaxKind::RBracket, 1),
            (b',', _, _) => (SyntaxKind::Comma, 1),
            (b'.', _, _) => (SyntaxKind::Dot, 1),
            (b';', _, _) => (SyntaxKind::Semicolon, 1),
            (b':', _, _) => (SyntaxKind::Colon, 1),
            (b'@', _, _) => (SyntaxKind::At, 1),
            (b'|', _, _) => (SyntaxKind::Pipe, 1),
            (b'&', _, _) => (SyntaxKind::Amp, 1),
            (b'?', _, _) => (SyntaxKind::Question, 1),
            (b'=', _, _) => (SyntaxKind::Eq, 1),
            (b'!', _, _) => (SyntaxKind::Bang, 1),
            (b'<', _, _) => (SyntaxKind::Lt, 1),
            (b'>', _, _) => (SyntaxKind::Gt, 1),
            (b'+', _, _) => (SyntaxKind::Plus, 1),
            (b'-', _, _) => (SyntaxKind::Minus, 1),
            (b'*', _, _) => (SyntaxKind::Star, 1),
            (b'%', _, _) => (SyntaxKind::Percent, 1),
            _ => return None,
        };
        self.pos += consume;
        Some(kind)
    }
}

// ----------------------------------------------------------------------
// Unicode identifier helpers.
//
// Pkl follows the standard `XID_Start` / `XID_Continue` classification, plus
// the extra ASCII characters `_` and `$`. The `unicode-ident` crate ships
// the canonical tables.

#[inline]
fn is_xid_start_unicode(c: char) -> bool {
    c == '_' || c == '$' || unicode_ident::is_xid_start(c)
}

#[inline]
fn is_xid_continue_unicode(c: char) -> bool {
    c == '_' || c == '$' || unicode_ident::is_xid_continue(c)
}

// Post-process: recognise `import*`, `read?`, `read*` as compound keywords.
//
// These don't fit cleanly inside the per-byte lexer (because their second
// half is not an identifier character) so we fix them up after the fact. The
// alternative — recognising them inline — bloats the ident path with three
// special cases.
pub fn fix_compound_keywords<'src>(tokens: &mut Vec<Token<'src>>) {
    let mut i = 0;
    while i + 1 < tokens.len() {
        let (a, b) = (&tokens[i], &tokens[i + 1]);
        let merged = match (a.kind, b.kind) {
            (SyntaxKind::ImportKw, SyntaxKind::Star) if a.span.end == b.span.start => {
                Some(SyntaxKind::ImportGlobKw)
            }
            (SyntaxKind::ReadKw, SyntaxKind::Question) if a.span.end == b.span.start => {
                Some(SyntaxKind::ReadOrNullKw)
            }
            (SyntaxKind::ReadKw, SyntaxKind::Star) if a.span.end == b.span.start => {
                Some(SyntaxKind::ReadGlobKw)
            }
            _ => None,
        };
        if let Some(kind) = merged {
            let new_span = a.span.join(b.span);
            let new_text = &tokens[i].text[..0]; // placeholder, replaced below
                                                 // Build the replacement text by extending across the join.
                                                 // Both a.text and b.text are slices into the same source; we
                                                 // reconstruct a single slice via raw pointer arithmetic so we
                                                 // don't have to thread the original `&str` here.
            let merged_text = unsafe {
                let ptr = a.text.as_ptr();
                let len = (b.span.end - a.span.start) as usize;
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len))
            };
            let _ = new_text;
            tokens[i] = Token::new(kind, new_span, merged_text);
            tokens.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

/// Convenience: tokenize an entire source buffer, applying compound-keyword
/// post-processing. Trivia is preserved.
pub fn tokenize(src: &str) -> Vec<Token<'_>> {
    let mut tokens = Lexer::new(src).collect_all();
    fix_compound_keywords(&mut tokens);
    tokens
}

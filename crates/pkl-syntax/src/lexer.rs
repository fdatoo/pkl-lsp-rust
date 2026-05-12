//! Hand-rolled lexer for Pkl.
//!
//! The lexer streams [`Token`]s over a borrowed `&str`. It never allocates
//! per-token; the text slice is borrowed from the source. Trivia (whitespace,
//! newlines, comments) is emitted as its own tokens so the parser can choose
//! to skip or preserve them for tools like a formatter.
//!
//! Strings are handled in two modes:
//!
//! * **Non-interpolated** strings (no `\(` at the appropriate hash level)
//!   are still emitted as a single [`SyntaxKind::String`] /
//!   [`SyntaxKind::MultilineString`] token. This keeps the token shape stable
//!   for downstream consumers that don't care about interpolation.
//! * **Interpolated** strings are decomposed: the lexer pushes a `String`
//!   mode and emits a [`SyntaxKind::StringQuoteOpen`] / `StringPart` /
//!   [`SyntaxKind::InterpolationStart`] / ... / [`SyntaxKind::InterpolationEnd`] /
//!   [`SyntaxKind::StringQuoteClose`] sequence. Inside an `InterpolationStart`
//!   ... `InterpolationEnd` region the lexer pushes an `Interpolation` mode
//!   that lexes normal Pkl tokens, tracking paren depth so a `)` only closes
//!   the hole when it's at depth zero (and, for custom-delimited strings,
//!   followed by the right number of `#` characters).

use crate::kind::{keyword_from_ident, SyntaxKind};
use crate::span::Span;
use crate::token::Token;

/// Stateful character cursor over the input.
pub struct Lexer<'src> {
    src: &'src str,
    bytes: &'src [u8],
    pos: usize,
    modes: Vec<Mode>,
}

/// Top-of-stack mode determines what `next_token` does. The lexer starts in
/// `Normal` and pushes / pops these on string/interpolation boundaries.
#[derive(Copy, Clone, Debug)]
enum Mode {
    /// Default Pkl token stream.
    Normal,
    /// Inside a string literal. We have already emitted the opening quote
    /// token and are now scanning string parts and interpolation markers.
    String {
        /// Number of leading `#` characters on the delimiter, or zero for
        /// the bare `"..."` / `"""..."""` forms.
        hashes: u32,
        /// True for triple-quoted multi-line strings.
        multiline: bool,
    },
    /// Inside an `\(...)` interpolation hole. We lex normal Pkl tokens but
    /// track paren depth so that only a top-level `)` ends the hole and
    /// pops back to the surrounding `String` mode.
    Interpolation { paren_depth: u32 },
}

impl<'src> Lexer<'src> {
    pub fn new(src: &'src str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            modes: vec![Mode::Normal],
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

        // String mode: scan a StringPart / interpolation marker / closing
        // quote. Handle this before the EOF sentinel so an unterminated
        // string still reports the partial part.
        if let Some(&Mode::String { hashes, multiline }) = self.modes.last() {
            return self.next_in_string(hashes, multiline);
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
                return Some(self.lex_custom_string_open(start));
            }
            b'"' => {
                if self.starts_with(b"\"\"\"") {
                    return Some(self.lex_multiline_string_open(start));
                } else {
                    return Some(self.lex_string_open(start));
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

    /// True if there is an unescaped `\(...)` interpolation hole in the
    /// single-line string starting at `start` (which is the position of the
    /// opening `"`). The scan stops at the first newline or closing `"`. Used
    /// to decide whether to emit the whole string as a single token or to
    /// decompose it.
    ///
    /// `hashes == 0` for the bare `"..."` form; for `#"..."#` and friends
    /// the caller has already consumed the leading `#`s and `start` is at
    /// the `"`.
    fn single_line_has_interpolation(&self, start: usize, hashes: usize) -> bool {
        let bytes = self.bytes;
        let mut i = start + 1; // past opening "
        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    if string_close_matches(bytes, i, hashes) {
                        return false;
                    }
                    i += 1;
                }
                b'\n' => return false,
                b'\\' => {
                    if hashes == 0 {
                        // Plain `"..."`: `\(` always starts a hole.
                        if matches!(bytes.get(i + 1), Some(b'(')) {
                            return true;
                        }
                        // Skip the escape's source bytes.
                        if i + 1 < bytes.len() {
                            i += 2;
                        } else {
                            i += 1;
                        }
                    } else {
                        // `#"..."#`: `\` is literal unless followed by N `#`s
                        // then a metacharacter. `\<N hashes>(` opens a hole.
                        if bytes.get(i + 1..i + 1 + hashes) == Some(&vec![b'#'; hashes][..])
                            && matches!(bytes.get(i + 1 + hashes), Some(b'('))
                        {
                            return true;
                        }
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
        false
    }

    fn multiline_has_interpolation(&self, start: usize, hashes: usize) -> bool {
        let bytes = self.bytes;
        let mut i = start + 3; // past opening """
        while i < bytes.len() {
            if i + 3 <= bytes.len() && &bytes[i..i + 3] == b"\"\"\"" {
                if multiline_close_matches(bytes, i, hashes) {
                    return false;
                }
                i += 1;
                continue;
            }
            match bytes[i] {
                b'\\' => {
                    if hashes == 0 {
                        if matches!(bytes.get(i + 1), Some(b'(')) {
                            return true;
                        }
                        if i + 1 < bytes.len() {
                            i += 2;
                        } else {
                            i += 1;
                        }
                    } else {
                        if bytes.get(i + 1..i + 1 + hashes) == Some(&vec![b'#'; hashes][..])
                            && matches!(bytes.get(i + 1 + hashes), Some(b'('))
                        {
                            return true;
                        }
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
        false
    }

    /// Open a plain `"..."` string. Either emits it as a single
    /// [`SyntaxKind::String`] token (legacy path for strings without
    /// interpolation) or emits the opening quote and pushes a `String`
    /// mode so subsequent calls return the parts and the interpolation
    /// markers.
    fn lex_string_open(&mut self, start: usize) -> Token<'src> {
        debug_assert_eq!(self.bytes[start], b'"');
        if self.single_line_has_interpolation(start, 0) {
            // Emit just the opening quote and switch to String mode.
            self.pos = start + 1;
            self.modes.push(Mode::String {
                hashes: 0,
                multiline: false,
            });
            let span = Span::new(start as u32, self.pos as u32);
            return Token::new(
                SyntaxKind::StringQuoteOpen,
                span,
                &self.src[start..self.pos],
            );
        }
        // Legacy single-token path: behave exactly as before, including
        // recovery to a newline on unterminated strings.
        self.pos = start + 1;
        let kind = self.lex_string_body();
        let span = Span::new(start as u32, self.pos as u32);
        Token::new(kind, span, &self.src[start..self.pos])
    }

    /// Body-scan loop for a non-interpolated single-line string. Caller has
    /// already advanced past the opening `"`. Returns the closing kind
    /// (`String` on success or `Error` on unterminated input).
    fn lex_string_body(&mut self) -> SyntaxKind {
        while let Some(b) = self.peek_byte(0) {
            match b {
                b'"' => {
                    self.pos += 1;
                    return SyntaxKind::String;
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos < self.bytes.len() {
                        self.pos += self.utf8_char_len();
                    }
                }
                b'\n' => return SyntaxKind::Error,
                _ => self.pos += self.utf8_char_len(),
            }
        }
        SyntaxKind::Error
    }

    fn lex_multiline_string_open(&mut self, start: usize) -> Token<'src> {
        debug_assert!(self.bytes[start..].starts_with(b"\"\"\""));
        if self.multiline_has_interpolation(start, 0) {
            self.pos = start + 3;
            self.modes.push(Mode::String {
                hashes: 0,
                multiline: true,
            });
            let span = Span::new(start as u32, self.pos as u32);
            return Token::new(
                SyntaxKind::StringQuoteOpen,
                span,
                &self.src[start..self.pos],
            );
        }
        self.pos = start + 3;
        let kind = self.lex_multiline_string_body();
        let span = Span::new(start as u32, self.pos as u32);
        Token::new(kind, span, &self.src[start..self.pos])
    }

    fn lex_multiline_string_body(&mut self) -> SyntaxKind {
        while self.pos < self.bytes.len() {
            if self.starts_with(b"\"\"\"") {
                self.pos += 3;
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
    fn lex_custom_string_open(&mut self, start: usize) -> Token<'src> {
        // Count leading hashes.
        let mut hashes = 0usize;
        while matches!(self.peek_byte(hashes), Some(b'#')) {
            hashes += 1;
        }
        if !matches!(self.peek_byte(hashes), Some(b'"')) {
            // Lone `#` — treat as error (Pkl has no `#` operator outside of
            // custom strings).
            self.pos += hashes;
            let span = Span::new(start as u32, self.pos as u32);
            return Token::new(SyntaxKind::Error, span, &self.src[start..self.pos]);
        }
        let quote_pos = start + hashes;
        let triple = self.bytes[quote_pos..].starts_with(b"\"\"\"");
        if triple {
            if self.multiline_has_interpolation(quote_pos, hashes) {
                self.pos = quote_pos + 3;
                self.modes.push(Mode::String {
                    hashes: hashes as u32,
                    multiline: true,
                });
                let span = Span::new(start as u32, self.pos as u32);
                return Token::new(
                    SyntaxKind::StringQuoteOpen,
                    span,
                    &self.src[start..self.pos],
                );
            }
            // Legacy single-token: scan until matching `"""` + N hashes.
            self.pos = quote_pos + 3;
            let kind = self.lex_custom_multiline_body(hashes);
            let span = Span::new(start as u32, self.pos as u32);
            Token::new(kind, span, &self.src[start..self.pos])
        } else {
            if self.single_line_has_interpolation(quote_pos, hashes) {
                self.pos = quote_pos + 1;
                self.modes.push(Mode::String {
                    hashes: hashes as u32,
                    multiline: false,
                });
                let span = Span::new(start as u32, self.pos as u32);
                return Token::new(
                    SyntaxKind::StringQuoteOpen,
                    span,
                    &self.src[start..self.pos],
                );
            }
            self.pos = quote_pos + 1;
            let kind = self.lex_custom_single_body(hashes);
            let span = Span::new(start as u32, self.pos as u32);
            Token::new(kind, span, &self.src[start..self.pos])
        }
    }

    fn lex_custom_single_body(&mut self, hashes: usize) -> SyntaxKind {
        while self.pos < self.bytes.len() {
            if string_close_matches(self.bytes, self.pos, hashes) {
                self.pos += 1 + hashes;
                return SyntaxKind::String;
            }
            if self.bytes[self.pos] == b'\n' {
                return SyntaxKind::Error;
            }
            self.pos += self.utf8_char_len();
        }
        SyntaxKind::Error
    }

    fn lex_custom_multiline_body(&mut self, hashes: usize) -> SyntaxKind {
        while self.pos < self.bytes.len() {
            if multiline_close_matches(self.bytes, self.pos, hashes) {
                self.pos += 3 + hashes;
                return SyntaxKind::MultilineString;
            }
            self.pos += self.utf8_char_len();
        }
        SyntaxKind::Error
    }

    // ------------------------------------------------------------------
    // String-mode dispatch
    //
    // Once we're inside a `Mode::String { ... }`, every `next_token` call
    // emits one of:
    // * `StringPart` / `MultilineStringPart` — literal text between markers,
    // * `InterpolationStart` — `\(` (or `\<N hashes>(`),
    // * `StringQuoteClose` — closing `"` (or `"""`, optionally followed by N hashes).

    fn next_in_string(&mut self, hashes: u32, multiline: bool) -> Option<Token<'src>> {
        let start = self.pos;
        let hashes = hashes as usize;

        // Check for closing fence first.
        if multiline {
            if multiline_close_matches(self.bytes, self.pos, hashes) {
                self.pos += 3 + hashes;
                self.modes.pop();
                let span = Span::new(start as u32, self.pos as u32);
                return Some(Token::new(
                    SyntaxKind::StringQuoteClose,
                    span,
                    &self.src[start..self.pos],
                ));
            }
        } else if string_close_matches(self.bytes, self.pos, hashes) {
            self.pos += 1 + hashes;
            self.modes.pop();
            let span = Span::new(start as u32, self.pos as u32);
            return Some(Token::new(
                SyntaxKind::StringQuoteClose,
                span,
                &self.src[start..self.pos],
            ));
        }

        // Interpolation start?
        if interpolation_start_matches(self.bytes, self.pos, hashes) {
            let len = 2 + hashes; // `\` + N hashes + `(`
            self.pos += len;
            self.modes.push(Mode::Interpolation { paren_depth: 0 });
            let span = Span::new(start as u32, self.pos as u32);
            return Some(Token::new(
                SyntaxKind::InterpolationStart,
                span,
                &self.src[start..self.pos],
            ));
        }

        // End of input inside a string — bail out so the parser can recover.
        if self.pos >= self.bytes.len() {
            self.modes.pop();
            let span = Span::new(self.pos as u32, self.pos as u32);
            self.pos += 1;
            return Some(Token::new(SyntaxKind::Eof, span, ""));
        }

        // Otherwise we're inside a literal-text run. Consume up to the next
        // marker (or, for single-line strings, the end of line which marks
        // an unterminated string).
        let part_kind = if multiline {
            SyntaxKind::MultilineStringPart
        } else {
            SyntaxKind::StringPart
        };

        while self.pos < self.bytes.len() {
            if multiline {
                if multiline_close_matches(self.bytes, self.pos, hashes) {
                    break;
                }
            } else if string_close_matches(self.bytes, self.pos, hashes) {
                break;
            }
            if interpolation_start_matches(self.bytes, self.pos, hashes) {
                break;
            }
            let b = self.bytes[self.pos];
            if !multiline && b == b'\n' {
                // Unterminated single-line string. Pop the mode and emit a
                // part covering whatever we've consumed (plus the newline?).
                // We stop before the newline so the next token is a regular
                // Newline trivia in Normal mode.
                self.modes.pop();
                break;
            }
            if b == b'\\' {
                // Skip the escape. In `#"..."#` strings, `\` is only an
                // escape when followed by N hashes then a metacharacter; we
                // already checked that this isn't an `InterpolationStart`
                // above, so consume the backslash and let the next iteration
                // handle the rest.
                if hashes == 0 {
                    // `\` + next char (or just `\` at end of input).
                    self.pos += 1;
                    if self.pos < self.bytes.len() {
                        self.pos += self.utf8_char_len();
                    }
                    continue;
                } else {
                    self.pos += 1;
                    continue;
                }
            }
            self.pos += self.utf8_char_len();
        }

        if self.pos == start {
            // We made no progress — this shouldn't happen but emit an Eof to
            // avoid an infinite loop.
            self.modes.pop();
            let span = Span::new(self.pos as u32, self.pos as u32);
            self.pos += 1;
            return Some(Token::new(SyntaxKind::Eof, span, ""));
        }

        let span = Span::new(start as u32, self.pos as u32);
        Some(Token::new(part_kind, span, &self.src[start..self.pos]))
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
    ///
    /// This is also responsible for tracking paren depth inside an enclosing
    /// `Interpolation` mode: `(` bumps depth, `)` at depth>0 decrements,
    /// `)` at depth==0 emits `InterpolationEnd` and pops the mode. Pkl's
    /// custom-delimited strings use only the opening `\#(` marker; the
    /// closing fence is the plain `)` regardless of hash count.
    fn try_lex_punct(&mut self) -> Option<SyntaxKind> {
        // First, look for an `InterpolationEnd` if we're inside a hole and
        // sitting on a `)` at depth 0.
        if let Some(&Mode::Interpolation { paren_depth }) = self.modes.last() {
            if paren_depth == 0 && self.peek_byte(0) == Some(b')') {
                self.pos += 1;
                self.modes.pop();
                return Some(SyntaxKind::InterpolationEnd);
            }
        }

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
        // Track paren depth for an enclosing Interpolation mode.
        if let Some(Mode::Interpolation { paren_depth }) = self.modes.last_mut() {
            match kind {
                SyntaxKind::LParen => *paren_depth += 1,
                SyntaxKind::RParen if *paren_depth > 0 => *paren_depth -= 1,
                _ => {}
            }
        }
        Some(kind)
    }
}

// ----------------------------------------------------------------------
// String-fence matching helpers (free fns for use both inside and outside
// the lexer's borrow checker scope).

/// True when `bytes[at..]` starts with a single-line closing fence: `"`
/// followed by exactly `hashes` `#` characters. Used both for the legacy
/// scan and for the mode-driven scan.
fn string_close_matches(bytes: &[u8], at: usize, hashes: usize) -> bool {
    if at >= bytes.len() || bytes[at] != b'"' {
        return false;
    }
    if hashes == 0 {
        return true;
    }
    let tail = &bytes[at + 1..];
    if tail.len() < hashes {
        return false;
    }
    tail[..hashes].iter().all(|&b| b == b'#')
}

fn multiline_close_matches(bytes: &[u8], at: usize, hashes: usize) -> bool {
    if at + 3 > bytes.len() || &bytes[at..at + 3] != b"\"\"\"" {
        return false;
    }
    if hashes == 0 {
        return true;
    }
    let tail = &bytes[at + 3..];
    if tail.len() < hashes {
        return false;
    }
    tail[..hashes].iter().all(|&b| b == b'#')
}

/// True when `bytes[at..]` is the start of an interpolation hole for a
/// string with the given hash count: `\` + N `#`s + `(`.
fn interpolation_start_matches(bytes: &[u8], at: usize, hashes: usize) -> bool {
    if at >= bytes.len() || bytes[at] != b'\\' {
        return false;
    }
    if hashes == 0 {
        return matches!(bytes.get(at + 1), Some(b'('));
    }
    let tail = &bytes[at + 1..];
    if tail.len() < hashes + 1 {
        return false;
    }
    tail[..hashes].iter().all(|&b| b == b'#') && tail[hashes] == b'('
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
            // Build the replacement text by extending across the join.
            // Both a.text and b.text are slices into the same source; we
            // reconstruct a single slice via raw pointer arithmetic so we
            // don't have to thread the original `&str` here.
            let merged_text = unsafe {
                let ptr = a.text.as_ptr();
                let len = (b.span.end - a.span.start) as usize;
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len))
            };
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

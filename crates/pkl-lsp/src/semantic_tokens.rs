//! `textDocument/semanticTokens/full` support.
//!
//! Walks the token stream and emits one semantic token per non-trivia
//! token using a tightly-scoped legend so editors can colour Pkl in a
//! way that matches the language's idioms (keywords, strings, numbers,
//! identifiers split into property/function/parameter via the resolver).

use pkl_analyze::SymbolKind;
use pkl_syntax::token::Token;
use pkl_syntax::SyntaxKind;
use tower_lsp::lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensLegend,
};

use crate::document::{byte_to_position, Document};

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::KEYWORD,
            SemanticTokenType::COMMENT,
            SemanticTokenType::STRING,
            SemanticTokenType::NUMBER,
            SemanticTokenType::OPERATOR,
            SemanticTokenType::TYPE,
            SemanticTokenType::CLASS,
            SemanticTokenType::INTERFACE,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::METHOD,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::PARAMETER,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::NAMESPACE,
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DECLARATION,
            SemanticTokenModifier::READONLY,
            SemanticTokenModifier::DOCUMENTATION,
        ],
    }
}

// Indexes into the legend above.
const TT_KEYWORD: u32 = 0;
const TT_COMMENT: u32 = 1;
const TT_STRING: u32 = 2;
const TT_NUMBER: u32 = 3;
const TT_OPERATOR: u32 = 4;
const TT_TYPE: u32 = 5;
const TT_CLASS: u32 = 6;
const TT_INTERFACE: u32 = 7;
#[allow(dead_code)] // distinguish top-level functions vs methods in a later pass
const TT_FUNCTION: u32 = 8;
const TT_METHOD: u32 = 9;
const TT_PROPERTY: u32 = 10;
const TT_PARAMETER: u32 = 11;
const TT_VARIABLE: u32 = 12;
const TT_NAMESPACE: u32 = 13;

const MOD_DECLARATION: u32 = 1 << 0;
const MOD_READONLY: u32 = 1 << 1;
const MOD_DOCUMENTATION: u32 = 1 << 2;

pub fn semantic_tokens(doc: &Document) -> SemanticTokens {
    let text = doc.rope.to_string();
    let tokens = pkl_syntax::tokenize(&text);
    let mut emitter = Emitter::default();

    for token in &tokens {
        let Some((ttype, tmods)) = classify(doc, token) else {
            continue;
        };
        let start = byte_to_position(&doc.rope, token.span.start as usize);
        let end = byte_to_position(&doc.rope, token.span.end as usize);
        // The LSP delta encoding only supports single-line tokens. Multi-
        // line tokens (block comments, multi-line strings) are split into
        // per-line slices.
        if end.line == start.line {
            emitter.push(
                start.line,
                start.character,
                end.character - start.character,
                ttype,
                tmods,
            );
        } else {
            let mut line = start.line;
            let mut char_start = start.character;
            while line < end.line {
                let line_len = line_len_utf16(&doc.rope, line);
                let len = line_len.saturating_sub(char_start);
                if len > 0 {
                    emitter.push(line, char_start, len, ttype, tmods);
                }
                line += 1;
                char_start = 0;
            }
            if end.character > 0 {
                emitter.push(end.line, 0, end.character, ttype, tmods);
            }
        }
    }

    SemanticTokens {
        result_id: None,
        data: emitter.data,
    }
}

#[derive(Default)]
struct Emitter {
    data: Vec<SemanticToken>,
    prev_line: u32,
    prev_char: u32,
}

impl Emitter {
    fn push(
        &mut self,
        line: u32,
        char_pos: u32,
        length: u32,
        token_type: u32,
        token_modifiers_bitset: u32,
    ) {
        if length == 0 {
            return;
        }
        let delta_line = line - self.prev_line;
        let delta_start = if delta_line == 0 {
            char_pos - self.prev_char
        } else {
            char_pos
        };
        self.data.push(SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type,
            token_modifiers_bitset,
        });
        self.prev_line = line;
        self.prev_char = char_pos;
    }
}

fn line_len_utf16(rope: &ropey::Rope, line: u32) -> u32 {
    if (line as usize) >= rope.len_lines() {
        return 0;
    }
    let slice = rope.line(line as usize);
    let mut total: u32 = 0;
    for c in slice.chars() {
        if c == '\n' {
            break;
        }
        total += c.len_utf16() as u32;
    }
    total
}

fn classify(doc: &Document, token: &Token<'_>) -> Option<(u32, u32)> {
    use SyntaxKind::*;
    let (ttype, modifiers) = match token.kind {
        DocComment => (TT_COMMENT, MOD_DOCUMENTATION),
        LineComment | BlockComment => (TT_COMMENT, 0),
        String | MultilineString => (TT_STRING, 0),
        IntNumber | FloatNumber | HexNumber | BinNumber | OctNumber => (TT_NUMBER, 0),
        // Plus / minus / etc. — keep operators distinct from keywords.
        Plus | Minus | Star | Slash | Percent | StarStar | EqEq | BangEq | LtEq | GtEq | Lt
        | Gt | Bang | Eq | Arrow | FatArrow | QuestionDot | QuestionQuestion | Pipe | PipeGt
        | Amp | Question => (TT_OPERATOR, 0),
        Ident | QuotedIdent => {
            // Use the resolver to refine the classification.
            let mods = MOD_READONLY;
            let kind = doc
                .analysis
                .resolution
                .by_span_start
                .get(&token.span.start)
                .map(|id| doc.analysis.resolution.symbol(*id).kind);
            let ttype = match kind {
                Some(SymbolKind::Class) => TT_CLASS,
                Some(SymbolKind::TypeAlias) => TT_INTERFACE,
                Some(SymbolKind::TypeParameter) => TT_TYPE,
                Some(SymbolKind::Property | SymbolKind::ObjectParameter) => TT_PROPERTY,
                Some(SymbolKind::Method) => TT_METHOD,
                Some(SymbolKind::Parameter) => TT_PARAMETER,
                Some(SymbolKind::LetBinding | SymbolKind::ForBinding) => TT_VARIABLE,
                Some(SymbolKind::Import { .. } | SymbolKind::Module) => TT_NAMESPACE,
                None => TT_VARIABLE,
            };
            (ttype, mods)
        }
        kind if kind.is_keyword() => {
            let modifier = matches!(
                token.kind,
                AbstractKw | OpenKw | LocalKw | HiddenKw | FixedKw | ExternalKw
            );
            (TT_KEYWORD, if modifier { MOD_DECLARATION } else { 0 })
        }
        _ => return None,
    };
    Some((ttype, modifiers))
}

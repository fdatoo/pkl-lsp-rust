//! Lossless syntax-tree parser.
//!
//! Companion to `parser.rs`: instead of constructing typed AST nodes, this
//! pass drives a `rowan::GreenNodeBuilder` directly to produce a fully
//! lossless tree. Every byte of the original source — trivia included —
//! is preserved in the resulting [`SyntaxNode`], and round-tripping by
//! calling `tree.text().to_string()` reproduces the input verbatim.
//!
//! Grammar coverage mirrors `parser.rs` exactly. The two parsers will be
//! reconciled in a follow-up commit when the typed AST is reformulated as
//! thin wrappers over the lossless tree.

use rowan::{Checkpoint, GreenNodeBuilder, Language};

use crate::diagnostic::SyntaxDiagnostic;
use crate::kind::SyntaxKind;
use crate::lexer::tokenize;
use crate::span::Span;
use crate::syntax::{PklLang, SyntaxNode};
use crate::token::Token;

/// Result of a lossless parse: the syntax-tree root plus any diagnostics
/// emitted along the way. The tree is always returned, even for malformed
/// input, so downstream tooling can still walk it.
pub struct LosslessParse {
    pub syntax: SyntaxNode,
    pub diagnostics: Vec<SyntaxDiagnostic>,
}

/// Parse `src` into a lossless rowan syntax tree.
pub fn parse_green(src: &str) -> LosslessParse {
    let tokens = tokenize(src);
    let mut parser = Parser::new(tokens);
    parser.parse_module();
    let green = parser.builder.finish();
    LosslessParse {
        syntax: SyntaxNode::new_root(green),
        diagnostics: parser.diagnostics,
    }
}

// ----------------------------------------------------------------------
// Parser state

struct Parser<'src> {
    tokens: Vec<Token<'src>>,
    /// Index into `tokens`, including trivia. Significant tokens are
    /// reached by skipping over leading trivia at each peek/bump.
    pos: usize,
    builder: GreenNodeBuilder<'static>,
    diagnostics: Vec<SyntaxDiagnostic>,
    /// True once we've emitted a "found end of file" diagnostic. Used to
    /// suppress the cascading expectations that follow when the user is
    /// in the middle of typing — they only need to see the first
    /// "unexpected end of file" message in the problems panel.
    at_eof_reported: bool,
}

impl<'src> Parser<'src> {
    fn new(tokens: Vec<Token<'src>>) -> Self {
        Self {
            tokens,
            pos: 0,
            builder: GreenNodeBuilder::new(),
            diagnostics: Vec::new(),
            at_eof_reported: false,
        }
    }

    // ------------------------------------------------------------------
    // Cursor primitives

    fn raw_at(&self, idx: usize) -> Option<&Token<'src>> {
        self.tokens.get(idx)
    }

    /// Index of the next significant token at or after `from`.
    fn skip_trivia_from(&self, from: usize) -> usize {
        let mut i = from;
        while i < self.tokens.len() && self.tokens[i].is_trivia() {
            i += 1;
        }
        i
    }

    fn peek_kind(&self) -> SyntaxKind {
        let i = self.skip_trivia_from(self.pos);
        self.tokens
            .get(i)
            .map(|t| t.kind)
            .unwrap_or(SyntaxKind::Eof)
    }

    /// Kind of the n-th *significant* token ahead (0 = current).
    fn nth(&self, n: usize) -> SyntaxKind {
        let mut i = self.pos;
        let mut count = 0;
        while i < self.tokens.len() {
            if !self.tokens[i].is_trivia() {
                if count == n {
                    return self.tokens[i].kind;
                }
                count += 1;
            }
            i += 1;
        }
        SyntaxKind::Eof
    }

    fn peek_span(&self) -> Span {
        let i = self.skip_trivia_from(self.pos);
        if let Some(t) = self.tokens.get(i) {
            t.span
        } else {
            self.tokens
                .last()
                .map(|t| Span::new(t.span.end, t.span.end))
                .unwrap_or(Span::EMPTY)
        }
    }

    fn at(&self, k: SyntaxKind) -> bool {
        self.peek_kind() == k
    }

    fn at_eof(&self) -> bool {
        let i = self.skip_trivia_from(self.pos);
        i >= self.tokens.len() || matches!(self.tokens[i].kind, SyntaxKind::Eof)
    }

    /// Emit all pending trivia into the builder, then consume and emit the
    /// next significant token.
    fn bump(&mut self) {
        self.eat_trivia();
        if let Some(t) = self.raw_at(self.pos) {
            if t.kind != SyntaxKind::Eof {
                self.emit_token(t.kind, t.text);
                self.pos += 1;
            }
        }
    }

    /// Emit any pending trivia tokens *without* consuming the next
    /// significant token.
    fn eat_trivia(&mut self) {
        while let Some(t) = self.raw_at(self.pos) {
            if t.is_trivia() {
                self.emit_token(t.kind, t.text);
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn emit_token(&mut self, kind: SyntaxKind, text: &str) {
        self.builder.token(PklLang::kind_to_raw(kind), text);
    }

    fn start_node(&mut self, kind: SyntaxKind) {
        self.builder.start_node(PklLang::kind_to_raw(kind));
    }

    fn finish_node(&mut self) {
        self.builder.finish_node();
    }

    fn checkpoint(&self) -> Checkpoint {
        self.builder.checkpoint()
    }

    fn start_node_at(&mut self, cp: Checkpoint, kind: SyntaxKind) {
        self.builder.start_node_at(cp, PklLang::kind_to_raw(kind));
    }

    fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: SyntaxKind, what: &str) {
        if self.at(kind) {
            self.bump();
        } else if self.at_eof() && self.at_eof_reported {
            // Already told the user the source ends abruptly; don't
            // bury them in cascading expectations.
        } else {
            let span = self.peek_span();
            let desc = self.peek_describe();
            self.error(
                span,
                format!("expected {} ({}), found {}", what, kind, desc),
            );
            if self.at_eof() {
                self.at_eof_reported = true;
            }
        }
    }

    fn peek_describe(&self) -> String {
        if self.at_eof() {
            return "end of file".into();
        }
        let i = self.skip_trivia_from(self.pos);
        let t = &self.tokens[i];
        match t.kind {
            SyntaxKind::Ident | SyntaxKind::QuotedIdent => format!("identifier `{}`", t.text),
            SyntaxKind::IntNumber
            | SyntaxKind::FloatNumber
            | SyntaxKind::HexNumber
            | SyntaxKind::BinNumber
            | SyntaxKind::OctNumber => format!("number `{}`", t.text),
            SyntaxKind::String | SyntaxKind::MultilineString => "string literal".into(),
            SyntaxKind::Error => format!("invalid token `{}`", t.text),
            other => format!("{}", other),
        }
    }

    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.diagnostics
            .push(SyntaxDiagnostic::error(span, msg.into()));
        if self.at_eof() {
            self.at_eof_reported = true;
        }
    }

    /// Resync to the next likely declaration boundary.
    fn recover_to_item(&mut self) {
        while !self.at_eof() {
            match self.peek_kind() {
                SyntaxKind::ClassKw
                | SyntaxKind::TypeAliasKw
                | SyntaxKind::ImportKw
                | SyntaxKind::ImportGlobKw
                | SyntaxKind::FunctionKw
                | SyntaxKind::ModuleKw
                | SyntaxKind::AmendsKw
                | SyntaxKind::ExtendsKw
                | SyntaxKind::At
                | SyntaxKind::AbstractKw
                | SyntaxKind::OpenKw
                | SyntaxKind::LocalKw
                | SyntaxKind::HiddenKw
                | SyntaxKind::FixedKw
                | SyntaxKind::ExternalKw
                | SyntaxKind::Ident
                | SyntaxKind::QuotedIdent
                | SyntaxKind::RBrace => return,
                _ => self.bump(),
            }
        }
    }

    // ------------------------------------------------------------------
    // Module

    fn parse_module(&mut self) {
        self.start_node(SyntaxKind::Module);

        self.parse_module_header();

        while matches!(
            self.peek_kind(),
            SyntaxKind::ImportKw | SyntaxKind::ImportGlobKw
        ) {
            self.parse_import();
        }

        while !self.at_eof() {
            let before = self.pos;
            self.parse_item();
            if self.pos == before {
                // No progress — emit an error node so we don't loop forever.
                let span = self.peek_span();
                self.error(span, "unexpected token");
                self.start_node(SyntaxKind::ErrorNode);
                if !self.at_eof() {
                    self.bump();
                }
                self.finish_node();
                self.recover_to_item();
            }
        }

        // Flush any trailing trivia so the source reconstructs verbatim.
        self.eat_trivia();
        // Skip the lexer's EOF sentinel without emitting it.
        while let Some(t) = self.raw_at(self.pos) {
            if t.kind == SyntaxKind::Eof {
                self.pos += 1;
            } else {
                break;
            }
        }

        self.finish_node();
    }

    /// True when the upcoming tokens form a module header (possibly after
    /// annotations and modifiers).
    fn starts_module_header(&self) -> bool {
        let mut i = 0usize;
        loop {
            match self.nth(i) {
                SyntaxKind::At => {
                    i += 1;
                    while matches!(
                        self.nth(i),
                        SyntaxKind::Ident | SyntaxKind::QuotedIdent | SyntaxKind::Dot
                    ) {
                        i += 1;
                    }
                    if self.nth(i) == SyntaxKind::LBrace {
                        // Skip balanced braces.
                        let mut depth = 1i32;
                        i += 1;
                        while depth > 0 {
                            match self.nth(i) {
                                SyntaxKind::LBrace => depth += 1,
                                SyntaxKind::RBrace => depth -= 1,
                                SyntaxKind::Eof => return false,
                                _ => {}
                            }
                            i += 1;
                        }
                    }
                }
                k if k.is_modifier_kw() => i += 1,
                SyntaxKind::ModuleKw | SyntaxKind::AmendsKw | SyntaxKind::ExtendsKw => return true,
                _ => return false,
            }
        }
    }

    fn parse_module_header(&mut self) {
        if !self.starts_module_header() {
            return;
        }

        self.start_node(SyntaxKind::ModuleHeader);

        self.parse_annotations();
        self.parse_modifiers();

        match self.peek_kind() {
            SyntaxKind::ModuleKw => {
                self.bump();
                self.parse_qualified_name("module name");
            }
            SyntaxKind::AmendsKw => {
                self.start_node(SyntaxKind::ExtendsAmendsClause);
                self.bump();
                self.parse_string_lit_required();
                self.finish_node();
            }
            SyntaxKind::ExtendsKw => {
                self.start_node(SyntaxKind::ExtendsAmendsClause);
                self.bump();
                self.parse_string_lit_required();
                self.finish_node();
            }
            _ => {}
        }

        // Optional secondary clause after `module name`.
        match self.peek_kind() {
            SyntaxKind::AmendsKw | SyntaxKind::ExtendsKw => {
                self.start_node(SyntaxKind::ExtendsAmendsClause);
                self.bump();
                self.parse_string_lit_required();
                self.finish_node();
            }
            _ => {}
        }

        self.finish_node();
    }

    fn parse_annotations(&mut self) {
        if !self.at(SyntaxKind::At) {
            return;
        }
        self.start_node(SyntaxKind::AnnotationList);
        while self.at(SyntaxKind::At) {
            self.start_node(SyntaxKind::Annotation);
            self.bump(); // @
            self.parse_qualified_name("annotation name");
            if self.at(SyntaxKind::LBrace) {
                self.parse_object_body();
            }
            self.finish_node();
        }
        self.finish_node();
    }

    fn parse_modifiers(&mut self) {
        if !self.peek_kind().is_modifier_kw() {
            return;
        }
        self.start_node(SyntaxKind::ModifierList);
        while self.peek_kind().is_modifier_kw() {
            self.start_node(SyntaxKind::Modifier);
            self.bump();
            self.finish_node();
        }
        self.finish_node();
    }

    fn parse_qualified_name(&mut self, what: &str) {
        let cp = self.checkpoint();
        if !self.parse_identifier_opt() {
            let span = self.peek_span();
            let desc = self.peek_describe();
            self.error(span, format!("expected {}, found {}", what, desc));
            // Still emit an empty QualifiedName for downstream stability.
            self.start_node_at(cp, SyntaxKind::QualifiedName);
            self.finish_node();
            return;
        }
        while self.at(SyntaxKind::Dot) && self.nth(1) == SyntaxKind::Ident {
            self.bump(); // .
            self.parse_identifier_opt();
        }
        self.start_node_at(cp, SyntaxKind::QualifiedName);
        self.finish_node();
    }

    /// Try to parse an identifier. Returns true if one was consumed.
    fn parse_identifier_opt(&mut self) -> bool {
        match self.peek_kind() {
            SyntaxKind::Ident | SyntaxKind::QuotedIdent => {
                self.bump();
                true
            }
            _ => false,
        }
    }

    fn parse_import(&mut self) {
        self.start_node(SyntaxKind::ImportClause);
        // `import` or `import*`
        self.bump();
        self.parse_string_lit_required();
        if self.eat(SyntaxKind::AsKw) && !self.parse_identifier_opt() {
            let span = self.peek_span();
            self.error(span, "expected identifier after `as`");
        }
        self.finish_node();
    }

    fn parse_string_lit_required(&mut self) {
        match self.peek_kind() {
            SyntaxKind::String | SyntaxKind::MultilineString => {
                self.bump();
            }
            _ => {
                let span = self.peek_span();
                self.error(span, "expected string literal");
            }
        }
    }

    // ------------------------------------------------------------------
    // Items

    fn parse_item(&mut self) {
        let cp = self.checkpoint();
        self.parse_annotations();
        self.parse_modifiers();

        match self.peek_kind() {
            SyntaxKind::ClassKw => self.parse_class(cp),
            SyntaxKind::TypeAliasKw => self.parse_typealias(cp),
            SyntaxKind::FunctionKw => self.parse_method(cp),
            SyntaxKind::Ident | SyntaxKind::QuotedIdent => self.parse_property(cp),
            SyntaxKind::Eof => {}
            _ => {
                let span = self.peek_span();
                let desc = self.peek_describe();
                self.error(span, format!("unexpected {}", desc));
                self.start_node_at(cp, SyntaxKind::ErrorNode);
                if !self.at_eof() {
                    self.bump();
                }
                self.finish_node();
            }
        }
    }

    fn parse_class(&mut self, cp: Checkpoint) {
        self.start_node_at(cp, SyntaxKind::ClassDecl);
        self.bump(); // class
        if !self.parse_identifier_opt() {
            let span = self.peek_span();
            self.error(span, "expected class name");
        }
        self.parse_type_parameters();
        if self.eat(SyntaxKind::ExtendsKw) {
            self.parse_type();
        }
        if self.at(SyntaxKind::LBrace) {
            self.parse_class_body();
        }
        self.finish_node();
    }

    fn parse_class_body(&mut self) {
        self.start_node(SyntaxKind::ClassBody);
        self.bump(); // {
        while !self.at_eof() && !self.at(SyntaxKind::RBrace) {
            let before = self.pos;
            let cp = self.checkpoint();
            self.parse_annotations();
            self.parse_modifiers();
            match self.peek_kind() {
                SyntaxKind::FunctionKw => self.parse_method_as(cp, SyntaxKind::ClassMethodDecl),
                SyntaxKind::Ident | SyntaxKind::QuotedIdent => {
                    self.parse_property_as(cp, SyntaxKind::ClassPropertyDecl);
                }
                _ => {
                    let span = self.peek_span();
                    let desc = self.peek_describe();
                    self.error(span, format!("unexpected {} in class body", desc));
                    if self.pos == before {
                        self.bump();
                    }
                }
            }
        }
        self.expect(SyntaxKind::RBrace, "closing `}`");
        self.finish_node();
    }

    fn parse_typealias(&mut self, cp: Checkpoint) {
        self.start_node_at(cp, SyntaxKind::TypeAliasDecl);
        self.bump(); // typealias
        if !self.parse_identifier_opt() {
            let span = self.peek_span();
            self.error(span, "expected type alias name");
        }
        self.parse_type_parameters();
        if self.eat(SyntaxKind::Eq) {
            self.parse_type();
        } else {
            let span = self.peek_span();
            self.error(span, "expected `=` in `typealias`");
        }
        self.finish_node();
    }

    fn parse_property(&mut self, cp: Checkpoint) {
        self.parse_property_as(cp, SyntaxKind::PropertyDecl);
    }

    fn parse_property_as(&mut self, cp: Checkpoint, kind: SyntaxKind) {
        self.start_node_at(cp, kind);
        if !self.parse_identifier_opt() {
            // Annotations or modifiers can leave us pointing at a non-ident
            // token if the source is malformed; emit an error rather than
            // panicking so the lossless tree still round-trips.
            let span = self.peek_span();
            self.error(span, "expected property name");
            self.finish_node();
            return;
        }
        if self.eat(SyntaxKind::Colon) {
            self.parse_type();
        }
        if self.eat(SyntaxKind::Eq) {
            self.parse_expr();
        } else if self.at(SyntaxKind::LBrace) {
            self.parse_object_body();
        }
        self.finish_node();
    }

    fn parse_method(&mut self, cp: Checkpoint) {
        self.parse_method_as(cp, SyntaxKind::MethodDecl);
    }

    fn parse_method_as(&mut self, cp: Checkpoint, kind: SyntaxKind) {
        self.start_node_at(cp, kind);
        self.bump(); // function
        if !self.parse_identifier_opt() {
            let span = self.peek_span();
            self.error(span, "expected function name");
        }
        self.parse_type_parameters();
        self.parse_parameter_list();
        if self.eat(SyntaxKind::Colon) {
            self.parse_type();
        }
        if self.eat(SyntaxKind::Eq) {
            self.parse_expr();
        }
        self.finish_node();
    }

    fn parse_type_parameters(&mut self) {
        if !self.at(SyntaxKind::Lt) {
            return;
        }
        self.start_node(SyntaxKind::TypeParameterList);
        self.bump(); // <
        while !self.at_eof() && !self.at(SyntaxKind::Gt) {
            self.start_node(SyntaxKind::TypeParameter);
            match self.peek_kind() {
                SyntaxKind::InKw | SyntaxKind::OutKw => self.bump(),
                _ => {}
            }
            if !self.parse_identifier_opt() {
                // Emit nothing — span is empty.
            }
            self.finish_node();
            if !self.eat(SyntaxKind::Comma) {
                break;
            }
        }
        self.expect(SyntaxKind::Gt, "closing `>`");
        self.finish_node();
    }

    fn parse_parameter_list(&mut self) {
        if !self.at(SyntaxKind::LParen) {
            let span = self.peek_span();
            self.error(span, "expected `(`");
            return;
        }
        self.start_node(SyntaxKind::ParameterList);
        self.bump(); // (
        while !self.at_eof() && !self.at(SyntaxKind::RParen) {
            self.parse_parameter();
            if !self.eat(SyntaxKind::Comma) {
                break;
            }
        }
        self.expect(SyntaxKind::RParen, "closing `)`");
        self.finish_node();
    }

    fn parse_parameter(&mut self) {
        self.start_node(SyntaxKind::Parameter);
        if !self.parse_identifier_opt() {
            let span = self.peek_span();
            self.error(span, "expected parameter name");
        }
        if self.eat(SyntaxKind::Colon) {
            self.parse_type();
        }
        self.finish_node();
    }

    // ------------------------------------------------------------------
    // Types

    fn parse_type(&mut self) {
        let cp = self.checkpoint();
        self.parse_type_nullable();
        if !self.at(SyntaxKind::Pipe) {
            return;
        }
        while self.eat(SyntaxKind::Pipe) {
            self.parse_type_nullable();
        }
        self.start_node_at(cp, SyntaxKind::TypeUnion);
        self.finish_node();
    }

    fn parse_type_nullable(&mut self) {
        let cp = self.checkpoint();
        self.parse_type_primary();
        while self.at(SyntaxKind::Question) {
            self.bump(); // ?
            self.start_node_at(cp, SyntaxKind::TypeNullable);
            self.finish_node();
        }
    }

    fn parse_type_primary(&mut self) {
        match self.peek_kind() {
            SyntaxKind::LParen => self.parse_type_paren_or_function(),
            SyntaxKind::UnknownKw => {
                self.start_node(SyntaxKind::TypeUnknown);
                self.bump();
                self.finish_node();
            }
            SyntaxKind::NothingKw => {
                self.start_node(SyntaxKind::TypeNothing);
                self.bump();
                self.finish_node();
            }
            SyntaxKind::ModuleKw => {
                self.start_node(SyntaxKind::TypeModule);
                self.bump();
                self.finish_node();
            }
            SyntaxKind::String | SyntaxKind::MultilineString => {
                self.start_node(SyntaxKind::TypeStringLiteral);
                self.bump();
                self.finish_node();
            }
            SyntaxKind::Ident | SyntaxKind::QuotedIdent => {
                self.start_node(SyntaxKind::TypeRef);
                self.parse_qualified_name("type name");
                if self.at(SyntaxKind::Lt) {
                    self.parse_type_argument_list();
                }
                self.finish_node();
            }
            _ => {
                let span = self.peek_span();
                let desc = self.peek_describe();
                self.error(span, format!("expected type, found {}", desc));
                self.start_node(SyntaxKind::ErrorNode);
                if !self.at_eof() {
                    self.bump();
                }
                self.finish_node();
            }
        }
    }

    fn parse_type_paren_or_function(&mut self) {
        let cp = self.checkpoint();
        self.bump(); // (
        let mut saw_comma = false;
        let mut count = 0usize;
        if !self.at(SyntaxKind::RParen) {
            self.parse_type();
            count += 1;
            while self.eat(SyntaxKind::Comma) {
                saw_comma = true;
                self.parse_type();
                count += 1;
            }
        }
        self.expect(SyntaxKind::RParen, "closing `)`");
        if self.at(SyntaxKind::Arrow) {
            self.bump();
            self.parse_type();
            self.start_node_at(cp, SyntaxKind::TypeFunction);
            self.finish_node();
            return;
        }
        if count == 1 && !saw_comma {
            self.start_node_at(cp, SyntaxKind::TypeParenthesized);
            self.finish_node();
            return;
        }
        // `(A, B)` without arrow is unusual at the type level. Match the
        // recursive-descent parser's diagnostic and emit an error node.
        let span = self.peek_span();
        self.error(
            span,
            "Pkl has no tuple types; use `Pair<A, B>` for a 2-tuple or write a function type \
             `(A, B) -> R`",
        );
        self.start_node_at(cp, SyntaxKind::ErrorNode);
        self.finish_node();
    }

    fn parse_type_argument_list(&mut self) {
        self.start_node(SyntaxKind::TypeArgumentList);
        self.bump(); // <
        while !self.at_eof() && !self.at(SyntaxKind::Gt) {
            self.parse_type();
            if !self.eat(SyntaxKind::Comma) {
                break;
            }
        }
        self.expect(SyntaxKind::Gt, "closing `>`");
        self.finish_node();
    }

    // ------------------------------------------------------------------
    // Expressions

    fn parse_expr(&mut self) {
        self.parse_expr_pipeline();
    }

    fn parse_expr_pipeline(&mut self) {
        let cp = self.checkpoint();
        self.parse_expr_null_coalesce();
        while self.at(SyntaxKind::PipeGt) {
            self.bump();
            self.parse_expr_null_coalesce();
            self.start_node_at(cp, SyntaxKind::BinaryExpr);
            self.finish_node();
        }
    }

    fn parse_expr_null_coalesce(&mut self) {
        let cp = self.checkpoint();
        self.parse_expr_or();
        while self.at(SyntaxKind::QuestionQuestion) {
            self.bump();
            self.parse_expr_or();
            self.start_node_at(cp, SyntaxKind::NullCoalesceExpr);
            self.finish_node();
        }
    }

    fn parse_expr_or(&mut self) {
        let cp = self.checkpoint();
        self.parse_expr_and();
        while self.at(SyntaxKind::Pipe) && self.nth(1) == SyntaxKind::Pipe {
            self.bump();
            self.bump();
            self.parse_expr_and();
            self.start_node_at(cp, SyntaxKind::BinaryExpr);
            self.finish_node();
        }
    }

    fn parse_expr_and(&mut self) {
        let cp = self.checkpoint();
        self.parse_expr_eq();
        while self.at(SyntaxKind::Amp) && self.nth(1) == SyntaxKind::Amp {
            self.bump();
            self.bump();
            self.parse_expr_eq();
            self.start_node_at(cp, SyntaxKind::BinaryExpr);
            self.finish_node();
        }
    }

    fn parse_expr_eq(&mut self) {
        let cp = self.checkpoint();
        self.parse_expr_cmp();
        while matches!(self.peek_kind(), SyntaxKind::EqEq | SyntaxKind::BangEq) {
            self.bump();
            self.parse_expr_cmp();
            self.start_node_at(cp, SyntaxKind::BinaryExpr);
            self.finish_node();
        }
    }

    fn parse_expr_cmp(&mut self) {
        let cp = self.checkpoint();
        self.parse_expr_add();
        loop {
            match self.peek_kind() {
                SyntaxKind::Lt | SyntaxKind::LtEq | SyntaxKind::Gt | SyntaxKind::GtEq => {
                    self.bump();
                    self.parse_expr_add();
                    self.start_node_at(cp, SyntaxKind::BinaryExpr);
                    self.finish_node();
                }
                SyntaxKind::IsKw => {
                    self.bump();
                    self.parse_type_nullable();
                    self.start_node_at(cp, SyntaxKind::TypeCheckExpr);
                    self.finish_node();
                }
                SyntaxKind::AsKw => {
                    self.bump();
                    self.parse_type_nullable();
                    self.start_node_at(cp, SyntaxKind::TypeCastExpr);
                    self.finish_node();
                }
                _ => break,
            }
        }
    }

    fn parse_expr_add(&mut self) {
        let cp = self.checkpoint();
        self.parse_expr_mul();
        while matches!(self.peek_kind(), SyntaxKind::Plus | SyntaxKind::Minus) {
            self.bump();
            self.parse_expr_mul();
            self.start_node_at(cp, SyntaxKind::BinaryExpr);
            self.finish_node();
        }
    }

    fn parse_expr_mul(&mut self) {
        let cp = self.checkpoint();
        self.parse_expr_pow();
        while matches!(
            self.peek_kind(),
            SyntaxKind::Star | SyntaxKind::Slash | SyntaxKind::Percent
        ) {
            self.bump();
            self.parse_expr_pow();
            self.start_node_at(cp, SyntaxKind::BinaryExpr);
            self.finish_node();
        }
    }

    fn parse_expr_pow(&mut self) {
        let cp = self.checkpoint();
        self.parse_expr_unary();
        if self.at(SyntaxKind::StarStar) {
            self.bump();
            self.parse_expr_pow(); // right-associative
            self.start_node_at(cp, SyntaxKind::BinaryExpr);
            self.finish_node();
        }
    }

    fn parse_expr_unary(&mut self) {
        match self.peek_kind() {
            SyntaxKind::Minus | SyntaxKind::Bang => {
                let cp = self.checkpoint();
                self.bump();
                self.parse_expr_unary();
                self.start_node_at(cp, SyntaxKind::UnaryExpr);
                self.finish_node();
            }
            _ => self.parse_expr_postfix(),
        }
    }

    fn parse_expr_postfix(&mut self) {
        let cp = self.checkpoint();
        self.parse_expr_primary();
        loop {
            match self.peek_kind() {
                SyntaxKind::Dot | SyntaxKind::QuestionDot => {
                    self.bump(); // `.` or `?.`
                    if !self.parse_identifier_opt() {
                        // Recovery: emit a diagnostic and an Error
                        // placeholder so completion can still see this
                        // is a member-access context.
                        let span = self.peek_span();
                        self.error(span, "expected member name after `.`");
                        self.start_node(SyntaxKind::ErrorNode);
                        self.finish_node();
                    }
                    self.start_node_at(cp, SyntaxKind::MemberExpr);
                    self.finish_node();
                }
                SyntaxKind::LParen => {
                    self.parse_arg_list();
                    self.start_node_at(cp, SyntaxKind::CallExpr);
                    self.finish_node();
                }
                SyntaxKind::LBracket => {
                    self.bump();
                    self.parse_expr();
                    self.expect(SyntaxKind::RBracket, "closing `]`");
                    self.start_node_at(cp, SyntaxKind::IndexExpr);
                    self.finish_node();
                }
                SyntaxKind::Bang if self.nth(1) == SyntaxKind::Bang => {
                    self.bump();
                    self.bump();
                    self.start_node_at(cp, SyntaxKind::NonNullExpr);
                    self.finish_node();
                }
                SyntaxKind::LBrace => {
                    self.parse_object_body();
                    self.start_node_at(cp, SyntaxKind::AmendsExpr);
                    self.finish_node();
                }
                _ => break,
            }
        }
    }

    fn parse_arg_list(&mut self) {
        self.start_node(SyntaxKind::ArgList);
        self.bump(); // (
        while !self.at_eof() && !self.at(SyntaxKind::RParen) {
            self.parse_expr();
            if !self.eat(SyntaxKind::Comma) {
                break;
            }
        }
        self.expect(SyntaxKind::RParen, "closing `)`");
        self.finish_node();
    }

    fn parse_expr_primary(&mut self) {
        match self.peek_kind() {
            SyntaxKind::IntNumber
            | SyntaxKind::HexNumber
            | SyntaxKind::BinNumber
            | SyntaxKind::OctNumber
            | SyntaxKind::FloatNumber
            | SyntaxKind::String
            | SyntaxKind::MultilineString
            | SyntaxKind::TrueKw
            | SyntaxKind::FalseKw
            | SyntaxKind::NullKw => {
                self.start_node(SyntaxKind::LiteralExpr);
                self.bump();
                self.finish_node();
            }
            SyntaxKind::ThisKw
            | SyntaxKind::SuperKw
            | SyntaxKind::OuterKw
            | SyntaxKind::ModuleKw => {
                self.start_node(SyntaxKind::IdentExpr);
                self.bump();
                self.finish_node();
            }
            SyntaxKind::LParen => {
                if self.looks_like_lambda() {
                    self.parse_lambda();
                } else {
                    self.start_node(SyntaxKind::ParenExpr);
                    self.bump(); // (
                    self.parse_expr();
                    self.expect(SyntaxKind::RParen, "closing `)`");
                    self.finish_node();
                }
            }
            SyntaxKind::IfKw => self.parse_if(),
            SyntaxKind::LetKw => self.parse_let(),
            SyntaxKind::NewKw => self.parse_new(),
            SyntaxKind::ThrowKw => {
                self.start_node(SyntaxKind::ThrowExpr);
                self.bump();
                self.expect(SyntaxKind::LParen, "`(`");
                self.parse_expr();
                self.expect(SyntaxKind::RParen, "closing `)`");
                self.finish_node();
            }
            SyntaxKind::TraceKw => {
                self.start_node(SyntaxKind::TraceExpr);
                self.bump();
                self.expect(SyntaxKind::LParen, "`(`");
                self.parse_expr();
                self.expect(SyntaxKind::RParen, "closing `)`");
                self.finish_node();
            }
            SyntaxKind::ReadKw | SyntaxKind::ReadOrNullKw | SyntaxKind::ReadGlobKw => {
                self.start_node(SyntaxKind::ReadExpr);
                self.bump();
                self.expect(SyntaxKind::LParen, "`(`");
                self.parse_expr();
                self.expect(SyntaxKind::RParen, "closing `)`");
                self.finish_node();
            }
            SyntaxKind::Ident | SyntaxKind::QuotedIdent => {
                self.start_node(SyntaxKind::IdentExpr);
                self.bump();
                self.finish_node();
            }
            _ => {
                if !(self.at_eof() && self.at_eof_reported) {
                    let span = self.peek_span();
                    let desc = self.peek_describe();
                    self.error(span, format!("expected expression, found {}", desc));
                }
                self.start_node(SyntaxKind::ErrorNode);
                if !self.at_eof() {
                    self.bump();
                }
                self.finish_node();
            }
        }
    }

    /// `(...) ->` after balanced parens.
    fn looks_like_lambda(&self) -> bool {
        // Scan ahead through significant tokens, tracking paren depth.
        let mut depth = 0i32;
        let mut i = 0usize;
        loop {
            let k = self.nth(i);
            match k {
                SyntaxKind::LParen => depth += 1,
                SyntaxKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return self.nth(i + 1) == SyntaxKind::Arrow;
                    }
                }
                SyntaxKind::Eof => return false,
                _ => {}
            }
            i += 1;
        }
    }

    fn parse_lambda(&mut self) {
        self.start_node(SyntaxKind::LambdaExpr);
        self.parse_parameter_list();
        self.expect(SyntaxKind::Arrow, "`->`");
        self.parse_expr();
        self.finish_node();
    }

    fn parse_if(&mut self) {
        self.start_node(SyntaxKind::IfExpr);
        self.bump(); // if
        self.expect(SyntaxKind::LParen, "`(` after `if`");
        self.parse_expr();
        self.expect(SyntaxKind::RParen, "closing `)`");
        self.parse_expr(); // then
        self.expect(SyntaxKind::ElseKw, "`else`");
        self.parse_expr(); // else
        self.finish_node();
    }

    fn parse_let(&mut self) {
        self.start_node(SyntaxKind::LetExpr);
        self.bump(); // let
        self.expect(SyntaxKind::LParen, "`(` after `let`");
        self.parse_parameter();
        self.expect(SyntaxKind::Eq, "`=`");
        self.parse_expr();
        self.expect(SyntaxKind::RParen, "closing `)`");
        self.parse_expr();
        self.finish_node();
    }

    fn parse_new(&mut self) {
        self.start_node(SyntaxKind::NewExpr);
        self.bump(); // new
        if !self.at(SyntaxKind::LBrace) {
            self.parse_type();
        }
        self.parse_object_body();
        self.finish_node();
    }

    // ------------------------------------------------------------------
    // Object bodies

    fn parse_object_body(&mut self) {
        self.start_node(SyntaxKind::ObjectBody);
        self.expect(SyntaxKind::LBrace, "`{`");

        if self.looks_like_object_params() {
            self.start_node(SyntaxKind::ParameterList);
            loop {
                self.parse_parameter();
                if !self.eat(SyntaxKind::Comma) {
                    break;
                }
            }
            self.expect(SyntaxKind::Arrow, "`->`");
            self.finish_node();
        }

        while !self.at_eof() && !self.at(SyntaxKind::RBrace) {
            let before = self.pos;
            self.parse_object_member();
            if self.pos == before {
                self.bump();
            }
            while self.eat(SyntaxKind::Semicolon) || self.eat(SyntaxKind::Comma) {}
        }
        self.expect(SyntaxKind::RBrace, "closing `}`");
        self.finish_node();
    }

    fn looks_like_object_params(&self) -> bool {
        let mut i = 0usize;
        loop {
            let k = self.nth(i);
            if !matches!(k, SyntaxKind::Ident | SyntaxKind::QuotedIdent) {
                return false;
            }
            i += 1;
            if self.nth(i) == SyntaxKind::Colon {
                let mut depth = 0i32;
                i += 1;
                loop {
                    match self.nth(i) {
                        SyntaxKind::Lt | SyntaxKind::LParen => depth += 1,
                        SyntaxKind::Gt | SyntaxKind::RParen => depth -= 1,
                        SyntaxKind::Comma | SyntaxKind::Arrow if depth == 0 => break,
                        SyntaxKind::RBrace | SyntaxKind::Eof => return false,
                        _ => {}
                    }
                    i += 1;
                }
            }
            match self.nth(i) {
                SyntaxKind::Arrow => return true,
                SyntaxKind::Comma => {
                    i += 1;
                    continue;
                }
                _ => return false,
            }
        }
    }

    fn parse_object_member(&mut self) {
        match self.peek_kind() {
            SyntaxKind::Ellipsis => {
                self.start_node(SyntaxKind::ObjectSpread);
                self.bump();
                self.parse_expr();
                self.finish_node();
            }
            SyntaxKind::WhenKw => {
                self.start_node(SyntaxKind::WhenGenerator);
                self.bump();
                self.expect(SyntaxKind::LParen, "`(` after `when`");
                self.parse_expr();
                self.expect(SyntaxKind::RParen, "closing `)`");
                self.parse_object_body();
                if self.eat(SyntaxKind::ElseKw) {
                    self.parse_object_body();
                }
                self.finish_node();
            }
            SyntaxKind::ForKw => {
                self.start_node(SyntaxKind::ForGenerator);
                self.bump();
                self.expect(SyntaxKind::LParen, "`(` after `for`");
                self.parse_parameter();
                while self.eat(SyntaxKind::Comma) {
                    self.parse_parameter();
                }
                self.expect(SyntaxKind::InKw, "`in`");
                self.parse_expr();
                self.expect(SyntaxKind::RParen, "closing `)`");
                self.parse_object_body();
                self.finish_node();
            }
            SyntaxKind::LBracket => {
                self.start_node(SyntaxKind::ObjectEntryComputed);
                self.bump();
                self.parse_expr();
                self.expect(SyntaxKind::RBracket, "closing `]`");
                if self.eat(SyntaxKind::Eq) {
                    self.parse_expr();
                } else if self.at(SyntaxKind::LBrace) {
                    self.parse_object_body();
                } else {
                    let span = self.peek_span();
                    self.error(span, "expected `=` or `{` after `[key]`");
                }
                self.finish_node();
            }
            SyntaxKind::FunctionKw => {
                let cp = self.checkpoint();
                self.parse_method_as(cp, SyntaxKind::ObjectMethod);
            }
            k if k.is_modifier_kw() => {
                let cp = self.checkpoint();
                self.parse_annotations();
                self.parse_modifiers();
                match self.peek_kind() {
                    SyntaxKind::FunctionKw => self.parse_method_as(cp, SyntaxKind::ObjectMethod),
                    SyntaxKind::Ident | SyntaxKind::QuotedIdent => {
                        self.parse_property_as(cp, SyntaxKind::ObjectProperty);
                    }
                    _ => {}
                }
            }
            SyntaxKind::Ident | SyntaxKind::QuotedIdent if self.looks_like_property_decl() => {
                let cp = self.checkpoint();
                self.parse_property_as(cp, SyntaxKind::ObjectProperty);
            }
            SyntaxKind::At => {
                let cp = self.checkpoint();
                self.parse_annotations();
                self.parse_modifiers();
                match self.peek_kind() {
                    SyntaxKind::FunctionKw => self.parse_method_as(cp, SyntaxKind::ObjectMethod),
                    _ => self.parse_property_as(cp, SyntaxKind::ObjectProperty),
                }
            }
            _ => {
                self.start_node(SyntaxKind::ObjectElement);
                self.parse_expr();
                self.finish_node();
            }
        }
    }

    fn looks_like_property_decl(&self) -> bool {
        let mut i = 0usize;
        if !matches!(self.nth(i), SyntaxKind::Ident | SyntaxKind::QuotedIdent) {
            return false;
        }
        i += 1;
        if self.nth(i) == SyntaxKind::Colon {
            let mut depth = 0i32;
            i += 1;
            loop {
                match self.nth(i) {
                    SyntaxKind::Lt | SyntaxKind::LParen => depth += 1,
                    SyntaxKind::Gt | SyntaxKind::RParen => depth -= 1,
                    SyntaxKind::Eq if depth == 0 => return true,
                    SyntaxKind::LBrace if depth == 0 => return true,
                    SyntaxKind::Semicolon
                    | SyntaxKind::Comma
                    | SyntaxKind::RBrace
                    | SyntaxKind::Eof
                        if depth == 0 =>
                    {
                        return false
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        matches!(self.nth(i), SyntaxKind::Eq | SyntaxKind::LBrace)
    }
}

// ----------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(src: &str) {
        let LosslessParse {
            syntax,
            diagnostics: _,
        } = parse_green(src);
        let reconstructed = syntax.text().to_string();
        assert_eq!(
            src, reconstructed,
            "round-trip mismatch\n--- input ---\n{src}\n--- output ---\n{reconstructed}\n"
        );
        assert_eq!(syntax.kind(), SyntaxKind::Module);
    }

    #[test]
    fn rt_empty() {
        round_trip("");
    }

    #[test]
    fn rt_whitespace_only() {
        round_trip("   \n\t\n  ");
    }

    #[test]
    fn rt_doc_comment_only() {
        round_trip("/// hello\n/// world\n");
    }

    #[test]
    fn rt_module_header() {
        round_trip("module foo\n");
    }

    #[test]
    fn rt_module_amends() {
        round_trip("amends \"base.pkl\"\n");
    }

    #[test]
    fn rt_simple_property() {
        round_trip("x = 1\n");
    }

    #[test]
    fn rt_typed_property() {
        round_trip("port: Int = 8080\n");
    }

    #[test]
    fn rt_class_with_members() {
        round_trip(
            "class Person {\n  name: String\n  age: Int = 0\n  function greet(): String = \"hi\"\n}\n",
        );
    }

    #[test]
    fn rt_imports() {
        round_trip("import \"a.pkl\"\nimport* \"b/*.pkl\" as bs\n");
    }

    #[test]
    fn rt_object_literal() {
        round_trip("config { x = 1; y = \"two\"; nested { z = 3 } }\n");
    }

    #[test]
    fn rt_expressions() {
        round_trip(
            "x = 1 + 2 * 3 - 4 / 5 % 6 ** 7\n\
             y = a || b && c == d != e < f <= g > h >= i\n\
             z = if (a) b else c\n\
             w = let (n = 1) n + 2\n\
             q = foo.bar(1, 2)[0]!!\n\
             r = (a, b) -> a + b\n",
        );
    }

    #[test]
    fn rt_when_for_generators() {
        round_trip(
            "config {\n\
            \x20 when (cond) {\n\
            \x20   y = 1\n\
            \x20 } else {\n\
            \x20   y = 2\n\
            \x20 }\n\
            \x20 for (i in xs) {\n\
            \x20   [i] = i * 2\n\
            \x20 }\n\
            }\n",
        );
    }

    #[test]
    fn rt_typealias_and_class_generics() {
        round_trip(
            "typealias StringMap<V> = Map<String, V>\n\
             class Box<out T> {\n  value: T\n}\n",
        );
    }

    #[test]
    fn rt_annotations() {
        round_trip("@Deprecated\nx: Int = 1\n");
    }

    #[test]
    fn rt_preserves_comments_and_whitespace() {
        let src = "// leading\n/// doc\nmodule foo // trailing\n\n// gap\nx = 1\n";
        round_trip(src);
    }

    #[test]
    fn rt_error_recovery_keeps_source() {
        // Garbage at top level — we should still emit an ErrorNode and
        // preserve the bytes.
        round_trip("@@@ what is this\nx = 1\n");
    }

    #[test]
    fn module_node_root() {
        let parsed = parse_green("x = 1\n");
        assert_eq!(parsed.syntax.kind(), SyntaxKind::Module);
        // First significant child is a PropertyDecl.
        let prop = parsed
            .syntax
            .children()
            .find(|n| n.kind() == SyntaxKind::PropertyDecl)
            .expect("PropertyDecl present");
        assert_eq!(prop.text().to_string(), "x = 1");
    }

    #[test]
    fn diagnostics_emitted_for_missing_brace() {
        let parsed = parse_green("class Foo {\n  x = 1\n");
        assert!(
            !parsed.diagnostics.is_empty(),
            "expected at least one diagnostic for unclosed class body"
        );
    }
}

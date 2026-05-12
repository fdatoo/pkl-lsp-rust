//! Recursive-descent parser for Pkl.
//!
//! Scope of this initial implementation:
//!
//! * Module header (`module foo`, `amends "x"`, `extends "x"`), with
//!   annotations, modifiers, and an optional leading doc comment.
//! * Top-level `import` and `import*` clauses with optional `as` alias.
//! * Top-level declarations: `class`, `typealias`, properties, methods.
//! * Class bodies containing the same property/method forms.
//! * Type references: named types, generics, nullable (`?`), unions (`|`),
//!   `unknown`, `nothing`, `module`, parenthesised types, function types.
//! * Expressions with the canonical Pkl precedence levels (pipeline, `??`,
//!   `||`, `&&`, equality, comparison + `is`/`as`, additive, multiplicative,
//!   exponent, unary, postfix: call/index/member/`!!`).
//! * `if`, `let`, `new`, `throw`, `trace`, `read[?|*]`, lambdas, and object
//!   bodies including `when`, `for`, spread, and `[key] = value` entries.
//!
//! Out of scope here (parser-level): full string-interpolation reparsing
//! and complete validation of unusual modifier combinations. Both are picked
//! up by the analyzer.

use crate::ast::*;
use crate::diagnostic::SyntaxDiagnostic;
use crate::kind::SyntaxKind;
use crate::lexer::tokenize;
use crate::span::Span;
use crate::token::Token;

/// Result of a parse: the AST root, plus any diagnostics emitted along the
/// way. The AST is always returned — even for malformed input — so that
/// downstream tooling can still operate on the partial tree.
pub struct ParseResult {
    pub module: Module,
    pub diagnostics: Vec<SyntaxDiagnostic>,
}

pub fn parse(src: &str) -> ParseResult {
    let tokens = tokenize(src);
    let mut parser = Parser::new(src, tokens);
    let module = parser.parse_module();
    ParseResult {
        module,
        diagnostics: parser.diagnostics,
    }
}

// ----------------------------------------------------------------------
// Parser state

struct Parser<'src> {
    src: &'src str,
    tokens: Vec<Token<'src>>,
    /// Indices into `tokens` skipping trivia, in original order. Used for
    /// O(1) `peek(n)` over the meaningful stream.
    significant: Vec<usize>,
    /// Current position into `significant`.
    cursor: usize,
    diagnostics: Vec<SyntaxDiagnostic>,
    /// Last doc comment seen immediately before the current significant
    /// token. Consumed by `take_doc_comment`.
    pending_doc: Option<(Span, String)>,
}

impl<'src> Parser<'src> {
    fn new(src: &'src str, tokens: Vec<Token<'src>>) -> Self {
        let mut significant = Vec::with_capacity(tokens.len());
        for (i, t) in tokens.iter().enumerate() {
            if !t.is_trivia() {
                significant.push(i);
            }
        }
        Self {
            src,
            tokens,
            significant,
            cursor: 0,
            diagnostics: Vec::new(),
            pending_doc: None,
        }
    }

    // ------------------------------------------------------------------
    // Token cursor primitives

    fn token_at(&self, idx: usize) -> &Token<'src> {
        let raw = self.significant[idx.min(self.significant.len() - 1)];
        &self.tokens[raw]
    }

    fn peek_kind(&self) -> SyntaxKind {
        if self.cursor >= self.significant.len() {
            return SyntaxKind::Eof;
        }
        self.token_at(self.cursor).kind
    }

    fn peek_kind_n(&self, n: usize) -> SyntaxKind {
        let idx = self.cursor + n;
        if idx >= self.significant.len() {
            return SyntaxKind::Eof;
        }
        self.token_at(idx).kind
    }

    fn peek_span(&self) -> Span {
        if self.cursor >= self.significant.len() {
            // Span of the EOF marker, if any, else end of source.
            let last = self.tokens.last().map(|t| t.span).unwrap_or_default();
            return last;
        }
        self.token_at(self.cursor).span
    }

    fn peek_text(&self) -> &'src str {
        if self.cursor >= self.significant.len() {
            return "";
        }
        self.token_at(self.cursor).text
    }

    fn bump(&mut self) -> &Token<'src> {
        // Walk through pending trivia between previous cursor and this one to
        // pick up any doc comment immediately preceding the next token.
        self.absorb_leading_doc_comment();
        let raw = self.significant[self.cursor.min(self.significant.len() - 1)];
        self.cursor += 1;
        &self.tokens[raw]
    }

    /// If the next significant token is preceded by one or more `///` doc
    /// comments (allowing whitespace between them), record them so the next
    /// declaration can attach the text. Idempotent.
    fn absorb_leading_doc_comment(&mut self) {
        if self.pending_doc.is_some() {
            return;
        }
        if self.cursor >= self.significant.len() {
            return;
        }
        // Find the range of raw tokens immediately before this significant
        // one and collect any trailing doc comments.
        let raw_end = self.significant[self.cursor];
        let raw_start = if self.cursor == 0 {
            0
        } else {
            self.significant[self.cursor - 1] + 1
        };
        let mut doc_start: Option<u32> = None;
        let mut doc_end: u32 = 0;
        let mut buf = String::new();
        for t in &self.tokens[raw_start..raw_end] {
            match t.kind {
                SyntaxKind::DocComment => {
                    if doc_start.is_none() {
                        doc_start = Some(t.span.start);
                    }
                    doc_end = t.span.end;
                    let stripped = t.text.trim_start_matches('/').trim_start();
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(stripped);
                }
                SyntaxKind::Whitespace | SyntaxKind::Newline => {}
                _ => {
                    // Any other trivia (regular comments) resets the doc
                    // accumulator — only doc comments immediately adjacent
                    // to the declaration attach.
                    doc_start = None;
                    doc_end = 0;
                    buf.clear();
                }
            }
        }
        if let Some(start) = doc_start {
            self.pending_doc = Some((Span::new(start, doc_end), buf));
        }
    }

    fn take_doc_comment(&mut self) -> Option<String> {
        self.absorb_leading_doc_comment();
        self.pending_doc.take().map(|(_, text)| text)
    }

    fn at(&self, kind: SyntaxKind) -> bool {
        self.peek_kind() == kind
    }

    fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: SyntaxKind, what: &str) -> Option<Span> {
        if self.at(kind) {
            let span = self.peek_span();
            self.bump();
            Some(span)
        } else {
            let span = self.peek_span();
            self.diagnostics.push(SyntaxDiagnostic::error(
                span,
                format!(
                    "expected {} ({}), found {}",
                    what,
                    kind,
                    self.peek_describe()
                ),
            ));
            None
        }
    }

    fn peek_describe(&self) -> String {
        if self.cursor >= self.significant.len() {
            return "end of file".into();
        }
        let tok = self.token_at(self.cursor);
        match tok.kind {
            SyntaxKind::Ident | SyntaxKind::QuotedIdent => format!("identifier `{}`", tok.text),
            SyntaxKind::IntNumber
            | SyntaxKind::FloatNumber
            | SyntaxKind::HexNumber
            | SyntaxKind::BinNumber
            | SyntaxKind::OctNumber => format!("number `{}`", tok.text),
            SyntaxKind::String | SyntaxKind::MultilineString => "string literal".into(),
            SyntaxKind::Error => format!("invalid token `{}`", tok.text),
            other => format!("{}", other),
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics
            .push(SyntaxDiagnostic::error(span, message));
    }

    fn is_done(&self) -> bool {
        self.cursor >= self.significant.len() || self.at(SyntaxKind::Eof)
    }

    /// Resync to the next likely declaration boundary.
    fn recover_to_item(&mut self) {
        while !self.is_done() {
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
                _ => {
                    self.bump();
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Module

    fn parse_module(&mut self) -> Module {
        let start = self.peek_span().start;

        // Optional module header.
        let header = self.parse_module_header();

        // Imports.
        let mut imports = Vec::new();
        while matches!(
            self.peek_kind(),
            SyntaxKind::ImportKw | SyntaxKind::ImportGlobKw
        ) {
            if let Some(import) = self.parse_import() {
                imports.push(import);
            } else {
                self.recover_to_item();
            }
        }

        // Items.
        let mut items = Vec::new();
        while !self.is_done() {
            let before = self.cursor;
            match self.parse_item() {
                Some(item) => items.push(item),
                None => {
                    if self.cursor == before {
                        // Didn't make progress — bump to avoid an infinite loop.
                        let span = self.peek_span();
                        self.error(span, "unexpected token");
                        self.bump();
                    }
                    self.recover_to_item();
                }
            }
        }

        let end = self
            .tokens
            .last()
            .map(|t| t.span.end)
            .unwrap_or(self.src.len() as u32);

        Module {
            span: Span::new(start, end),
            header,
            imports,
            items,
        }
    }

    /// True if the upcoming tokens (after any annotations or modifiers) are
    /// `module`, `amends`, or `extends`. Used to decide whether to consume
    /// annotations/modifiers into the module header rather than the first
    /// declaration.
    fn starts_module_header(&self) -> bool {
        let mut i = 0usize;
        loop {
            match self.peek_kind_n(i) {
                SyntaxKind::At => {
                    // Skip `@Name` and optional `@Name { ... }`.
                    i += 1;
                    while matches!(
                        self.peek_kind_n(i),
                        SyntaxKind::Ident | SyntaxKind::QuotedIdent | SyntaxKind::Dot
                    ) {
                        i += 1;
                    }
                    if self.peek_kind_n(i) == SyntaxKind::LBrace {
                        // Skip balanced braces.
                        let mut depth = 1i32;
                        i += 1;
                        while i < self.significant.len() && depth > 0 {
                            match self.peek_kind_n(i) {
                                SyntaxKind::LBrace => depth += 1,
                                SyntaxKind::RBrace => depth -= 1,
                                SyntaxKind::Eof => return false,
                                _ => {}
                            }
                            i += 1;
                        }
                    }
                }
                k if k.is_modifier_kw() => {
                    i += 1;
                }
                SyntaxKind::ModuleKw | SyntaxKind::AmendsKw | SyntaxKind::ExtendsKw => {
                    return true;
                }
                _ => return false,
            }
        }
    }

    fn parse_module_header(&mut self) -> Option<ModuleHeader> {
        // Look ahead past any annotations/modifiers to see whether a header
        // keyword (`module`, `amends`, `extends`) follows. If not, the
        // annotations/modifiers belong to the first declaration, not the
        // module, so we leave them for `parse_item` to consume.
        if !self.starts_module_header() {
            return None;
        }

        let doc = self.take_doc_comment();
        let start = self.peek_span().start;
        let annotations = self.parse_annotations();
        let modifiers = self.parse_modifiers();
        let mut name = None;
        let mut clause = None;

        match self.peek_kind() {
            SyntaxKind::ModuleKw => {
                self.bump();
                name = Some(self.parse_qualified_name("module name"));
            }
            SyntaxKind::AmendsKw => {
                let kw_span = self.peek_span();
                self.bump();
                if let Some(target) = self.parse_string_lit() {
                    clause = Some(ExtendsAmendsClause::Amends {
                        span: kw_span.join(target.span),
                        target,
                    });
                } else {
                    self.error(kw_span, "expected string after `amends`");
                }
            }
            SyntaxKind::ExtendsKw => {
                let kw_span = self.peek_span();
                self.bump();
                if let Some(target) = self.parse_string_lit() {
                    clause = Some(ExtendsAmendsClause::Extends {
                        span: kw_span.join(target.span),
                        target,
                    });
                } else {
                    self.error(kw_span, "expected string after `extends`");
                }
            }
            _ => unreachable!("starts_module_header guarantees one of these"),
        }

        // Optional secondary `extends "..."` / `amends "..."` after `module
        // name`.
        if clause.is_none() {
            match self.peek_kind() {
                SyntaxKind::AmendsKw => {
                    let kw_span = self.peek_span();
                    self.bump();
                    if let Some(target) = self.parse_string_lit() {
                        clause = Some(ExtendsAmendsClause::Amends {
                            span: kw_span.join(target.span),
                            target,
                        });
                    }
                }
                SyntaxKind::ExtendsKw => {
                    let kw_span = self.peek_span();
                    self.bump();
                    if let Some(target) = self.parse_string_lit() {
                        clause = Some(ExtendsAmendsClause::Extends {
                            span: kw_span.join(target.span),
                            target,
                        });
                    }
                }
                _ => {}
            }
        }

        let end = match (&name, &clause) {
            (_, Some(ExtendsAmendsClause::Extends { span, .. }))
            | (_, Some(ExtendsAmendsClause::Amends { span, .. })) => span.end,
            (Some(n), _) => n.span.end,
            _ => start,
        };

        Some(ModuleHeader {
            span: Span::new(start, end),
            doc_comment: doc,
            annotations,
            modifiers,
            name,
            clause,
        })
    }

    fn parse_annotations(&mut self) -> Vec<Annotation> {
        let mut out = Vec::new();
        while self.at(SyntaxKind::At) {
            let start = self.peek_span().start;
            self.bump();
            let name = self.parse_qualified_name("annotation name");
            let mut end = name.span.end;
            let body = if self.at(SyntaxKind::LBrace) {
                let body = self.parse_object_body();
                end = body.span.end;
                Some(body)
            } else {
                None
            };
            out.push(Annotation {
                span: Span::new(start, end),
                name,
                body,
            });
        }
        out
    }

    fn parse_modifiers(&mut self) -> Vec<Modifier> {
        let mut out = Vec::new();
        loop {
            let kind = match self.peek_kind() {
                SyntaxKind::AbstractKw => ModifierKind::Abstract,
                SyntaxKind::OpenKw => ModifierKind::Open,
                SyntaxKind::LocalKw => ModifierKind::Local,
                SyntaxKind::HiddenKw => ModifierKind::Hidden,
                SyntaxKind::FixedKw => ModifierKind::Fixed,
                SyntaxKind::ExternalKw => ModifierKind::External,
                _ => break,
            };
            let span = self.peek_span();
            self.bump();
            out.push(Modifier { span, kind });
        }
        out
    }

    fn parse_qualified_name(&mut self, what: &str) -> QualifiedName {
        let mut segments = Vec::new();
        let start_span = self.peek_span();
        if let Some(id) = self.parse_identifier_opt() {
            segments.push(id);
        } else {
            self.error(
                start_span,
                format!("expected {}, found {}", what, self.peek_describe()),
            );
            return QualifiedName {
                span: start_span,
                segments,
            };
        }
        while self.at(SyntaxKind::Dot) && self.peek_kind_n(1) == SyntaxKind::Ident {
            self.bump();
            if let Some(id) = self.parse_identifier_opt() {
                segments.push(id);
            }
        }
        let end = segments
            .last()
            .map(|s| s.span.end)
            .unwrap_or(start_span.end);
        QualifiedName {
            span: Span::new(start_span.start, end),
            segments,
        }
    }

    fn parse_identifier_opt(&mut self) -> Option<Identifier> {
        match self.peek_kind() {
            SyntaxKind::Ident => {
                let span = self.peek_span();
                let text = self.peek_text().to_owned();
                self.bump();
                Some(Identifier { span, name: text })
            }
            SyntaxKind::QuotedIdent => {
                let span = self.peek_span();
                let text = self.peek_text();
                // Strip backticks.
                let name = text
                    .trim_start_matches('`')
                    .trim_end_matches('`')
                    .to_owned();
                self.bump();
                Some(Identifier { span, name })
            }
            _ => None,
        }
    }

    fn parse_import(&mut self) -> Option<Import> {
        let start_span = self.peek_span();
        let is_glob = matches!(self.peek_kind(), SyntaxKind::ImportGlobKw);
        self.bump();
        let path = self.parse_string_lit()?;
        let mut end = path.span.end;
        let mut alias = None;
        if self.eat(SyntaxKind::AsKw) {
            if let Some(id) = self.parse_identifier_opt() {
                end = id.span.end;
                alias = Some(id);
            } else {
                self.error(self.peek_span(), "expected identifier after `as`");
            }
        }
        Some(Import {
            span: Span::new(start_span.start, end),
            is_glob,
            path,
            alias,
        })
    }

    fn parse_string_lit(&mut self) -> Option<StringLit> {
        match self.peek_kind() {
            SyntaxKind::String | SyntaxKind::MultilineString => {
                let span = self.peek_span();
                let raw = self.peek_text().to_owned();
                let value = decode_simple_string(&raw);
                self.bump();
                Some(StringLit { span, raw, value })
            }
            _ => {
                self.error(self.peek_span(), "expected string literal");
                None
            }
        }
    }

    // ------------------------------------------------------------------
    // Items

    fn parse_item(&mut self) -> Option<Item> {
        let doc = self.take_doc_comment();
        let annotations = self.parse_annotations();
        let modifiers = self.parse_modifiers();

        match self.peek_kind() {
            SyntaxKind::ClassKw => Some(Item::Class(self.parse_class(doc, annotations, modifiers))),
            SyntaxKind::TypeAliasKw => Some(Item::TypeAlias(self.parse_typealias(
                doc,
                annotations,
                modifiers,
            ))),
            SyntaxKind::FunctionKw => {
                Some(Item::Method(self.parse_method(doc, annotations, modifiers)))
            }
            SyntaxKind::Ident | SyntaxKind::QuotedIdent => Some(Item::Property(
                self.parse_property(doc, annotations, modifiers),
            )),
            SyntaxKind::Eof => None,
            _ => {
                let span = self.peek_span();
                self.error(span, format!("unexpected {}", self.peek_describe()));
                self.bump();
                Some(Item::Error(ErrorItem {
                    span,
                    message: "unexpected token".into(),
                }))
            }
        }
    }

    fn parse_class(
        &mut self,
        doc: Option<String>,
        annotations: Vec<Annotation>,
        modifiers: Vec<Modifier>,
    ) -> ClassDecl {
        let start = annotations
            .first()
            .map(|a| a.span.start)
            .or_else(|| modifiers.first().map(|m| m.span.start))
            .unwrap_or(self.peek_span().start);
        self.bump(); // class
        let name = self.parse_identifier_opt().unwrap_or_else(|| {
            let span = self.peek_span();
            self.error(span, "expected class name");
            Identifier {
                span,
                name: String::new(),
            }
        });
        let type_parameters = self.parse_type_parameters();
        let extends = if self.eat(SyntaxKind::ExtendsKw) {
            Some(self.parse_type())
        } else {
            None
        };
        let (body, end) = if self.at(SyntaxKind::LBrace) {
            let b = self.parse_class_body();
            let e = b.span.end;
            (Some(b), e)
        } else {
            (
                None,
                extends
                    .as_ref()
                    .map(|t| t.span().end)
                    .unwrap_or(name.span.end),
            )
        };
        ClassDecl {
            span: Span::new(start, end),
            doc_comment: doc,
            annotations,
            modifiers,
            name,
            type_parameters,
            extends,
            body,
        }
    }

    fn parse_class_body(&mut self) -> ClassBody {
        let start = self.peek_span().start;
        self.bump(); // {
        let mut members = Vec::new();
        while !self.is_done() && !self.at(SyntaxKind::RBrace) {
            let before = self.cursor;
            let doc = self.take_doc_comment();
            let annotations = self.parse_annotations();
            let modifiers = self.parse_modifiers();
            match self.peek_kind() {
                SyntaxKind::FunctionKw => members.push(ClassMember::Method(self.parse_method(
                    doc,
                    annotations,
                    modifiers,
                ))),
                SyntaxKind::Ident | SyntaxKind::QuotedIdent => members.push(ClassMember::Property(
                    self.parse_property(doc, annotations, modifiers),
                )),
                _ => {
                    let span = self.peek_span();
                    self.error(
                        span,
                        format!("unexpected {} in class body", self.peek_describe()),
                    );
                    if self.cursor == before {
                        self.bump();
                    }
                }
            }
        }
        let end = self.peek_span().end;
        self.expect(SyntaxKind::RBrace, "closing `}`");
        ClassBody {
            span: Span::new(start, end),
            members,
        }
    }

    fn parse_typealias(
        &mut self,
        doc: Option<String>,
        annotations: Vec<Annotation>,
        modifiers: Vec<Modifier>,
    ) -> TypeAliasDecl {
        let start = annotations
            .first()
            .map(|a| a.span.start)
            .or_else(|| modifiers.first().map(|m| m.span.start))
            .unwrap_or(self.peek_span().start);
        self.bump(); // typealias
        let name = self.parse_identifier_opt().unwrap_or_else(|| {
            let span = self.peek_span();
            self.error(span, "expected type alias name");
            Identifier {
                span,
                name: String::new(),
            }
        });
        let type_parameters = self.parse_type_parameters();
        let mut aliased = None;
        let mut end = name.span.end;
        if self.eat(SyntaxKind::Eq) {
            let ty = self.parse_type();
            end = ty.span().end;
            aliased = Some(ty);
        } else {
            self.error(self.peek_span(), "expected `=` in `typealias`");
        }
        TypeAliasDecl {
            span: Span::new(start, end),
            doc_comment: doc,
            annotations,
            modifiers,
            name,
            type_parameters,
            aliased,
        }
    }

    fn parse_property(
        &mut self,
        doc: Option<String>,
        annotations: Vec<Annotation>,
        modifiers: Vec<Modifier>,
    ) -> PropertyDecl {
        let start = annotations
            .first()
            .map(|a| a.span.start)
            .or_else(|| modifiers.first().map(|m| m.span.start))
            .unwrap_or(self.peek_span().start);
        let name = self
            .parse_identifier_opt()
            .expect("called with ident at cursor");
        let mut end = name.span.end;
        let ty = if self.eat(SyntaxKind::Colon) {
            let t = self.parse_type();
            end = t.span().end;
            Some(t)
        } else {
            None
        };
        let value = if self.eat(SyntaxKind::Eq) {
            let e = self.parse_expr();
            end = e.span().end;
            Some(PropertyValue::Expr(e))
        } else if self.at(SyntaxKind::LBrace) {
            let b = self.parse_object_body();
            end = b.span.end;
            Some(PropertyValue::ObjectBody(b))
        } else {
            None
        };
        PropertyDecl {
            span: Span::new(start, end),
            doc_comment: doc,
            annotations,
            modifiers,
            name,
            ty,
            value,
        }
    }

    fn parse_method(
        &mut self,
        doc: Option<String>,
        annotations: Vec<Annotation>,
        modifiers: Vec<Modifier>,
    ) -> MethodDecl {
        let start = annotations
            .first()
            .map(|a| a.span.start)
            .or_else(|| modifiers.first().map(|m| m.span.start))
            .unwrap_or(self.peek_span().start);
        self.bump(); // function
        let name = self.parse_identifier_opt().unwrap_or_else(|| {
            let span = self.peek_span();
            self.error(span, "expected function name");
            Identifier {
                span,
                name: String::new(),
            }
        });
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameter_list();
        let mut end = name.span.end;
        let return_type = if self.eat(SyntaxKind::Colon) {
            let t = self.parse_type();
            end = t.span().end;
            Some(t)
        } else {
            None
        };
        let body = if self.eat(SyntaxKind::Eq) {
            let e = self.parse_expr();
            end = e.span().end;
            Some(e)
        } else {
            None
        };
        MethodDecl {
            span: Span::new(start, end),
            doc_comment: doc,
            annotations,
            modifiers,
            name,
            type_parameters,
            parameters,
            return_type,
            body,
        }
    }

    fn parse_type_parameters(&mut self) -> Vec<TypeParameter> {
        if !self.at(SyntaxKind::Lt) {
            return Vec::new();
        }
        self.bump();
        let mut out = Vec::new();
        while !self.is_done() && !self.at(SyntaxKind::Gt) {
            let start = self.peek_span().start;
            let variance = match self.peek_kind() {
                SyntaxKind::InKw => {
                    self.bump();
                    Some(Variance::In)
                }
                SyntaxKind::OutKw => {
                    self.bump();
                    Some(Variance::Out)
                }
                _ => None,
            };
            let name = self.parse_identifier_opt().unwrap_or_else(|| Identifier {
                span: self.peek_span(),
                name: String::new(),
            });
            let end = name.span.end;
            out.push(TypeParameter {
                span: Span::new(start, end),
                variance,
                name,
            });
            if !self.eat(SyntaxKind::Comma) {
                break;
            }
        }
        self.expect(SyntaxKind::Gt, "closing `>`");
        out
    }

    fn parse_parameter_list(&mut self) -> Vec<Parameter> {
        if !self.eat(SyntaxKind::LParen) {
            self.error(self.peek_span(), "expected `(`");
            return Vec::new();
        }
        let mut out = Vec::new();
        while !self.is_done() && !self.at(SyntaxKind::RParen) {
            let p = self.parse_parameter();
            out.push(p);
            if !self.eat(SyntaxKind::Comma) {
                break;
            }
        }
        self.expect(SyntaxKind::RParen, "closing `)`");
        out
    }

    fn parse_parameter(&mut self) -> Parameter {
        let start = self.peek_span().start;
        let name = self.parse_identifier_opt().unwrap_or_else(|| {
            let span = self.peek_span();
            self.error(span, "expected parameter name");
            Identifier {
                span,
                name: String::new(),
            }
        });
        let mut end = name.span.end;
        let ty = if self.eat(SyntaxKind::Colon) {
            let t = self.parse_type();
            end = t.span().end;
            Some(t)
        } else {
            None
        };
        Parameter {
            span: Span::new(start, end),
            name,
            ty,
        }
    }

    // ------------------------------------------------------------------
    // Types

    fn parse_type(&mut self) -> TypeRef {
        let first = self.parse_type_nullable();
        if !self.at(SyntaxKind::Pipe) {
            return first;
        }
        let start = first.span().start;
        let mut members = vec![first];
        while self.eat(SyntaxKind::Pipe) {
            members.push(self.parse_type_nullable());
        }
        let end = members.last().map(|t| t.span().end).unwrap_or(start);
        TypeRef::Union {
            span: Span::new(start, end),
            members,
        }
    }

    fn parse_type_nullable(&mut self) -> TypeRef {
        let mut t = self.parse_type_primary();
        while self.at(SyntaxKind::Question) {
            let qspan = self.peek_span();
            self.bump();
            let span = Span::new(t.span().start, qspan.end);
            t = TypeRef::Nullable {
                span,
                inner: Box::new(t),
            };
        }
        t
    }

    fn parse_type_primary(&mut self) -> TypeRef {
        match self.peek_kind() {
            SyntaxKind::LParen => {
                let start = self.peek_span().start;
                self.bump();
                // Could be function type `(A, B) -> C` or parenthesised type.
                let mut params = Vec::new();
                let mut saw_comma = false;
                if !self.at(SyntaxKind::RParen) {
                    params.push(self.parse_type());
                    while self.eat(SyntaxKind::Comma) {
                        saw_comma = true;
                        params.push(self.parse_type());
                    }
                }
                let close = self.peek_span();
                self.expect(SyntaxKind::RParen, "closing `)`");
                if self.at(SyntaxKind::Arrow) {
                    self.bump();
                    let result = self.parse_type();
                    let span = Span::new(start, result.span().end);
                    return TypeRef::Function {
                        span,
                        parameters: params,
                        result: Box::new(result),
                    };
                }
                if params.len() == 1 && !saw_comma {
                    let inner = params.pop().unwrap();
                    return TypeRef::Parenthesized {
                        span: Span::new(start, close.end),
                        inner: Box::new(inner),
                    };
                }
                // `(A, B)` without arrow is unusual at the type level —
                // Pkl has no tuple types. Suggest `Pair<A, B>` (the
                // closest stdlib equivalent) and emit a parse-time
                // diagnostic so editors light it up.
                let span = Span::new(start, close.end);
                self.error(
                    span,
                    "Pkl has no tuple types; use `Pair<A, B>` for a 2-tuple or write a function type \
                     `(A, B) -> R`",
                );
                TypeRef::Error {
                    span,
                    message: "expected a `Pair<A, B>` or function type `(A, B) -> R`".into(),
                }
            }
            SyntaxKind::UnknownKw => {
                let span = self.peek_span();
                self.bump();
                TypeRef::Unknown(span)
            }
            SyntaxKind::NothingKw => {
                let span = self.peek_span();
                self.bump();
                TypeRef::Nothing(span)
            }
            SyntaxKind::ModuleKw => {
                let span = self.peek_span();
                self.bump();
                TypeRef::Module(span)
            }
            SyntaxKind::String | SyntaxKind::MultilineString => {
                let s = self.parse_string_lit().expect("checked above");
                TypeRef::StringLiteral(s)
            }
            SyntaxKind::Ident | SyntaxKind::QuotedIdent => {
                let name = self.parse_qualified_name("type name");
                let mut arguments = Vec::new();
                let mut end = name.span.end;
                if self.at(SyntaxKind::Lt) {
                    self.bump();
                    while !self.is_done() && !self.at(SyntaxKind::Gt) {
                        arguments.push(self.parse_type());
                        if !self.eat(SyntaxKind::Comma) {
                            break;
                        }
                    }
                    let span = self.peek_span();
                    end = span.end;
                    self.expect(SyntaxKind::Gt, "closing `>`");
                }
                TypeRef::Named {
                    span: Span::new(name.span.start, end),
                    name,
                    arguments,
                }
            }
            _ => {
                let span = self.peek_span();
                self.error(
                    span,
                    format!("expected type, found {}", self.peek_describe()),
                );
                self.bump();
                TypeRef::Error {
                    span,
                    message: "expected type".into(),
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Expressions

    fn parse_expr(&mut self) -> Expr {
        self.parse_expr_pipeline()
    }

    fn parse_expr_pipeline(&mut self) -> Expr {
        let mut lhs = self.parse_expr_null_coalesce();
        while self.at(SyntaxKind::PipeGt) {
            self.bump();
            let rhs = self.parse_expr_null_coalesce();
            let span = Span::new(lhs.span().start, rhs.span().end);
            lhs = Expr::Binary {
                span,
                op: BinaryOp::Pipeline,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        lhs
    }

    fn parse_expr_null_coalesce(&mut self) -> Expr {
        let mut lhs = self.parse_expr_or();
        while self.at(SyntaxKind::QuestionQuestion) {
            self.bump();
            let rhs = self.parse_expr_or();
            let span = Span::new(lhs.span().start, rhs.span().end);
            lhs = Expr::Binary {
                span,
                op: BinaryOp::NullCoalesce,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        lhs
    }

    fn parse_expr_or(&mut self) -> Expr {
        let mut lhs = self.parse_expr_and();
        while self.at(SyntaxKind::Pipe) && self.peek_kind_n(1) == SyntaxKind::Pipe {
            // Treat consecutive `|` `|` as `||`.
            self.bump();
            self.bump();
            let rhs = self.parse_expr_and();
            let span = Span::new(lhs.span().start, rhs.span().end);
            lhs = Expr::Binary {
                span,
                op: BinaryOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        lhs
    }

    fn parse_expr_and(&mut self) -> Expr {
        let mut lhs = self.parse_expr_eq();
        while self.at(SyntaxKind::Amp) && self.peek_kind_n(1) == SyntaxKind::Amp {
            self.bump();
            self.bump();
            let rhs = self.parse_expr_eq();
            let span = Span::new(lhs.span().start, rhs.span().end);
            lhs = Expr::Binary {
                span,
                op: BinaryOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        lhs
    }

    fn parse_expr_eq(&mut self) -> Expr {
        let mut lhs = self.parse_expr_cmp();
        loop {
            let op = match self.peek_kind() {
                SyntaxKind::EqEq => BinaryOp::Eq,
                SyntaxKind::BangEq => BinaryOp::NotEq,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_expr_cmp();
            let span = Span::new(lhs.span().start, rhs.span().end);
            lhs = Expr::Binary {
                span,
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        lhs
    }

    fn parse_expr_cmp(&mut self) -> Expr {
        let mut lhs = self.parse_expr_add();
        loop {
            match self.peek_kind() {
                SyntaxKind::Lt => {
                    self.bump();
                    let rhs = self.parse_expr_add();
                    let span = Span::new(lhs.span().start, rhs.span().end);
                    lhs = Expr::Binary {
                        span,
                        op: BinaryOp::Lt,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    };
                }
                SyntaxKind::LtEq => {
                    self.bump();
                    let rhs = self.parse_expr_add();
                    let span = Span::new(lhs.span().start, rhs.span().end);
                    lhs = Expr::Binary {
                        span,
                        op: BinaryOp::LtEq,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    };
                }
                SyntaxKind::Gt => {
                    self.bump();
                    let rhs = self.parse_expr_add();
                    let span = Span::new(lhs.span().start, rhs.span().end);
                    lhs = Expr::Binary {
                        span,
                        op: BinaryOp::Gt,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    };
                }
                SyntaxKind::GtEq => {
                    self.bump();
                    let rhs = self.parse_expr_add();
                    let span = Span::new(lhs.span().start, rhs.span().end);
                    lhs = Expr::Binary {
                        span,
                        op: BinaryOp::GtEq,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    };
                }
                SyntaxKind::IsKw => {
                    self.bump();
                    let ty = self.parse_type_nullable();
                    let span = Span::new(lhs.span().start, ty.span().end);
                    lhs = Expr::TypeCheck {
                        span,
                        operand: Box::new(lhs),
                        ty,
                    };
                }
                SyntaxKind::AsKw => {
                    self.bump();
                    let ty = self.parse_type_nullable();
                    let span = Span::new(lhs.span().start, ty.span().end);
                    lhs = Expr::TypeCast {
                        span,
                        operand: Box::new(lhs),
                        ty,
                    };
                }
                _ => break,
            }
        }
        lhs
    }

    fn parse_expr_add(&mut self) -> Expr {
        let mut lhs = self.parse_expr_mul();
        loop {
            let op = match self.peek_kind() {
                SyntaxKind::Plus => BinaryOp::Add,
                SyntaxKind::Minus => BinaryOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_expr_mul();
            let span = Span::new(lhs.span().start, rhs.span().end);
            lhs = Expr::Binary {
                span,
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        lhs
    }

    fn parse_expr_mul(&mut self) -> Expr {
        let mut lhs = self.parse_expr_pow();
        loop {
            let op = match self.peek_kind() {
                SyntaxKind::Star => BinaryOp::Mul,
                SyntaxKind::Slash => BinaryOp::Div,
                SyntaxKind::Percent => BinaryOp::Rem,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_expr_pow();
            let span = Span::new(lhs.span().start, rhs.span().end);
            lhs = Expr::Binary {
                span,
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        lhs
    }

    fn parse_expr_pow(&mut self) -> Expr {
        let lhs = self.parse_expr_unary();
        if self.at(SyntaxKind::StarStar) {
            self.bump();
            let rhs = self.parse_expr_pow();
            let span = Span::new(lhs.span().start, rhs.span().end);
            return Expr::Binary {
                span,
                op: BinaryOp::Pow,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        lhs
    }

    fn parse_expr_unary(&mut self) -> Expr {
        match self.peek_kind() {
            SyntaxKind::Minus => {
                let start = self.peek_span().start;
                self.bump();
                let operand = self.parse_expr_unary();
                let span = Span::new(start, operand.span().end);
                Expr::Unary {
                    span,
                    op: UnaryOp::Neg,
                    operand: Box::new(operand),
                }
            }
            SyntaxKind::Bang => {
                let start = self.peek_span().start;
                self.bump();
                let operand = self.parse_expr_unary();
                let span = Span::new(start, operand.span().end);
                Expr::Unary {
                    span,
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                }
            }
            _ => self.parse_expr_postfix(),
        }
    }

    fn parse_expr_postfix(&mut self) -> Expr {
        let mut expr = self.parse_expr_primary();
        loop {
            match self.peek_kind() {
                SyntaxKind::Dot => {
                    self.bump();
                    let name = self.parse_identifier_opt().unwrap_or_else(|| Identifier {
                        span: self.peek_span(),
                        name: String::new(),
                    });
                    let span = Span::new(expr.span().start, name.span.end);
                    expr = Expr::Member {
                        span,
                        receiver: Box::new(expr),
                        nullable: false,
                        name,
                    };
                }
                SyntaxKind::QuestionDot => {
                    self.bump();
                    let name = self.parse_identifier_opt().unwrap_or_else(|| Identifier {
                        span: self.peek_span(),
                        name: String::new(),
                    });
                    let span = Span::new(expr.span().start, name.span.end);
                    expr = Expr::Member {
                        span,
                        receiver: Box::new(expr),
                        nullable: true,
                        name,
                    };
                }
                SyntaxKind::LParen => {
                    let (args, end) = self.parse_arg_list();
                    let span = Span::new(expr.span().start, end);
                    expr = Expr::Call {
                        span,
                        callee: Box::new(expr),
                        type_args: Vec::new(),
                        args,
                    };
                }
                SyntaxKind::LBracket => {
                    self.bump();
                    let index = self.parse_expr();
                    let end_span = self.peek_span();
                    self.expect(SyntaxKind::RBracket, "closing `]`");
                    let span = Span::new(expr.span().start, end_span.end);
                    expr = Expr::Index {
                        span,
                        receiver: Box::new(expr),
                        index: Box::new(index),
                    };
                }
                SyntaxKind::Bang if self.peek_kind_n(1) == SyntaxKind::Bang => {
                    let start = expr.span().start;
                    self.bump();
                    let end_span = self.peek_span();
                    self.bump();
                    expr = Expr::NonNull {
                        span: Span::new(start, end_span.end),
                        operand: Box::new(expr),
                    };
                }
                SyntaxKind::LBrace => {
                    // `expr { ... }` amends an existing object literal.
                    let body = self.parse_object_body();
                    let span = Span::new(expr.span().start, body.span.end);
                    expr = Expr::AmendsObject {
                        span,
                        base: Box::new(expr),
                        body,
                    };
                }
                _ => break,
            }
        }
        expr
    }

    fn parse_arg_list(&mut self) -> (Vec<Expr>, u32) {
        self.bump(); // (
        let mut args = Vec::new();
        while !self.is_done() && !self.at(SyntaxKind::RParen) {
            args.push(self.parse_expr());
            if !self.eat(SyntaxKind::Comma) {
                break;
            }
        }
        let end = self.peek_span().end;
        self.expect(SyntaxKind::RParen, "closing `)`");
        (args, end)
    }

    fn parse_expr_primary(&mut self) -> Expr {
        match self.peek_kind() {
            SyntaxKind::IntNumber
            | SyntaxKind::HexNumber
            | SyntaxKind::BinNumber
            | SyntaxKind::OctNumber => {
                let span = self.peek_span();
                let raw = self.peek_text().to_owned();
                self.bump();
                Expr::Literal(Literal::Int { span, raw })
            }
            SyntaxKind::FloatNumber => {
                let span = self.peek_span();
                let raw = self.peek_text().to_owned();
                self.bump();
                Expr::Literal(Literal::Float { span, raw })
            }
            SyntaxKind::String | SyntaxKind::MultilineString => {
                let s = self.parse_string_lit().expect("matched above");
                Expr::Literal(Literal::String(s))
            }
            SyntaxKind::TrueKw => {
                let span = self.peek_span();
                self.bump();
                Expr::Literal(Literal::Bool { span, value: true })
            }
            SyntaxKind::FalseKw => {
                let span = self.peek_span();
                self.bump();
                Expr::Literal(Literal::Bool { span, value: false })
            }
            SyntaxKind::NullKw => {
                let span = self.peek_span();
                self.bump();
                Expr::Literal(Literal::Null { span })
            }
            SyntaxKind::ThisKw => {
                let span = self.peek_span();
                self.bump();
                Expr::SpecialIdent {
                    span,
                    kind: SpecialIdentKind::This,
                }
            }
            SyntaxKind::SuperKw => {
                let span = self.peek_span();
                self.bump();
                Expr::SpecialIdent {
                    span,
                    kind: SpecialIdentKind::Super,
                }
            }
            SyntaxKind::OuterKw => {
                let span = self.peek_span();
                self.bump();
                Expr::SpecialIdent {
                    span,
                    kind: SpecialIdentKind::Outer,
                }
            }
            SyntaxKind::ModuleKw => {
                let span = self.peek_span();
                self.bump();
                Expr::SpecialIdent {
                    span,
                    kind: SpecialIdentKind::Module,
                }
            }
            SyntaxKind::LParen => {
                // Could be lambda `(a, b) -> body` or just a parenthesised
                // expression. We disambiguate by trying lambda first when
                // the shape matches.
                if self.looks_like_lambda() {
                    return self.parse_lambda();
                }
                let start = self.peek_span().start;
                self.bump();
                let inner = self.parse_expr();
                let end_span = self.peek_span();
                self.expect(SyntaxKind::RParen, "closing `)`");
                Expr::Paren {
                    span: Span::new(start, end_span.end),
                    inner: Box::new(inner),
                }
            }
            SyntaxKind::IfKw => self.parse_if(),
            SyntaxKind::LetKw => self.parse_let(),
            SyntaxKind::NewKw => self.parse_new(),
            SyntaxKind::ThrowKw => {
                let start = self.peek_span().start;
                self.bump();
                self.expect(SyntaxKind::LParen, "`(`");
                let arg = self.parse_expr();
                let end_span = self.peek_span();
                self.expect(SyntaxKind::RParen, "closing `)`");
                Expr::Throw {
                    span: Span::new(start, end_span.end),
                    argument: Box::new(arg),
                }
            }
            SyntaxKind::TraceKw => {
                let start = self.peek_span().start;
                self.bump();
                self.expect(SyntaxKind::LParen, "`(`");
                let arg = self.parse_expr();
                let end_span = self.peek_span();
                self.expect(SyntaxKind::RParen, "closing `)`");
                Expr::Trace {
                    span: Span::new(start, end_span.end),
                    argument: Box::new(arg),
                }
            }
            SyntaxKind::ReadKw | SyntaxKind::ReadOrNullKw | SyntaxKind::ReadGlobKw => {
                let kind = match self.peek_kind() {
                    SyntaxKind::ReadKw => ReadKind::Read,
                    SyntaxKind::ReadOrNullKw => ReadKind::ReadOrNull,
                    _ => ReadKind::ReadGlob,
                };
                let start = self.peek_span().start;
                self.bump();
                self.expect(SyntaxKind::LParen, "`(`");
                let arg = self.parse_expr();
                let end_span = self.peek_span();
                self.expect(SyntaxKind::RParen, "closing `)`");
                Expr::Read {
                    span: Span::new(start, end_span.end),
                    kind,
                    argument: Box::new(arg),
                }
            }
            SyntaxKind::Ident | SyntaxKind::QuotedIdent => {
                let id = self.parse_identifier_opt().unwrap();
                Expr::Ident(id)
            }
            _ => {
                let span = self.peek_span();
                let desc = self.peek_describe();
                self.error(span, format!("expected expression, found {}", desc));
                if !self.is_done() {
                    self.bump();
                }
                Expr::Error {
                    span,
                    message: "expected expression".into(),
                }
            }
        }
    }

    fn looks_like_lambda(&self) -> bool {
        // Look ahead through a balanced `(...)` and check for `->`.
        let mut depth = 0i32;
        let mut i = self.cursor;
        while i < self.significant.len() {
            match self.token_at(i).kind {
                SyntaxKind::LParen => depth += 1,
                SyntaxKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(self.peek_kind_n(i - self.cursor + 1), SyntaxKind::Arrow);
                    }
                }
                SyntaxKind::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    fn parse_lambda(&mut self) -> Expr {
        let start = self.peek_span().start;
        let parameters = self.parse_parameter_list();
        self.expect(SyntaxKind::Arrow, "`->`");
        let body = self.parse_expr();
        Expr::Lambda {
            span: Span::new(start, body.span().end),
            parameters,
            body: Box::new(body),
        }
    }

    fn parse_if(&mut self) -> Expr {
        let start = self.peek_span().start;
        self.bump(); // if
        self.expect(SyntaxKind::LParen, "`(` after `if`");
        let cond = self.parse_expr();
        self.expect(SyntaxKind::RParen, "closing `)`");
        let then_branch = self.parse_expr();
        self.expect(SyntaxKind::ElseKw, "`else`");
        let else_branch = self.parse_expr();
        Expr::If {
            span: Span::new(start, else_branch.span().end),
            cond: Box::new(cond),
            then_branch: Box::new(then_branch),
            else_branch: Box::new(else_branch),
        }
    }

    fn parse_let(&mut self) -> Expr {
        let start = self.peek_span().start;
        self.bump(); // let
        self.expect(SyntaxKind::LParen, "`(` after `let`");
        let binding = self.parse_parameter();
        self.expect(SyntaxKind::Eq, "`=`");
        let value = self.parse_expr();
        self.expect(SyntaxKind::RParen, "closing `)`");
        let body = self.parse_expr();
        Expr::Let {
            span: Span::new(start, body.span().end),
            binding: Box::new(binding),
            value: Box::new(value),
            body: Box::new(body),
        }
    }

    fn parse_new(&mut self) -> Expr {
        let start = self.peek_span().start;
        self.bump(); // new
        let ty = if !self.at(SyntaxKind::LBrace) {
            Some(self.parse_type())
        } else {
            None
        };
        let body = self.parse_object_body();
        Expr::New {
            span: Span::new(start, body.span.end),
            ty,
            body,
        }
    }

    // ------------------------------------------------------------------
    // Object bodies

    fn parse_object_body(&mut self) -> ObjectBody {
        let start = self.peek_span().start;
        self.expect(SyntaxKind::LBrace, "`{`");
        // Optional lambda-style parameter list at the head:
        //   { x, y -> body }
        let parameters = if self.looks_like_object_params() {
            let mut params = Vec::new();
            loop {
                let p = self.parse_parameter();
                params.push(p);
                if !self.eat(SyntaxKind::Comma) {
                    break;
                }
            }
            self.expect(SyntaxKind::Arrow, "`->`");
            params
        } else {
            Vec::new()
        };

        let mut members = Vec::new();
        while !self.is_done() && !self.at(SyntaxKind::RBrace) {
            let before = self.cursor;
            if let Some(m) = self.parse_object_member() {
                members.push(m);
            } else if self.cursor == before {
                self.bump();
            }
            // Optional semicolons / commas between elements.
            while self.eat(SyntaxKind::Semicolon) || self.eat(SyntaxKind::Comma) {}
        }
        let end_span = self.peek_span();
        self.expect(SyntaxKind::RBrace, "closing `}`");
        ObjectBody {
            span: Span::new(start, end_span.end),
            parameters,
            members,
        }
    }

    fn looks_like_object_params(&self) -> bool {
        // Heuristic: `ident (`,` ident)* `->`
        let mut i = 0usize;
        loop {
            let k = self.peek_kind_n(i);
            if !matches!(k, SyntaxKind::Ident | SyntaxKind::QuotedIdent) {
                return false;
            }
            i += 1;
            // optional type annotation
            if self.peek_kind_n(i) == SyntaxKind::Colon {
                // Skip until comma or arrow at depth 0.
                let mut depth = 0i32;
                i += 1;
                while i < self.significant.len() {
                    match self.peek_kind_n(i) {
                        SyntaxKind::Lt | SyntaxKind::LParen => depth += 1,
                        SyntaxKind::Gt | SyntaxKind::RParen => depth -= 1,
                        SyntaxKind::Comma | SyntaxKind::Arrow if depth == 0 => break,
                        SyntaxKind::RBrace | SyntaxKind::Eof => return false,
                        _ => {}
                    }
                    i += 1;
                }
            }
            match self.peek_kind_n(i) {
                SyntaxKind::Arrow => return true,
                SyntaxKind::Comma => {
                    i += 1;
                    continue;
                }
                _ => return false,
            }
        }
    }

    fn parse_object_member(&mut self) -> Option<ObjectMember> {
        match self.peek_kind() {
            SyntaxKind::Ellipsis => {
                let start = self.peek_span().start;
                self.bump();
                let expr = self.parse_expr();
                Some(ObjectMember::Spread {
                    span: Span::new(start, expr.span().end),
                    expr,
                })
            }
            SyntaxKind::WhenKw => {
                let start = self.peek_span().start;
                self.bump();
                self.expect(SyntaxKind::LParen, "`(` after `when`");
                let cond = self.parse_expr();
                self.expect(SyntaxKind::RParen, "closing `)`");
                let then_body = self.parse_object_body();
                let mut end = then_body.span.end;
                let else_body = if self.eat(SyntaxKind::ElseKw) {
                    let b = self.parse_object_body();
                    end = b.span.end;
                    Some(b)
                } else {
                    None
                };
                Some(ObjectMember::When {
                    span: Span::new(start, end),
                    cond,
                    then_body,
                    else_body,
                })
            }
            SyntaxKind::ForKw => {
                let start = self.peek_span().start;
                self.bump();
                self.expect(SyntaxKind::LParen, "`(` after `for`");
                let mut bindings = Vec::new();
                bindings.push(self.parse_parameter());
                while self.eat(SyntaxKind::Comma) {
                    bindings.push(self.parse_parameter());
                }
                self.expect(SyntaxKind::InKw, "`in`");
                let iterable = self.parse_expr();
                self.expect(SyntaxKind::RParen, "closing `)`");
                let body = self.parse_object_body();
                Some(ObjectMember::For {
                    span: Span::new(start, body.span.end),
                    bindings,
                    iterable,
                    body,
                })
            }
            SyntaxKind::LBracket => {
                let start = self.peek_span().start;
                self.bump();
                let key = self.parse_expr();
                self.expect(SyntaxKind::RBracket, "closing `]`");
                if self.eat(SyntaxKind::Eq) {
                    let value = self.parse_expr();
                    let end = value.span().end;
                    Some(ObjectMember::Entry {
                        span: Span::new(start, end),
                        key,
                        value: PropertyValue::Expr(value),
                    })
                } else if self.at(SyntaxKind::LBrace) {
                    let body = self.parse_object_body();
                    let end = body.span.end;
                    Some(ObjectMember::Entry {
                        span: Span::new(start, end),
                        key,
                        value: PropertyValue::ObjectBody(body),
                    })
                } else {
                    self.error(self.peek_span(), "expected `=` or `{` after `[key]`");
                    None
                }
            }
            SyntaxKind::FunctionKw => {
                let m = self.parse_method(None, Vec::new(), Vec::new());
                Some(ObjectMember::Method(m))
            }
            SyntaxKind::AbstractKw
            | SyntaxKind::OpenKw
            | SyntaxKind::LocalKw
            | SyntaxKind::HiddenKw
            | SyntaxKind::FixedKw
            | SyntaxKind::ExternalKw => {
                let doc = self.take_doc_comment();
                let annotations = self.parse_annotations();
                let modifiers = self.parse_modifiers();
                match self.peek_kind() {
                    SyntaxKind::FunctionKw => Some(ObjectMember::Method(self.parse_method(
                        doc,
                        annotations,
                        modifiers,
                    ))),
                    SyntaxKind::Ident | SyntaxKind::QuotedIdent => Some(ObjectMember::Property(
                        self.parse_property(doc, annotations, modifiers),
                    )),
                    _ => None,
                }
            }
            SyntaxKind::Ident | SyntaxKind::QuotedIdent => {
                // Could be either a property `name = ...` / `name { ... }`
                // / `name : T = ...` OR a bare expression element.
                if self.looks_like_property_decl() {
                    let p = self.parse_property(None, Vec::new(), Vec::new());
                    Some(ObjectMember::Property(p))
                } else {
                    let e = self.parse_expr();
                    Some(ObjectMember::Element(e))
                }
            }
            SyntaxKind::At => {
                let doc = self.take_doc_comment();
                let annotations = self.parse_annotations();
                let modifiers = self.parse_modifiers();
                match self.peek_kind() {
                    SyntaxKind::FunctionKw => Some(ObjectMember::Method(self.parse_method(
                        doc,
                        annotations,
                        modifiers,
                    ))),
                    _ => Some(ObjectMember::Property(self.parse_property(
                        doc,
                        annotations,
                        modifiers,
                    ))),
                }
            }
            _ => {
                let e = self.parse_expr();
                Some(ObjectMember::Element(e))
            }
        }
    }

    fn looks_like_property_decl(&self) -> bool {
        // ident (`:` type)? (`=` | `{`)
        let mut i = 0usize;
        if !matches!(
            self.peek_kind_n(i),
            SyntaxKind::Ident | SyntaxKind::QuotedIdent
        ) {
            return false;
        }
        i += 1;
        if self.peek_kind_n(i) == SyntaxKind::Colon {
            // Walk to `=` or `{` (or stop on terminators).
            let mut depth = 0i32;
            i += 1;
            while i < self.significant.len() {
                match self.peek_kind_n(i) {
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
            return false;
        }
        matches!(self.peek_kind_n(i), SyntaxKind::Eq | SyntaxKind::LBrace)
    }
}

// ----------------------------------------------------------------------
// Helpers

fn decode_simple_string(raw: &str) -> Option<String> {
    // Only handle the `"..."` single-line form here. Anything starting with
    // `"""` (multi-line) or `#` (custom delimiter) is left for the analyzer
    // to decode once it understands interpolation.
    let bytes = raw.as_bytes();
    if bytes.first() != Some(&b'"') || bytes.last() != Some(&b'"') || bytes.len() < 2 {
        return None;
    }
    if raw.starts_with("\"\"\"") {
        return None;
    }
    let inner = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('\'') => out.push('\''),
                Some('0') => out.push('\0'),
                Some('s') => out.push(' '),
                Some('(') => {
                    // Interpolation — we can't decode without parsing the
                    // contained expression. Bail out and let later passes
                    // handle it.
                    return None;
                }
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => return None,
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

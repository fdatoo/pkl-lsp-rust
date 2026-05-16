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

use std::path::{Path, PathBuf};

use pkl_analyze::infer::{stdlib_members_of, user_members_of};
use pkl_analyze::{FsLoaderConfig, ModuleGraph, SymbolKind, Ty, WorkspaceIndex};
use pkl_stdlib::MemberKind;
use pkl_syntax::cst::{self, AstNode, Expr, Item, ObjectMember, PropertyValue};
use tower_lsp::lsp_types::*;

use crate::config::CompletionConfig;
use crate::document::Document;

struct KeywordSpec {
    label: &'static str,
    detail: &'static str,
    insert_text: Option<&'static str>,
}

const KEYWORDS: &[KeywordSpec] = &[
    KeywordSpec {
        label: "abstract",
        detail: "Declare an abstract class member",
        insert_text: None,
    },
    KeywordSpec {
        label: "amends",
        detail: "Amend another module",
        insert_text: Some("amends \"${1:module}\"\n\n${0}"),
    },
    KeywordSpec {
        label: "as",
        detail: "Name an import alias",
        insert_text: None,
    },
    KeywordSpec {
        label: "class",
        detail: "Declare a class",
        insert_text: Some("class ${1:Name} {\n  ${0}\n}"),
    },
    KeywordSpec {
        label: "else",
        detail: "Else branch for if/when",
        insert_text: None,
    },
    KeywordSpec {
        label: "extends",
        detail: "Extend a module or class",
        insert_text: Some("extends \"${1:module}\"\n\n${0}"),
    },
    KeywordSpec {
        label: "external",
        detail: "Declare an external member",
        insert_text: None,
    },
    KeywordSpec {
        label: "false",
        detail: "Boolean false",
        insert_text: None,
    },
    KeywordSpec {
        label: "fixed",
        detail: "Prevent overrides",
        insert_text: None,
    },
    KeywordSpec {
        label: "for",
        detail: "Generate entries from a collection",
        insert_text: Some("for (${1:item} in ${2:items}) {\n  ${0}\n}"),
    },
    KeywordSpec {
        label: "function",
        detail: "Declare a function",
        insert_text: Some("function ${1:name}(${2}) = ${0}"),
    },
    KeywordSpec {
        label: "hidden",
        detail: "Hide a member from output",
        insert_text: None,
    },
    KeywordSpec {
        label: "if",
        detail: "Conditional expression",
        insert_text: Some("if (${1:condition}) ${2:value} else ${0}"),
    },
    KeywordSpec {
        label: "import",
        detail: "Import another module",
        insert_text: Some("import \"${1:module}\" as ${2:name}"),
    },
    KeywordSpec {
        label: "in",
        detail: "For-generator separator",
        insert_text: None,
    },
    KeywordSpec {
        label: "is",
        detail: "Type check expression",
        insert_text: None,
    },
    KeywordSpec {
        label: "let",
        detail: "Local expression binding",
        insert_text: Some("let (${1:name} = ${2:value}) ${0}"),
    },
    KeywordSpec {
        label: "local",
        detail: "Declare a local member",
        insert_text: None,
    },
    KeywordSpec {
        label: "module",
        detail: "Declare module metadata",
        insert_text: Some("module ${1:name}\n\n${0}"),
    },
    KeywordSpec {
        label: "new",
        detail: "Create an object",
        insert_text: Some("new ${1:Type} {\n  ${0}\n}"),
    },
    KeywordSpec {
        label: "null",
        detail: "Null value",
        insert_text: None,
    },
    KeywordSpec {
        label: "open",
        detail: "Allow object members to be extended",
        insert_text: None,
    },
    KeywordSpec {
        label: "out",
        detail: "Mark a type parameter covariant",
        insert_text: None,
    },
    KeywordSpec {
        label: "outer",
        detail: "Reference an outer object scope",
        insert_text: None,
    },
    KeywordSpec {
        label: "read",
        detail: "Read an external resource",
        insert_text: Some("read(\"${1:path}\")"),
    },
    KeywordSpec {
        label: "super",
        detail: "Reference a superclass member",
        insert_text: None,
    },
    KeywordSpec {
        label: "this",
        detail: "Reference the current object",
        insert_text: None,
    },
    KeywordSpec {
        label: "throw",
        detail: "Throw an error",
        insert_text: Some("throw(${1:message})"),
    },
    KeywordSpec {
        label: "trace",
        detail: "Trace a value while evaluating",
        insert_text: Some("trace(${1:value})"),
    },
    KeywordSpec {
        label: "true",
        detail: "Boolean true",
        insert_text: None,
    },
    KeywordSpec {
        label: "typealias",
        detail: "Declare a type alias",
        insert_text: Some("typealias ${1:Name} = ${0}"),
    },
    KeywordSpec {
        label: "unknown",
        detail: "Unknown type marker",
        insert_text: None,
    },
    KeywordSpec {
        label: "when",
        detail: "Conditional object members",
        insert_text: Some("when (${1:condition}) {\n  ${0}\n}"),
    },
];

pub fn complete_at(
    doc: &Document,
    graph: &ModuleGraph,
    workspace_index: &WorkspaceIndex,
    loader_config: &FsLoaderConfig,
    completion_config: &CompletionConfig,
    uri: &Url,
    position: Position,
) -> Option<CompletionResponse> {
    let offset = doc.position_to_offset(position);
    let text = doc.rope.to_string();
    let context = detect_context(&text, offset as usize);

    let items = match context {
        Context::Member { dot_pos } => member_completions(doc, graph, uri, dot_pos as u32),
        Context::ImportPath { quote_start } => import_path_completions(
            doc,
            graph,
            workspace_index,
            loader_config,
            uri,
            quote_start,
            offset as usize,
        ),
        Context::String => string_value_completions(doc, completion_config, offset),
        Context::TopLevel => contextual_body_completions(doc, graph, uri, offset)
            .unwrap_or_else(|| top_level_completions(doc)),
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
    String,
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

    if is_inside_string(text, offset) {
        return Context::String;
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

fn is_inside_string(text: &str, offset: usize) -> bool {
    let offset = offset as u32;
    for token in pkl_syntax::tokenize(text) {
        match token.kind {
            pkl_syntax::SyntaxKind::String | pkl_syntax::SyntaxKind::MultilineString => {
                if token.span.start < offset && offset < token.span.end {
                    return true;
                }
            }
            pkl_syntax::SyntaxKind::StringPart | pkl_syntax::SyntaxKind::MultilineStringPart => {
                if token.span.touches(offset) {
                    return true;
                }
            }
            pkl_syntax::SyntaxKind::StringQuoteOpen => {
                if token.span.end == offset {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

struct StringValueContext {
    field_name: String,
    replace_range: Range,
}

fn string_value_completions(
    doc: &Document,
    completion_config: &CompletionConfig,
    offset: u32,
) -> Vec<CompletionItem> {
    let Some(ctx) = string_value_context(doc, offset) else {
        return Vec::new();
    };
    let Some(values) = completion_config.values.get(&ctx.field_name) else {
        return Vec::new();
    };

    values
        .iter()
        .enumerate()
        .map(|(idx, value)| CompletionItem {
            label: value.label().to_string(),
            kind: Some(CompletionItemKind::VALUE),
            detail: value.detail().map(ToOwned::to_owned),
            documentation: value.documentation(),
            sort_text: Some(format!("0000_value_{idx:04}_{}", value.label())),
            filter_text: Some(value.label().to_string()),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: ctx.replace_range,
                new_text: value.label().to_string(),
            })),
            ..Default::default()
        })
        .collect()
}

fn string_value_context(doc: &Document, offset: u32) -> Option<StringValueContext> {
    for item in doc.module().items() {
        match item {
            Item::Property(p) => {
                let Some(name) = p.name().map(|t| pkl_syntax::cst::ident_text(&t)) else {
                    continue;
                };
                if let Some(ctx) = property_string_value_context(doc, &name, p.value(), offset) {
                    return Some(ctx);
                }
            }
            Item::Method(m) => {
                if let Some(body) = m.body() {
                    if let Some(ctx) = expr_string_value_context(doc, &body, offset) {
                        return Some(ctx);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn property_string_value_context(
    doc: &Document,
    name: &str,
    value: Option<PropertyValue>,
    offset: u32,
) -> Option<StringValueContext> {
    match value {
        Some(PropertyValue::Expr(expr)) => literal_string_replace_range(doc, &expr, offset)
            .map(|replace_range| StringValueContext {
                field_name: name.to_string(),
                replace_range,
            })
            .or_else(|| expr_string_value_context(doc, &expr, offset)),
        Some(PropertyValue::ObjectBody(body)) => {
            object_body_string_value_context(doc, &body, offset)
        }
        None => None,
    }
}

fn object_body_string_value_context(
    doc: &Document,
    body: &cst::ObjectBody,
    offset: u32,
) -> Option<StringValueContext> {
    let span = pkl_syntax::cst::significant_span(body.syntax());
    if !span.contains(offset) {
        return None;
    }
    for member in body.members() {
        match member {
            ObjectMember::Property(p) => {
                let Some(name) = p.name().map(|t| pkl_syntax::cst::ident_text(&t)) else {
                    continue;
                };
                if let Some(ctx) = property_string_value_context(doc, &name, p.value(), offset) {
                    return Some(ctx);
                }
            }
            ObjectMember::Element(e) => {
                if let Some(expr) = e.expr() {
                    if let Some(ctx) = expr_string_value_context(doc, &expr, offset) {
                        return Some(ctx);
                    }
                }
            }
            ObjectMember::Entry(e) => match e.value() {
                Some(PropertyValue::ObjectBody(body)) => {
                    if let Some(ctx) = object_body_string_value_context(doc, &body, offset) {
                        return Some(ctx);
                    }
                }
                Some(PropertyValue::Expr(expr)) => {
                    if let Some(ctx) = expr_string_value_context(doc, &expr, offset) {
                        return Some(ctx);
                    }
                }
                None => {}
            },
            ObjectMember::When(w) => {
                if let Some(body) = w.then_body() {
                    if let Some(ctx) = object_body_string_value_context(doc, &body, offset) {
                        return Some(ctx);
                    }
                }
                if let Some(body) = w.else_body() {
                    if let Some(ctx) = object_body_string_value_context(doc, &body, offset) {
                        return Some(ctx);
                    }
                }
            }
            ObjectMember::For(f) => {
                if let Some(body) = f.body() {
                    if let Some(ctx) = object_body_string_value_context(doc, &body, offset) {
                        return Some(ctx);
                    }
                }
            }
            ObjectMember::Method(m) => {
                if let Some(expr) = m.body() {
                    if let Some(ctx) = expr_string_value_context(doc, &expr, offset) {
                        return Some(ctx);
                    }
                }
            }
            ObjectMember::Spread(s) => {
                if let Some(expr) = s.expr() {
                    if let Some(ctx) = expr_string_value_context(doc, &expr, offset) {
                        return Some(ctx);
                    }
                }
            }
        }
    }
    None
}

fn expr_string_value_context(
    doc: &Document,
    expr: &Expr,
    offset: u32,
) -> Option<StringValueContext> {
    let span = pkl_syntax::cst::significant_span(expr.syntax());
    if !span.contains(offset) {
        return None;
    }
    match expr {
        Expr::New(n) => n
            .body()
            .and_then(|body| object_body_string_value_context(doc, &body, offset)),
        Expr::Amends(a) => a
            .body()
            .and_then(|body| object_body_string_value_context(doc, &body, offset)),
        Expr::Paren(p) => p
            .inner()
            .and_then(|inner| expr_string_value_context(doc, &inner, offset)),
        _ => None,
    }
}

fn literal_string_replace_range(doc: &Document, expr: &Expr, offset: u32) -> Option<Range> {
    let Expr::Literal(lit) = expr else {
        return None;
    };
    if !matches!(
        lit.kind(),
        Some(cst::LiteralKind::String | cst::LiteralKind::MultilineString)
    ) {
        return None;
    }
    let token = lit.token()?;
    let token_span = pkl_syntax::cst::token_span(&token);
    let (content_start, content_end) =
        string_literal_content_offsets(token.text(), token_span.start)?;
    if !(content_start <= offset && offset <= content_end) {
        return None;
    }
    Some(Range {
        start: crate::document::byte_to_position(&doc.rope, content_start as usize),
        end: crate::document::byte_to_position(&doc.rope, content_end as usize),
    })
}

fn string_literal_content_offsets(text: &str, token_start: u32) -> Option<(u32, u32)> {
    let hash_count = text.bytes().take_while(|b| *b == b'#').count();
    let rest = &text[hash_count..];
    let quote_count = if rest.starts_with("\"\"\"") { 3 } else { 1 };
    let open_len = hash_count + quote_count;
    let close_len = quote_count + hash_count;
    if text.len() < open_len + close_len {
        return None;
    }
    Some((
        token_start + open_len as u32,
        token_start + (text.len() - close_len) as u32,
    ))
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

    items.extend(keyword_completions(None));

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

fn keyword_completions(replace_range: Option<Range>) -> Vec<CompletionItem> {
    KEYWORDS
        .iter()
        .enumerate()
        .map(|(idx, kw)| {
            let insert = kw.insert_text.unwrap_or(kw.label).to_string();
            CompletionItem {
                label: kw.label.to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                detail: Some(kw.detail.to_string()),
                insert_text: Some(insert.clone()),
                insert_text_format: kw.insert_text.map(|_| InsertTextFormat::SNIPPET),
                sort_text: Some(format!("9000_keyword_{idx:02}_{}", kw.label)),
                text_edit: replace_range.map(|range| {
                    CompletionTextEdit::Edit(TextEdit {
                        range,
                        new_text: insert.clone(),
                    })
                }),
                ..Default::default()
            }
        })
        .collect()
}

fn contextual_constructor_completion(
    surface: &BodySurface,
    replace_range: Range,
    rank: usize,
) -> CompletionItem {
    let type_name = surface.type_name();
    CompletionItem {
        label: format!("new {}", type_name),
        kind: Some(CompletionItemKind::SNIPPET),
        detail: Some(format!("Create {}", type_name)),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        filter_text: Some(format!("new {}", type_name)),
        sort_text: Some(format!("0000_constructor_{rank:04}_{type_name}")),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: replace_range,
            new_text: format!("new {} {{\n  ${{0}}\n}}", type_name),
        })),
        ..Default::default()
    }
}

// ----------------------------------------------------------------------
// Member completions: receiver type → members.

fn member_completions(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    dot_pos: u32,
) -> Vec<CompletionItem> {
    if let Some(items) = imported_module_member_completions(doc, graph, uri, dot_pos) {
        return items;
    }

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

fn imported_module_member_completions(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    dot_pos: u32,
) -> Option<Vec<CompletionItem>> {
    let receiver_sym = doc
        .analysis
        .resolution
        .references
        .iter()
        .find_map(|reference| {
            if reference.span.end != dot_pos {
                return None;
            }
            let sym = doc.analysis.resolution.symbol(reference.symbol);
            matches!(sym.kind, SymbolKind::Import { .. }).then_some(sym)
        })?;

    let module_uri = crate::uri::url_to_module_uri(uri);
    let imported = graph.imported_module(&module_uri, &receiver_sym.name)?;

    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for sym in imported.analysis.resolution.symbols.iter() {
        if sym.origin.is_stdlib() || sym.container.is_some() {
            continue;
        }
        if !seen.insert(sym.name.clone()) {
            continue;
        }
        items.push(symbol_completion(sym));
    }
    Some(items)
}

#[derive(Clone, Debug)]
enum BodySurface {
    Local(Ty),
    ImportedClass { alias: String, class_name: String },
}

impl BodySurface {
    fn type_name(&self) -> String {
        match self {
            BodySurface::Local(Ty::Named { name, .. }) => name.clone(),
            BodySurface::Local(ty) => ty.to_string(),
            BodySurface::ImportedClass { alias, class_name } => {
                format!("{}.{}", alias, class_name)
            }
        }
    }
}

fn contextual_body_completions(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    offset: u32,
) -> Option<Vec<CompletionItem>> {
    if constructor_prefix_intent(doc, offset) {
        if let Some(items) = constructor_body_completions(doc, graph, uri, offset) {
            return Some(items);
        }
    }

    if let Some(items) = object_body_completions(doc, graph, uri, offset) {
        return Some(items);
    }

    constructor_body_completions(doc, graph, uri, offset)
}

fn constructor_body_completions(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    offset: u32,
) -> Option<Vec<CompletionItem>> {
    let replace_range = current_prefix_replace_range(doc, offset);
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for surface in contextual_constructor_surfaces(doc, graph, uri, offset) {
        let key = surface.type_name();
        if !seen.insert(key) {
            continue;
        }
        items.push(contextual_constructor_completion(
            &surface,
            replace_range,
            items.len(),
        ));
    }

    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

fn object_body_completions(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    offset: u32,
) -> Option<Vec<CompletionItem>> {
    let module = doc.module();
    let mut best: Option<(u32, BodySurface, std::collections::HashSet<String>)> = None;
    for item in module.items() {
        match item {
            Item::Property(p) => {
                let expected = p.ty().as_ref().map(|ty| body_surface_from_type(doc, ty));
                inspect_property_value_for_body(
                    doc,
                    graph,
                    uri,
                    p.value(),
                    expected.as_ref(),
                    offset,
                    &mut best,
                );
            }
            Item::Class(_) => {}
            Item::Method(m) => {
                if let Some(body) = m.body() {
                    inspect_expr_for_body(doc, graph, uri, &body, None, offset, &mut best);
                }
            }
            _ => {}
        }
    }

    let (_, surface, declared_names) = best?;
    let items = body_surface_completions(doc, graph, uri, &surface, &declared_names);
    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

fn body_surface_completions(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    surface: &BodySurface,
    declared_names: &std::collections::HashSet<String>,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    match surface {
        BodySurface::Local(ty) => {
            for (m, owner) in stdlib_members_of(ty) {
                if declared_names.contains(m.name) || !seen.insert(m.name.to_string()) {
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
            for sym_id in user_members_of(&doc.analysis.resolution, ty) {
                let sym = doc.analysis.resolution.symbol(sym_id);
                if declared_names.contains(&sym.name) || !seen.insert(sym.name.clone()) {
                    continue;
                }
                items.push(symbol_completion(sym));
            }
        }
        BodySurface::ImportedClass { alias, class_name } => {
            for item in imported_class_member_completions(graph, uri, alias, class_name) {
                if declared_names.contains(&item.label) || !seen.insert(item.label.clone()) {
                    continue;
                }
                items.push(item);
            }
        }
    }
    items
}

fn inspect_property_value_for_body(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    value: Option<PropertyValue>,
    expected: Option<&BodySurface>,
    offset: u32,
    best: &mut Option<(u32, BodySurface, std::collections::HashSet<String>)>,
) {
    match value {
        Some(PropertyValue::ObjectBody(body)) => {
            inspect_object_body(doc, graph, uri, &body, expected, offset, best)
        }
        Some(PropertyValue::Expr(expr)) => {
            inspect_expr_for_body(doc, graph, uri, &expr, expected, offset, best)
        }
        None => {}
    }
}

fn inspect_object_body(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    body: &cst::ObjectBody,
    expected: Option<&BodySurface>,
    offset: u32,
    best: &mut Option<(u32, BodySurface, std::collections::HashSet<String>)>,
) {
    let span = pkl_syntax::cst::significant_span(body.syntax());
    if !span.contains(offset) {
        return;
    }
    if let Some(surface) = expected {
        if !matches!(surface, BodySurface::Local(Ty::Unknown))
            && best
                .as_ref()
                .map(|(start, _, _)| span.start >= *start)
                .unwrap_or(true)
        {
            *best = Some((
                span.start,
                surface.clone(),
                declared_object_member_names(body),
            ));
        }
    }

    for member in body.members() {
        match member {
            ObjectMember::Property(p) => {
                let declared = p.ty().as_ref().map(|ty| body_surface_from_type(doc, ty));
                let name = p
                    .name()
                    .map(|t| pkl_syntax::cst::ident_text(&t))
                    .unwrap_or_default();
                let member_surface = declared.or_else(|| {
                    expected.and_then(|surface| {
                        expected_member_surface(doc, graph, uri, surface, &name)
                    })
                });
                inspect_property_value_for_body(
                    doc,
                    graph,
                    uri,
                    p.value(),
                    member_surface.as_ref(),
                    offset,
                    best,
                );
            }
            ObjectMember::Method(m) => {
                if let Some(expr) = m.body() {
                    inspect_expr_for_body(doc, graph, uri, &expr, None, offset, best);
                }
            }
            ObjectMember::Element(e) => {
                if let Some(expr) = e.expr() {
                    inspect_expr_for_body(doc, graph, uri, &expr, None, offset, best);
                }
            }
            ObjectMember::Entry(e) => match e.value() {
                Some(PropertyValue::ObjectBody(body)) => {
                    inspect_object_body(doc, graph, uri, &body, None, offset, best)
                }
                Some(PropertyValue::Expr(expr)) => {
                    inspect_expr_for_body(doc, graph, uri, &expr, None, offset, best)
                }
                None => {}
            },
            ObjectMember::When(w) => {
                if let Some(then_body) = w.then_body() {
                    inspect_object_body(doc, graph, uri, &then_body, expected, offset, best);
                }
                if let Some(else_body) = w.else_body() {
                    inspect_object_body(doc, graph, uri, &else_body, expected, offset, best);
                }
            }
            ObjectMember::For(f) => {
                if let Some(body) = f.body() {
                    inspect_object_body(doc, graph, uri, &body, expected, offset, best);
                }
            }
            ObjectMember::Spread(s) => {
                if let Some(expr) = s.expr() {
                    inspect_expr_for_body(doc, graph, uri, &expr, None, offset, best);
                }
            }
        }
    }
}

fn inspect_expr_for_body(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    expr: &Expr,
    expected: Option<&BodySurface>,
    offset: u32,
    best: &mut Option<(u32, BodySurface, std::collections::HashSet<String>)>,
) {
    let span = pkl_syntax::cst::significant_span(expr.syntax());
    if !span.contains(offset) {
        return;
    }
    match expr {
        Expr::New(n) => {
            let new_surface = n
                .ty()
                .as_ref()
                .map(|ty| body_surface_from_type(doc, ty))
                .or_else(|| expected.cloned());
            if let Some(body) = n.body() {
                inspect_object_body(doc, graph, uri, &body, new_surface.as_ref(), offset, best);
            }
        }
        Expr::Amends(a) => {
            if let Some(body) = a.body() {
                inspect_object_body(doc, graph, uri, &body, expected, offset, best);
            }
        }
        Expr::Paren(p) => {
            if let Some(inner) = p.inner() {
                inspect_expr_for_body(doc, graph, uri, &inner, expected, offset, best);
            }
        }
        _ => {}
    }
}

fn declared_object_member_names(body: &cst::ObjectBody) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for member in body.members() {
        match member {
            ObjectMember::Property(p) => {
                if let Some(name) = p.name() {
                    names.insert(pkl_syntax::cst::ident_text(&name));
                }
            }
            ObjectMember::Method(m) => {
                if let Some(name) = m.name() {
                    names.insert(pkl_syntax::cst::ident_text(&name));
                }
            }
            _ => {}
        }
    }
    names
}

fn body_surface_from_type(doc: &Document, ty: &cst::Type) -> BodySurface {
    if let Some((alias, class_name)) = imported_class_from_type(doc, ty) {
        BodySurface::ImportedClass { alias, class_name }
    } else {
        BodySurface::Local(Ty::from_cst_type(ty))
    }
}

fn imported_class_from_type(doc: &Document, ty: &cst::Type) -> Option<(String, String)> {
    let cst::Type::Named(named) = ty else {
        return None;
    };
    let name = named.name()?;
    let segs: Vec<_> = name.segments().collect();
    if segs.len() < 2 {
        return None;
    }
    let head = &segs[0];
    let tail = segs.last()?;
    let head_span = pkl_syntax::cst::token_span(head);
    let sym_id = doc
        .analysis
        .resolution
        .by_span_start
        .get(&head_span.start)?;
    let sym = doc.analysis.resolution.symbol(*sym_id);
    if !matches!(sym.kind, SymbolKind::Import { .. }) {
        return None;
    }
    Some((sym.name.clone(), pkl_syntax::cst::ident_text(tail)))
}

fn expected_member_surface(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    surface: &BodySurface,
    member_name: &str,
) -> Option<BodySurface> {
    match surface {
        BodySurface::Local(ty) => {
            for sym_id in user_members_of(&doc.analysis.resolution, ty) {
                let sym = doc.analysis.resolution.symbol(sym_id);
                if sym.name == member_name {
                    return Some(BodySurface::Local(sym.declared_ty.clone()));
                }
            }
            None
        }
        BodySurface::ImportedClass { alias, class_name } => {
            let module_uri = crate::uri::url_to_module_uri(uri);
            let imported = graph.imported_module(&module_uri, alias)?;
            let class = graph.lookup_top_level(imported, class_name)?;
            let member = imported
                .analysis
                .resolution
                .symbols
                .iter()
                .find(|s| s.container == Some(class.id) && s.name == member_name)?;
            let Ty::Named { name, .. } = &member.declared_ty else {
                return Some(BodySurface::Local(member.declared_ty.clone()));
            };
            Some(BodySurface::ImportedClass {
                alias: alias.clone(),
                class_name: name.clone(),
            })
        }
    }
}

fn imported_class_member_completions(
    graph: &ModuleGraph,
    uri: &Url,
    alias: &str,
    class_name: &str,
) -> Vec<CompletionItem> {
    let module_uri = crate::uri::url_to_module_uri(uri);
    let Some(imported) = graph.imported_module(&module_uri, alias) else {
        return Vec::new();
    };
    let Some(class) = graph.lookup_top_level(imported, class_name) else {
        return Vec::new();
    };
    imported
        .analysis
        .resolution
        .symbols
        .iter()
        .filter(|s| s.container == Some(class.id))
        .map(symbol_completion)
        .collect()
}

fn contextual_constructor_surfaces(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    offset: u32,
) -> Vec<BodySurface> {
    let mut best: Option<(u32, Vec<BodySurface>)> = None;
    for item in doc.module().items() {
        match item {
            Item::Property(p) => {
                let expected = p.ty().as_ref().map(|ty| body_surface_from_type(doc, ty));
                collect_property_value_constructor_surfaces(
                    doc,
                    graph,
                    uri,
                    p.value(),
                    expected.as_ref(),
                    offset,
                    &mut best,
                );
            }
            Item::Method(m) => {
                if let Some(body) = m.body() {
                    collect_expr_constructor_surfaces(
                        doc, graph, uri, &body, None, offset, &mut best,
                    );
                }
            }
            _ => {}
        }
    }
    best.map(|(_, surfaces)| surfaces).unwrap_or_default()
}

fn collect_property_value_constructor_surfaces(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    value: Option<PropertyValue>,
    expected: Option<&BodySurface>,
    offset: u32,
    best: &mut Option<(u32, Vec<BodySurface>)>,
) {
    match value {
        Some(PropertyValue::ObjectBody(body)) => {
            collect_object_body_constructor_surfaces(doc, graph, uri, &body, expected, offset, best)
        }
        Some(PropertyValue::Expr(expr)) => {
            collect_expr_constructor_surfaces(doc, graph, uri, &expr, expected, offset, best)
        }
        None => {}
    }
}

fn collect_object_body_constructor_surfaces(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    body: &cst::ObjectBody,
    expected: Option<&BodySurface>,
    offset: u32,
    best: &mut Option<(u32, Vec<BodySurface>)>,
) {
    let span = pkl_syntax::cst::significant_span(body.syntax());
    if !span.contains(offset) {
        return;
    }

    let mut surfaces = Vec::new();
    let mut prior_elements = Vec::new();
    for member in body.members() {
        if let ObjectMember::Element(e) = &member {
            let element_span = pkl_syntax::cst::significant_span(e.syntax());
            if element_span.end <= offset {
                if let Some(surface) = e
                    .expr()
                    .as_ref()
                    .and_then(|expr| new_expr_surface(doc, expr))
                {
                    prior_elements.push((element_span.end, surface));
                }
            }
        }
    }
    prior_elements.sort_by(|(a, _), (b, _)| b.cmp(a));
    surfaces.extend(prior_elements.into_iter().map(|(_, surface)| surface));
    if let Some(expected) = expected {
        surfaces.extend(element_surfaces_from_expected(expected));
    }
    if constructor_prefix_active(doc, offset) {
        surfaces.extend(imported_class_surfaces(doc, graph, uri));
    }
    if !surfaces.is_empty()
        && best
            .as_ref()
            .map(|(start, _)| span.start >= *start)
            .unwrap_or(true)
    {
        *best = Some((span.start, surfaces));
    }

    for member in body.members() {
        match member {
            ObjectMember::Property(p) => {
                let declared = p.ty().as_ref().map(|ty| body_surface_from_type(doc, ty));
                let name = p
                    .name()
                    .map(|t| pkl_syntax::cst::ident_text(&t))
                    .unwrap_or_default();
                let member_surface = declared.or_else(|| {
                    expected.and_then(|surface| {
                        expected_member_surface(doc, graph, uri, surface, &name)
                    })
                });
                collect_property_value_constructor_surfaces(
                    doc,
                    graph,
                    uri,
                    p.value(),
                    member_surface.as_ref(),
                    offset,
                    best,
                );
            }
            ObjectMember::Element(e) => {
                if let Some(expr) = e.expr() {
                    collect_expr_constructor_surfaces(doc, graph, uri, &expr, None, offset, best);
                }
            }
            ObjectMember::Entry(e) => match e.value() {
                Some(PropertyValue::ObjectBody(body)) => collect_object_body_constructor_surfaces(
                    doc, graph, uri, &body, None, offset, best,
                ),
                Some(PropertyValue::Expr(expr)) => {
                    collect_expr_constructor_surfaces(doc, graph, uri, &expr, None, offset, best)
                }
                None => {}
            },
            ObjectMember::When(w) => {
                if let Some(then_body) = w.then_body() {
                    collect_object_body_constructor_surfaces(
                        doc, graph, uri, &then_body, expected, offset, best,
                    );
                }
                if let Some(else_body) = w.else_body() {
                    collect_object_body_constructor_surfaces(
                        doc, graph, uri, &else_body, expected, offset, best,
                    );
                }
            }
            ObjectMember::For(f) => {
                if let Some(body) = f.body() {
                    collect_object_body_constructor_surfaces(
                        doc, graph, uri, &body, expected, offset, best,
                    );
                }
            }
            ObjectMember::Method(m) => {
                if let Some(expr) = m.body() {
                    collect_expr_constructor_surfaces(doc, graph, uri, &expr, None, offset, best);
                }
            }
            ObjectMember::Spread(s) => {
                if let Some(expr) = s.expr() {
                    collect_expr_constructor_surfaces(doc, graph, uri, &expr, None, offset, best);
                }
            }
        }
    }
}

fn collect_expr_constructor_surfaces(
    doc: &Document,
    graph: &ModuleGraph,
    uri: &Url,
    expr: &Expr,
    expected: Option<&BodySurface>,
    offset: u32,
    best: &mut Option<(u32, Vec<BodySurface>)>,
) {
    let span = pkl_syntax::cst::significant_span(expr.syntax());
    if !span.contains(offset) {
        return;
    }
    match expr {
        Expr::New(n) => {
            let new_surface = n
                .ty()
                .as_ref()
                .map(|ty| body_surface_from_type(doc, ty))
                .or_else(|| expected.cloned());
            if let Some(body) = n.body() {
                collect_object_body_constructor_surfaces(
                    doc,
                    graph,
                    uri,
                    &body,
                    new_surface.as_ref(),
                    offset,
                    best,
                );
            }
        }
        Expr::Amends(a) => {
            if let Some(body) = a.body() {
                collect_object_body_constructor_surfaces(
                    doc, graph, uri, &body, expected, offset, best,
                );
            }
        }
        Expr::Paren(p) => {
            if let Some(inner) = p.inner() {
                collect_expr_constructor_surfaces(doc, graph, uri, &inner, expected, offset, best);
            }
        }
        _ => {}
    }
}

fn new_expr_surface(doc: &Document, expr: &Expr) -> Option<BodySurface> {
    let Expr::New(n) = expr else {
        return None;
    };
    n.ty().as_ref().map(|ty| body_surface_from_type(doc, ty))
}

fn element_surfaces_from_expected(surface: &BodySurface) -> Vec<BodySurface> {
    match surface {
        BodySurface::Local(Ty::List(inner))
        | BodySurface::Local(Ty::Listing(inner))
        | BodySurface::Local(Ty::Set(inner)) => vec![BodySurface::Local((**inner).clone())],
        _ => Vec::new(),
    }
}

fn imported_class_surfaces(doc: &Document, graph: &ModuleGraph, uri: &Url) -> Vec<BodySurface> {
    let module_uri = crate::uri::url_to_module_uri(uri);
    let mut surfaces = Vec::new();
    for import in doc.analysis.resolution.imports.values() {
        if import.is_glob {
            continue;
        }
        let Some(imported) = graph.imported_module(&module_uri, &import.local_name) else {
            continue;
        };
        for sym in imported.analysis.resolution.symbols.iter() {
            if sym.origin.is_stdlib()
                || sym.container.is_some()
                || !matches!(sym.kind, SymbolKind::Class)
            {
                continue;
            }
            surfaces.push(BodySurface::ImportedClass {
                alias: import.local_name.clone(),
                class_name: sym.name.clone(),
            });
        }
    }
    surfaces
}

fn constructor_prefix_active(doc: &Document, offset: u32) -> bool {
    let text = doc.rope.to_string();
    let prefix = current_prefix_text(&text, offset as usize);
    prefix.is_empty() || "new".starts_with(prefix)
}

fn constructor_prefix_intent(doc: &Document, offset: u32) -> bool {
    let text = doc.rope.to_string();
    let prefix = current_prefix_text(&text, offset as usize);
    !prefix.is_empty() && "new".starts_with(prefix)
}

fn current_prefix_text(text: &str, offset: usize) -> &str {
    let bytes = text.as_bytes();
    let mut start = offset;
    while start > 0 {
        let b = bytes[start - 1];
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' {
            start -= 1;
        } else {
            break;
        }
    }
    &text[start..offset]
}

fn current_prefix_replace_range(doc: &Document, offset: u32) -> Range {
    let text = doc.rope.to_string();
    let prefix = current_prefix_text(&text, offset as usize);
    let start = offset as usize - prefix.len();
    Range {
        start: crate::document::byte_to_position(&doc.rope, start),
        end: crate::document::byte_to_position(&doc.rope, offset as usize),
    }
}

// ----------------------------------------------------------------------
// Import-path completions: `pkl:` stdlib modules and workspace files.

fn import_path_completions(
    doc: &Document,
    _graph: &ModuleGraph,
    workspace_index: &WorkspaceIndex,
    loader_config: &FsLoaderConfig,
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

    let range_start_pos = crate::document::byte_to_position(&doc.rope, in_quotes_start);
    let range_end_pos = crate::document::byte_to_position(&doc.rope, cursor);
    let replace_range = Range {
        start: range_start_pos,
        end: range_end_pos,
    };

    if prefix.starts_with("pkl:") {
        return stdlib_import_completions(Some(replace_range));
    }

    if let Some((namespace, rest)) = namespace_prefix(prefix) {
        if let Some(root) = loader_config.namespaces.get(namespace) {
            return filesystem_import_completions(root, rest, replace_range)
                .into_iter()
                .map(|item| {
                    let relative_insert = item.detail.clone().unwrap_or_else(|| item.label.clone());
                    let qualified_insert = item
                        .detail
                        .clone()
                        .map(|detail| format!("{}:{}", namespace, detail))
                        .unwrap_or_else(|| item.label.clone());
                    CompletionItem {
                        label: item.label.clone(),
                        filter_text: Some(relative_insert.clone()),
                        insert_text: Some(relative_insert),
                        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                            range: replace_range,
                            new_text: qualified_insert,
                        })),
                        ..item
                    }
                })
                .collect();
        }
    }

    if let Some(rest) = prefix.strip_prefix("modulepath:") {
        return modulepath_completions(loader_config, rest, replace_range);
    }

    if prefix.starts_with("package:") {
        return package_completions(loader_config, prefix, replace_range);
    }

    // (3) Anything else: workspace files. Resolve the current document's
    // filesystem path through the URI; non-file schemes have nothing to
    // offer.
    let Ok(current_path) = uri.to_file_path() else {
        // Still surface stdlib options as a fallback — the user might
        // be typing `pkl:` next.
        return stdlib_import_completions(Some(replace_range));
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
    for (i, item) in stdlib_import_completions(Some(replace_range))
        .into_iter()
        .enumerate()
    {
        items.push(CompletionItem {
            sort_text: Some(format!("z{:05}", stdlib_offset + i)),
            ..item
        });
    }
    for (i, ns) in loader_config.namespaces.keys().enumerate() {
        let insert = format!("{}:", ns);
        if !insert.starts_with(prefix) {
            continue;
        }
        items.push(CompletionItem {
            label: insert.clone(),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some("configured namespace".to_string()),
            sort_text: Some(format!("y{:05}", i)),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: replace_range,
                new_text: insert,
            })),
            ..Default::default()
        });
    }

    items
}

fn namespace_prefix(prefix: &str) -> Option<(&str, &str)> {
    let idx = prefix.find(':')?;
    let namespace = &prefix[..idx];
    if namespace.is_empty()
        || !namespace
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some((namespace, &prefix[idx + 1..]))
}

fn modulepath_completions(
    loader_config: &FsLoaderConfig,
    rest: &str,
    replace_range: Range,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in &loader_config.module_paths {
        for item in filesystem_import_completions(root, rest, replace_range) {
            let Some(detail) = item.detail.clone() else {
                continue;
            };
            if !seen.insert(detail.clone()) {
                continue;
            }
            items.push(CompletionItem {
                label: format!("modulepath:{}", item.label),
                filter_text: Some(format!("modulepath:{}", detail)),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: replace_range,
                    new_text: format!("modulepath:{}", detail),
                })),
                ..item
            });
        }
    }
    items
}

fn package_completions(
    loader_config: &FsLoaderConfig,
    prefix: &str,
    replace_range: Range,
) -> Vec<CompletionItem> {
    let Some(root) = loader_config.package_cache.as_ref() else {
        return Vec::new();
    };
    let rest = prefix.strip_prefix("package:").unwrap_or(prefix);
    filesystem_import_completions(root, rest, replace_range)
        .into_iter()
        .map(|item| {
            let insert = item
                .detail
                .clone()
                .map(|detail| format!("package:{}", detail))
                .unwrap_or_else(|| item.label.clone());
            CompletionItem {
                label: insert.clone(),
                filter_text: Some(insert.clone()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: replace_range,
                    new_text: insert,
                })),
                ..item
            }
        })
        .collect()
}

fn filesystem_import_completions(
    root: &Path,
    rest: &str,
    replace_range: Range,
) -> Vec<CompletionItem> {
    let mut base = PathBuf::from(root);
    let mut typed_dir = "";
    let mut filter = rest;
    if let Some((dir, leaf)) = rest.rsplit_once('/') {
        typed_dir = dir;
        filter = leaf;
        if !dir.is_empty() {
            base.push(dir);
        }
    }

    let Ok(read_dir) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = read_dir.filter_map(Result::ok).collect();
    entries.sort_by_key(|entry| entry.file_name());

    entries
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            if !file_name.starts_with(filter) {
                return None;
            }
            let is_dir = path.is_dir();
            if !is_dir && path.extension().and_then(|e| e.to_str()) != Some("pkl") {
                return None;
            }
            let display_name = if is_dir {
                format!("{}/", file_name)
            } else {
                file_name
                    .strip_suffix(".pkl")
                    .unwrap_or(&file_name)
                    .to_string()
            };
            let insert = if typed_dir.is_empty() {
                display_name.clone()
            } else {
                format!("{}/{}", typed_dir, display_name)
            };
            Some(CompletionItem {
                label: insert.clone(),
                kind: Some(if is_dir {
                    CompletionItemKind::FOLDER
                } else {
                    CompletionItemKind::FILE
                }),
                detail: Some(insert.clone()),
                filter_text: Some(insert.clone()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: replace_range,
                    new_text: insert,
                })),
                ..Default::default()
            })
        })
        .collect()
}

fn stdlib_import_completions(replace_range: Option<Range>) -> Vec<CompletionItem> {
    pkl_stdlib::vendored::MODULES
        .iter()
        .map(|m| CompletionItem {
            label: format!("pkl:{}", m.name),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some(m.module.to_string()),
            filter_text: Some(format!("pkl:{}", m.name)),
            text_edit: replace_range.map(|range| {
                CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: format!("pkl:{}", m.name),
                })
            }),
            ..Default::default()
        })
        .collect()
}

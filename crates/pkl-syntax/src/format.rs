//! Pretty-print a Pkl module back to source text.
//!
//! The formatter walks the typed CST view (`crate::cst`) over the
//! lossless syntax tree and emits canonical Pkl source. Layout rules:
//!
//! * Two-space indentation, LF line endings.
//! * One blank line between top-level items.
//! * Object bodies and class bodies expand to multi-line form whenever
//!   they contain more than one member; otherwise they stay inline.
//! * All comments — `///` doc, `//` line, and `/* ... */` block —
//!   that precede an item, class member, or object member are
//!   preserved verbatim and emitted on their own lines at the
//!   declaration's indentation.

use crate::cst::{
    self, AmendsExpr, Annotation, ArgList, AstNode, BinaryExpr, BinaryOp, CallExpr, ClassBody,
    ClassDecl, ClassMember, ClassMethodDecl, ClassPropertyDecl, ClauseKind, Expr, ForGenerator,
    FunctionType, IdentExpr, IfExpr, ImportClause, IndexExpr, LambdaExpr, LetExpr, LiteralExpr,
    LiteralKind, MemberExpr, MethodDecl, Modifier, ModifierKind, Module, ModuleHeader, NamedType,
    NewExpr, NonNullExpr, NullCoalesceExpr, ObjectBody, ObjectElement, ObjectEntryComputed,
    ObjectMember, ObjectMethod, ObjectProperty, ObjectSpread, Parameter, ParenExpr,
    ParenthesizedType, PropertyDecl, PropertyValue, QualifiedName, ReadExpr, ReadKind,
    SpecialIdentKind, ThrowExpr, TraceExpr, Type, TypeCastExpr, TypeCheckExpr, TypeParameter,
    UnaryExpr, UnaryOp, UnionType, Variance, WhenGenerator,
};
use crate::kind::SyntaxKind;
use crate::syntax::{SyntaxNode, SyntaxToken};

/// Format an entire module. Always ends with a single trailing newline.
pub fn format_module(module: &Module) -> String {
    let mut out = String::new();
    let ctx = FmtCtx { indent: 0 };

    if let Some(header) = module.header() {
        write_module_header(&mut out, &header);
        out.push('\n');
    }

    let imports: Vec<ImportClause> = module.imports().collect();
    if !imports.is_empty() {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        for import in &imports {
            write_import(&mut out, import);
            out.push('\n');
        }
    }

    let items: Vec<cst::Item> = module.items().collect();
    let mut first_item = true;
    for item in &items {
        let pre = collect_leading_comments(item.syntax());
        if !pre.is_empty() {
            if !out.is_empty() && !out.ends_with("\n\n") {
                out.push('\n');
            }
            for c in &pre {
                out.push_str(c);
                out.push('\n');
            }
        } else if !out.is_empty() && (!first_item || !out.ends_with("\n\n")) {
            out.push('\n');
        }
        write_item(&mut out, item, ctx);
        first_item = false;
    }

    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[derive(Clone, Copy)]
struct FmtCtx {
    indent: usize,
}

impl FmtCtx {
    fn child(self) -> Self {
        Self {
            indent: self.indent + 1,
        }
    }
    fn pad(self) -> String {
        "  ".repeat(self.indent)
    }
}

// ----------------------------------------------------------------------
// Comment helpers

/// Collect the non-doc comment trivia that immediately precede `node`
/// as direct leading-trivia children. Doc comments are skipped because
/// they are re-emitted explicitly by `write_doc`. Returned strings are
/// the raw token texts (e.g. `// foo` or `/* foo */`), without any
/// surrounding whitespace.
fn collect_leading_comments(node: &SyntaxNode) -> Vec<String> {
    let mut out = Vec::new();
    for el in node.children_with_tokens() {
        match el.into_token() {
            Some(t) if t.kind().is_trivia() => match t.kind() {
                SyntaxKind::LineComment | SyntaxKind::BlockComment => {
                    out.push(t.text().to_owned());
                }
                _ => {}
            },
            _ => break,
        }
    }
    out
}

/// Pull the doc-comment text out of `node`'s leading trivia, falling
/// back to walking the syntax tree if the typed accessor has not been
/// provided by the caller. Mirrors `cst::doc_comment_for` but lives
/// here so the formatter does not need to handle the `Option<String>`
/// indirection at every site.
fn doc_for(node: &SyntaxNode) -> Option<String> {
    cst::doc_comment_for(node)
}

// ----------------------------------------------------------------------
// Module header

fn write_module_header(out: &mut String, header: &ModuleHeader) {
    if let Some(doc) = header.doc_comment() {
        for line in doc.lines() {
            out.push_str("/// ");
            out.push_str(line);
            out.push('\n');
        }
    }
    for ann in header.annotations() {
        write_annotation(out, &ann, FmtCtx { indent: 0 });
        out.push('\n');
    }
    let mut prefix = String::new();
    let modifiers: Vec<Modifier> = header.modifiers().collect();
    if !modifiers.is_empty() {
        prefix.push_str(&format_modifiers_list(&modifiers));
        prefix.push(' ');
    }
    if let Some(name) = header.name() {
        out.push_str(&prefix);
        out.push_str("module ");
        write_qualified_name(out, &name);
        if let Some(clause) = header.clause() {
            out.push('\n');
            out.push_str(match clause.kind() {
                ClauseKind::Amends => "amends ",
                ClauseKind::Extends => "extends ",
            });
            if let Some(target) = clause.target() {
                out.push_str(target.text());
            }
        }
    } else if let Some(clause) = header.clause() {
        out.push_str(&prefix);
        out.push_str(match clause.kind() {
            ClauseKind::Amends => "amends ",
            ClauseKind::Extends => "extends ",
        });
        if let Some(target) = clause.target() {
            out.push_str(target.text());
        }
    }
}

fn write_import(out: &mut String, import: &ImportClause) {
    out.push_str(if import.is_glob() {
        "import* "
    } else {
        "import "
    });
    if let Some(path) = import.path() {
        out.push_str(path.text());
    }
    if let Some(alias) = import.alias() {
        out.push_str(" as ");
        out.push_str(&cst::ident_text(&alias));
    }
}

// ----------------------------------------------------------------------
// Top-level items

fn write_item(out: &mut String, item: &cst::Item, ctx: FmtCtx) {
    match item {
        cst::Item::Class(c) => write_class(out, c, ctx),
        cst::Item::TypeAlias(t) => write_typealias(out, t, ctx),
        cst::Item::Property(p) => write_property(out, p, ctx),
        cst::Item::Method(m) => write_method(out, m, ctx),
        cst::Item::Error(_) => {}
    }
}

fn write_class(out: &mut String, c: &ClassDecl, ctx: FmtCtx) {
    write_doc(out, doc_for(c.syntax()).as_deref(), ctx);
    for a in c.annotations() {
        out.push_str(&ctx.pad());
        write_annotation(out, &a, ctx);
        out.push('\n');
    }
    let pad = ctx.pad();
    out.push_str(&pad);
    let modifiers: Vec<Modifier> = c.modifiers().collect();
    if !modifiers.is_empty() {
        out.push_str(&format_modifiers_list(&modifiers));
        out.push(' ');
    }
    out.push_str("class ");
    if let Some(name) = c.name() {
        out.push_str(&cst::ident_text(&name));
    }
    if let Some(tps) = c.type_parameters() {
        let params: Vec<TypeParameter> = tps.parameters().collect();
        if !params.is_empty() {
            out.push_str(&format_type_parameters(&params));
        }
    }
    if let Some(ext) = c.extends() {
        out.push_str(" extends ");
        out.push_str(&format_type(&ext));
    }
    if let Some(body) = c.body() {
        write_class_body(out, &body, ctx);
    }
}

fn write_class_body(out: &mut String, body: &ClassBody, ctx: FmtCtx) {
    let members: Vec<ClassMember> = body.members().collect();
    if members.is_empty() {
        out.push_str(" {}");
        return;
    }
    out.push_str(" {\n");
    let child = ctx.child();
    for member in &members {
        // Emit any non-doc comments that lead the member.
        let pre = collect_leading_comments(member.syntax());
        for c in &pre {
            out.push_str(&child.pad());
            out.push_str(c);
            out.push('\n');
        }
        out.push_str(&child.pad());
        match member {
            ClassMember::Property(p) => write_class_property(out, p, child),
            ClassMember::Method(m) => write_class_method(out, m, child),
        }
        out.push('\n');
    }
    out.push_str(&ctx.pad());
    out.push('}');
}

fn write_typealias(out: &mut String, t: &cst::TypeAliasDecl, ctx: FmtCtx) {
    write_doc(out, doc_for(t.syntax()).as_deref(), ctx);
    out.push_str(&ctx.pad());
    let modifiers: Vec<Modifier> = t.modifiers().collect();
    if !modifiers.is_empty() {
        out.push_str(&format_modifiers_list(&modifiers));
        out.push(' ');
    }
    out.push_str("typealias ");
    if let Some(name) = t.name() {
        out.push_str(&cst::ident_text(&name));
    }
    if let Some(tps) = t.type_parameters() {
        let params: Vec<TypeParameter> = tps.parameters().collect();
        if !params.is_empty() {
            out.push_str(&format_type_parameters(&params));
        }
    }
    if let Some(aliased) = t.aliased_type() {
        out.push_str(" = ");
        out.push_str(&format_type(&aliased));
    }
}

fn write_property(out: &mut String, p: &PropertyDecl, ctx: FmtCtx) {
    write_doc(out, doc_for(p.syntax()).as_deref(), ctx);
    out.push_str(&ctx.pad());
    write_property_inline(
        out,
        p.annotations().collect(),
        p.modifiers().collect(),
        p.name().as_ref(),
        p.ty().as_ref(),
        p.value().as_ref(),
        ctx,
    );
}

fn write_class_property(out: &mut String, p: &ClassPropertyDecl, ctx: FmtCtx) {
    write_property_inline(
        out,
        p.annotations().collect(),
        p.modifiers().collect(),
        p.name().as_ref(),
        p.ty().as_ref(),
        p.value().as_ref(),
        ctx,
    );
}

fn write_object_property(out: &mut String, p: &ObjectProperty, ctx: FmtCtx) {
    write_property_inline(
        out,
        p.annotations().collect(),
        p.modifiers().collect(),
        p.name().as_ref(),
        p.ty().as_ref(),
        p.value().as_ref(),
        ctx,
    );
}

#[allow(clippy::too_many_arguments)]
fn write_property_inline(
    out: &mut String,
    annotations: Vec<Annotation>,
    modifiers: Vec<Modifier>,
    name: Option<&SyntaxToken>,
    ty: Option<&Type>,
    value: Option<&PropertyValue>,
    ctx: FmtCtx,
) {
    for a in &annotations {
        write_annotation(out, a, ctx);
        out.push(' ');
    }
    if !modifiers.is_empty() {
        out.push_str(&format_modifiers_list(&modifiers));
        out.push(' ');
    }
    if let Some(n) = name {
        out.push_str(&cst::ident_text(n));
    }
    if let Some(t) = ty {
        out.push_str(": ");
        out.push_str(&format_type(t));
    }
    match value {
        Some(PropertyValue::Expr(e)) => {
            out.push_str(" = ");
            write_expr(out, e, ctx);
        }
        Some(PropertyValue::ObjectBody(body)) => {
            out.push(' ');
            write_object_body(out, body, ctx);
        }
        None => {}
    }
}

fn write_method(out: &mut String, m: &MethodDecl, ctx: FmtCtx) {
    write_doc(out, doc_for(m.syntax()).as_deref(), ctx);
    out.push_str(&ctx.pad());
    write_method_inline(
        out,
        m.annotations().collect(),
        m.modifiers().collect(),
        m.name().as_ref(),
        m.type_parameters(),
        m.parameters(),
        m.return_type().as_ref(),
        m.body().as_ref(),
        ctx,
    );
}

fn write_class_method(out: &mut String, m: &ClassMethodDecl, ctx: FmtCtx) {
    write_method_inline(
        out,
        m.annotations().collect(),
        m.modifiers().collect(),
        m.name().as_ref(),
        m.type_parameters(),
        m.parameters(),
        m.return_type().as_ref(),
        m.body().as_ref(),
        ctx,
    );
}

fn write_object_method(out: &mut String, m: &ObjectMethod, ctx: FmtCtx) {
    write_method_inline(
        out,
        m.annotations().collect(),
        m.modifiers().collect(),
        m.name().as_ref(),
        m.type_parameters(),
        m.parameters(),
        m.return_type().as_ref(),
        m.body().as_ref(),
        ctx,
    );
}

#[allow(clippy::too_many_arguments)]
fn write_method_inline(
    out: &mut String,
    annotations: Vec<Annotation>,
    modifiers: Vec<Modifier>,
    name: Option<&SyntaxToken>,
    type_parameters: Option<cst::TypeParameterList>,
    parameters: Option<cst::ParameterList>,
    return_type: Option<&Type>,
    body: Option<&Expr>,
    ctx: FmtCtx,
) {
    for a in &annotations {
        write_annotation(out, a, ctx);
        out.push(' ');
    }
    if !modifiers.is_empty() {
        out.push_str(&format_modifiers_list(&modifiers));
        out.push(' ');
    }
    out.push_str("function ");
    if let Some(n) = name {
        out.push_str(&cst::ident_text(n));
    }
    if let Some(tps) = type_parameters {
        let params: Vec<TypeParameter> = tps.parameters().collect();
        if !params.is_empty() {
            out.push_str(&format_type_parameters(&params));
        }
    }
    out.push('(');
    let params: Vec<Parameter> = parameters
        .as_ref()
        .map(|p| p.parameters().collect())
        .unwrap_or_default();
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        if let Some(name) = p.name() {
            out.push_str(&cst::ident_text(&name));
        }
        if let Some(ty) = p.ty() {
            out.push_str(": ");
            out.push_str(&format_type(&ty));
        }
    }
    out.push(')');
    if let Some(ret) = return_type {
        out.push_str(": ");
        out.push_str(&format_type(ret));
    }
    if let Some(b) = body {
        out.push_str(" = ");
        write_expr(out, b, ctx);
    }
}

fn write_annotation(out: &mut String, a: &Annotation, ctx: FmtCtx) {
    out.push('@');
    if let Some(name) = a.name() {
        write_qualified_name(out, &name);
    }
    if let Some(body) = a.body() {
        out.push(' ');
        write_object_body(out, &body, ctx);
    }
}

fn write_qualified_name(out: &mut String, name: &QualifiedName) {
    out.push_str(&name.text_joined());
}

fn write_doc(out: &mut String, doc: Option<&str>, ctx: FmtCtx) {
    let Some(doc) = doc else {
        return;
    };
    let pad = ctx.pad();
    for line in doc.lines() {
        out.push_str(&pad);
        out.push_str("/// ");
        out.push_str(line);
        out.push('\n');
    }
}

// ----------------------------------------------------------------------
// Object bodies

fn write_object_body(out: &mut String, body: &ObjectBody, ctx: FmtCtx) {
    let params: Vec<Parameter> = body
        .parameters()
        .map(|p| p.parameters().collect())
        .unwrap_or_default();
    let members: Vec<ObjectMember> = body.members().collect();

    if members.is_empty() && params.is_empty() {
        out.push_str("{}");
        return;
    }
    if members.len() == 1
        && params.is_empty()
        && collect_leading_comments(members[0].syntax()).is_empty()
    {
        out.push_str("{ ");
        write_object_member(out, &members[0], ctx);
        out.push_str(" }");
        return;
    }
    out.push_str("{\n");
    let child = ctx.child();
    if !params.is_empty() {
        out.push_str(&child.pad());
        for (i, p) in params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            if let Some(name) = p.name() {
                out.push_str(&cst::ident_text(&name));
            }
            if let Some(ty) = p.ty() {
                out.push_str(": ");
                out.push_str(&format_type(&ty));
            }
        }
        out.push_str(" ->\n");
    }
    for member in &members {
        for c in collect_leading_comments(member.syntax()) {
            out.push_str(&child.pad());
            out.push_str(&c);
            out.push('\n');
        }
        out.push_str(&child.pad());
        write_object_member(out, member, child);
        out.push('\n');
    }
    out.push_str(&ctx.pad());
    out.push('}');
}

fn write_object_member(out: &mut String, member: &ObjectMember, ctx: FmtCtx) {
    match member {
        ObjectMember::Property(p) => write_object_property(out, p, ctx),
        ObjectMember::Method(m) => write_object_method(out, m, ctx),
        ObjectMember::Element(e) => {
            if let Some(expr) = ObjectElement::expr(e) {
                write_expr(out, &expr, ctx);
            }
        }
        ObjectMember::Entry(e) => write_object_entry(out, e, ctx),
        ObjectMember::When(w) => write_when(out, w, ctx),
        ObjectMember::For(f) => write_for(out, f, ctx),
        ObjectMember::Spread(s) => write_spread(out, s, ctx),
    }
}

fn write_object_entry(out: &mut String, e: &ObjectEntryComputed, ctx: FmtCtx) {
    out.push('[');
    if let Some(key) = e.key() {
        write_expr(out, &key, ctx);
    }
    out.push_str("] ");
    match e.value() {
        Some(PropertyValue::Expr(expr)) => {
            out.push_str("= ");
            write_expr(out, &expr, ctx);
        }
        Some(PropertyValue::ObjectBody(body)) => write_object_body(out, &body, ctx),
        None => {}
    }
}

fn write_when(out: &mut String, w: &WhenGenerator, ctx: FmtCtx) {
    out.push_str("when (");
    if let Some(cond) = w.condition() {
        write_expr(out, &cond, ctx);
    }
    out.push_str(") ");
    if let Some(then_body) = w.then_body() {
        write_object_body(out, &then_body, ctx);
    }
    if let Some(else_body) = w.else_body() {
        out.push_str(" else ");
        write_object_body(out, &else_body, ctx);
    }
}

fn write_for(out: &mut String, f: &ForGenerator, ctx: FmtCtx) {
    out.push_str("for (");
    let bindings: Vec<Parameter> = f.bindings().collect();
    for (i, b) in bindings.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        if let Some(name) = b.name() {
            out.push_str(&cst::ident_text(&name));
        }
    }
    out.push_str(" in ");
    if let Some(it) = f.iterable() {
        write_expr(out, &it, ctx);
    }
    out.push_str(") ");
    if let Some(body) = f.body() {
        write_object_body(out, &body, ctx);
    }
}

fn write_spread(out: &mut String, s: &ObjectSpread, ctx: FmtCtx) {
    out.push_str("...");
    if let Some(e) = s.expr() {
        write_expr(out, &e, ctx);
    }
}

// ----------------------------------------------------------------------
// Expressions

fn write_expr(out: &mut String, expr: &Expr, ctx: FmtCtx) {
    match expr {
        Expr::Literal(lit) => write_literal(out, lit),
        Expr::InterpolatedString(s) => write_interpolated_string(out, s, ctx),
        Expr::Ident(id) => write_ident_expr(out, id),
        Expr::Paren(p) => write_paren(out, p, ctx),
        Expr::Unary(u) => write_unary(out, u, ctx),
        Expr::Binary(b) => write_binary(out, b, ctx),
        Expr::NullCoalesce(n) => write_null_coalesce(out, n, ctx),
        Expr::TypeCheck(tc) => write_type_check(out, tc, ctx),
        Expr::TypeCast(tc) => write_type_cast(out, tc, ctx),
        Expr::If(i) => write_if(out, i, ctx),
        Expr::Let(l) => write_let(out, l, ctx),
        Expr::Lambda(l) => write_lambda(out, l, ctx),
        Expr::Call(c) => write_call(out, c, ctx),
        Expr::Index(i) => write_index(out, i, ctx),
        Expr::Member(m) => write_member(out, m, ctx),
        Expr::NonNull(n) => write_non_null(out, n, ctx),
        Expr::New(n) => write_new(out, n, ctx),
        Expr::Amends(a) => write_amends(out, a, ctx),
        Expr::Throw(t) => write_throw(out, t, ctx),
        Expr::Trace(t) => write_trace(out, t, ctx),
        Expr::Read(r) => write_read(out, r, ctx),
        Expr::Error(_) => out.push_str("<error>"),
    }
}

fn write_interpolated_string(out: &mut String, s: &cst::InterpolatedString, ctx: FmtCtx) {
    use rowan::NodeOrToken;

    // Emit the opening quote.
    if let Some(open) = s.open_quote() {
        out.push_str(open.text());
    }
    // Walk children in source order, emitting parts verbatim and recursing
    // into each interpolation hole so the inner expression gets canonical
    // formatting (with the surrounding `\(` ... `)` markers preserved as
    // raw text from the source).
    for el in s.syntax().children_with_tokens() {
        match el {
            NodeOrToken::Token(t) => match t.kind() {
                SyntaxKind::StringPart | SyntaxKind::MultilineStringPart => {
                    out.push_str(t.text());
                }
                // Open/close quotes are emitted explicitly.
                _ => {}
            },
            NodeOrToken::Node(n) => {
                if let Some(hole) = cst::Interpolation::cast(n) {
                    if let Some(open) = hole.open_marker() {
                        out.push_str(open.text());
                    }
                    if let Some(inner) = hole.expr() {
                        write_expr(out, &inner, ctx);
                    }
                    if let Some(close) = hole.close_marker() {
                        out.push_str(close.text());
                    }
                }
            }
        }
    }
    if let Some(close) = s.close_quote() {
        out.push_str(close.text());
    }
}

fn write_literal(out: &mut String, lit: &LiteralExpr) {
    let Some(t) = lit.token() else {
        return;
    };
    match lit.kind() {
        Some(LiteralKind::Bool) => out.push_str(match lit.bool_value() {
            Some(true) => "true",
            Some(false) => "false",
            None => "false",
        }),
        Some(LiteralKind::Null) => out.push_str("null"),
        _ => out.push_str(t.text()),
    }
}

fn write_ident_expr(out: &mut String, id: &IdentExpr) {
    if let Some(s) = id.special() {
        out.push_str(match s {
            SpecialIdentKind::This => "this",
            SpecialIdentKind::Super => "super",
            SpecialIdentKind::Outer => "outer",
            SpecialIdentKind::Module => "module",
        });
    } else if let Some(t) = id.token() {
        out.push_str(&cst::ident_text(&t));
    }
}

fn write_paren(out: &mut String, p: &ParenExpr, ctx: FmtCtx) {
    out.push('(');
    if let Some(inner) = p.inner() {
        write_expr(out, &inner, ctx);
    }
    out.push(')');
}

fn write_unary(out: &mut String, u: &UnaryExpr, ctx: FmtCtx) {
    out.push_str(match u.op() {
        Some(UnaryOp::Neg) => "-",
        Some(UnaryOp::Not) => "!",
        None => "",
    });
    if let Some(operand) = u.operand() {
        write_expr(out, &operand, ctx);
    }
}

fn write_binary(out: &mut String, b: &BinaryExpr, ctx: FmtCtx) {
    if let Some(lhs) = b.lhs() {
        write_expr(out, &lhs, ctx);
    }
    out.push(' ');
    out.push_str(match b.op() {
        Some(BinaryOp::Add) => "+",
        Some(BinaryOp::Sub) => "-",
        Some(BinaryOp::Mul) => "*",
        Some(BinaryOp::Div) => "/",
        Some(BinaryOp::Rem) => "%",
        Some(BinaryOp::Pow) => "**",
        Some(BinaryOp::Eq) => "==",
        Some(BinaryOp::NotEq) => "!=",
        Some(BinaryOp::Lt) => "<",
        Some(BinaryOp::LtEq) => "<=",
        Some(BinaryOp::Gt) => ">",
        Some(BinaryOp::GtEq) => ">=",
        Some(BinaryOp::And) => "&&",
        Some(BinaryOp::Or) => "||",
        Some(BinaryOp::Pipeline) => "|>",
        None => "?",
    });
    out.push(' ');
    if let Some(rhs) = b.rhs() {
        write_expr(out, &rhs, ctx);
    }
}

fn write_null_coalesce(out: &mut String, n: &NullCoalesceExpr, ctx: FmtCtx) {
    if let Some(lhs) = n.lhs() {
        write_expr(out, &lhs, ctx);
    }
    out.push_str(" ?? ");
    if let Some(rhs) = n.rhs() {
        write_expr(out, &rhs, ctx);
    }
}

fn write_type_check(out: &mut String, tc: &TypeCheckExpr, ctx: FmtCtx) {
    if let Some(operand) = tc.operand() {
        write_expr(out, &operand, ctx);
    }
    out.push_str(" is ");
    if let Some(ty) = tc.ty() {
        out.push_str(&format_type(&ty));
    }
}

fn write_type_cast(out: &mut String, tc: &TypeCastExpr, ctx: FmtCtx) {
    if let Some(operand) = tc.operand() {
        write_expr(out, &operand, ctx);
    }
    out.push_str(" as ");
    if let Some(ty) = tc.ty() {
        out.push_str(&format_type(&ty));
    }
}

fn write_if(out: &mut String, i: &IfExpr, ctx: FmtCtx) {
    out.push_str("if (");
    if let Some(cond) = i.condition() {
        write_expr(out, &cond, ctx);
    }
    out.push_str(") ");
    if let Some(t) = i.then_branch() {
        write_expr(out, &t, ctx);
    }
    out.push_str(" else ");
    if let Some(e) = i.else_branch() {
        write_expr(out, &e, ctx);
    }
}

fn write_let(out: &mut String, l: &LetExpr, ctx: FmtCtx) {
    out.push_str("let (");
    if let Some(b) = l.binding() {
        if let Some(name) = b.name() {
            out.push_str(&cst::ident_text(&name));
        }
        if let Some(ty) = b.ty() {
            out.push_str(": ");
            out.push_str(&format_type(&ty));
        }
    }
    out.push_str(" = ");
    if let Some(v) = l.value() {
        write_expr(out, &v, ctx);
    }
    out.push_str(") ");
    if let Some(body) = l.body() {
        write_expr(out, &body, ctx);
    }
}

fn write_lambda(out: &mut String, l: &LambdaExpr, ctx: FmtCtx) {
    let params: Vec<Parameter> = l
        .parameters()
        .map(|p| p.parameters().collect())
        .unwrap_or_default();
    out.push('(');
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        if let Some(name) = p.name() {
            out.push_str(&cst::ident_text(&name));
        }
        if let Some(ty) = p.ty() {
            out.push_str(": ");
            out.push_str(&format_type(&ty));
        }
    }
    out.push_str(") -> ");
    if let Some(body) = l.body() {
        write_expr(out, &body, ctx);
    }
}

fn write_call(out: &mut String, c: &CallExpr, ctx: FmtCtx) {
    if let Some(callee) = c.callee() {
        write_expr(out, &callee, ctx);
    }
    out.push('(');
    let args = c
        .arg_list()
        .map(|al| ArgList::args(&al).collect::<Vec<_>>())
        .unwrap_or_default();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_expr(out, a, ctx);
    }
    out.push(')');
}

fn write_index(out: &mut String, i: &IndexExpr, ctx: FmtCtx) {
    if let Some(rec) = i.receiver() {
        write_expr(out, &rec, ctx);
    }
    out.push('[');
    if let Some(idx) = i.index() {
        write_expr(out, &idx, ctx);
    }
    out.push(']');
}

fn write_member(out: &mut String, m: &MemberExpr, ctx: FmtCtx) {
    if let Some(rec) = m.receiver() {
        write_expr(out, &rec, ctx);
    }
    out.push_str(if m.nullable() { "?." } else { "." });
    if let Some(name) = m.name() {
        out.push_str(&cst::ident_text(&name));
    }
}

fn write_non_null(out: &mut String, n: &NonNullExpr, ctx: FmtCtx) {
    if let Some(operand) = n.operand() {
        write_expr(out, &operand, ctx);
    }
    out.push_str("!!");
}

fn write_new(out: &mut String, n: &NewExpr, ctx: FmtCtx) {
    out.push_str("new ");
    if let Some(ty) = n.ty() {
        out.push_str(&format_type(&ty));
        out.push(' ');
    }
    if let Some(body) = n.body() {
        write_object_body(out, &body, ctx);
    }
}

fn write_amends(out: &mut String, a: &AmendsExpr, ctx: FmtCtx) {
    if let Some(base) = a.base() {
        write_expr(out, &base, ctx);
    }
    out.push(' ');
    if let Some(body) = a.body() {
        write_object_body(out, &body, ctx);
    }
}

fn write_throw(out: &mut String, t: &ThrowExpr, ctx: FmtCtx) {
    out.push_str("throw(");
    if let Some(arg) = t.argument() {
        write_expr(out, &arg, ctx);
    }
    out.push(')');
}

fn write_trace(out: &mut String, t: &TraceExpr, ctx: FmtCtx) {
    out.push_str("trace(");
    if let Some(arg) = t.argument() {
        write_expr(out, &arg, ctx);
    }
    out.push(')');
}

fn write_read(out: &mut String, r: &ReadExpr, ctx: FmtCtx) {
    out.push_str(match r.read_kind() {
        Some(ReadKind::Read) => "read(",
        Some(ReadKind::ReadOrNull) => "read?(",
        Some(ReadKind::ReadGlob) => "read*(",
        None => "read(",
    });
    if let Some(arg) = r.argument() {
        write_expr(out, &arg, ctx);
    }
    out.push(')');
}

// ----------------------------------------------------------------------
// Types

fn format_type(ty: &Type) -> String {
    let mut out = String::new();
    write_type(&mut out, ty);
    out
}

fn write_type(out: &mut String, ty: &Type) {
    match ty {
        Type::Named(n) => write_named_type(out, n),
        Type::Nullable(n) => {
            if let Some(inner) = n.inner() {
                write_type(out, &inner);
            }
            out.push('?');
        }
        Type::Union(u) => write_union_type(out, u),
        Type::Function(f) => write_function_type(out, f),
        Type::Parenthesized(p) => write_parenthesized_type(out, p),
        Type::StringLiteral(s) => {
            if let Some(t) = s.token() {
                out.push_str(t.text());
            }
        }
        Type::Unknown(_) => out.push_str("unknown"),
        Type::Nothing(_) => out.push_str("nothing"),
        Type::Module(_) => out.push_str("module"),
        Type::Error(_) => out.push_str("<error>"),
    }
}

fn write_named_type(out: &mut String, n: &NamedType) {
    if let Some(name) = n.name() {
        out.push_str(&name.text_joined());
    }
    if let Some(args) = n.type_arguments() {
        let arguments: Vec<Type> = args.arguments().collect();
        if !arguments.is_empty() {
            out.push('<');
            for (i, a) in arguments.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_type(out, a);
            }
            out.push('>');
        }
    }
}

fn write_union_type(out: &mut String, u: &UnionType) {
    let members: Vec<Type> = u.members().collect();
    for (i, m) in members.iter().enumerate() {
        if i > 0 {
            out.push_str(" | ");
        }
        write_type(out, m);
    }
}

fn write_function_type(out: &mut String, f: &FunctionType) {
    out.push('(');
    let params: Vec<Type> = f.parameters().collect();
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_type(out, p);
    }
    out.push_str(") -> ");
    if let Some(r) = f.result() {
        write_type(out, &r);
    }
}

fn write_parenthesized_type(out: &mut String, p: &ParenthesizedType) {
    out.push('(');
    if let Some(inner) = p.inner() {
        write_type(out, &inner);
    }
    out.push(')');
}

// ----------------------------------------------------------------------
// Modifier / type-parameter helpers

fn format_modifiers_list(mods: &[Modifier]) -> String {
    mods.iter()
        .filter_map(|m| m.kind())
        .map(|k| match k {
            ModifierKind::Abstract => "abstract",
            ModifierKind::Open => "open",
            ModifierKind::Local => "local",
            ModifierKind::Hidden => "hidden",
            ModifierKind::Fixed => "fixed",
            ModifierKind::External => "external",
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_type_parameters(params: &[TypeParameter]) -> String {
    let mut out = String::from("<");
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        match p.variance() {
            Some(Variance::In) => out.push_str("in "),
            Some(Variance::Out) => out.push_str("out "),
            None => {}
        }
        if let Some(name) = p.name() {
            out.push_str(&cst::ident_text(&name));
        }
    }
    out.push('>');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cst::Module as CstModule;
    use crate::parse;

    fn format_str(input: &str) -> String {
        let parsed = parse(input);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let module = CstModule::cast(parsed.syntax()).expect("module root");
        format_module(&module)
    }

    #[test]
    fn formats_property() {
        let out = format_str("name: String = \"alice\"");
        assert_eq!(out, "name: String = \"alice\"\n");
    }

    #[test]
    fn formats_class_with_members() {
        let src = "class Person { name: String\nage: Int = 0 }";
        let out = format_str(src);
        let expected = "class Person {\n  name: String\n  age: Int = 0\n}\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn formats_import() {
        let out = format_str(r#"import "other.pkl" as other"#);
        assert!(out.trim() == "import \"other.pkl\" as other", "{}", out);
    }

    #[test]
    fn idempotent_on_self() {
        let src = "class Foo {\n  bar: Int = 1\n}\n";
        let once = format_str(src);
        let twice = format_str(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn preserves_line_comment_before_top_level_item() {
        let src = "// keep me\nx: Int = 1\n";
        let out = format_str(src);
        assert_eq!(out, "// keep me\nx: Int = 1\n");
    }

    #[test]
    fn preserves_block_comment_before_class_member() {
        let src = "class Foo {\n  /* preserved */\n  x: Int\n  y: Int\n}\n";
        let out = format_str(src);
        assert!(out.contains("/* preserved */"), "{out}");
    }

    #[test]
    fn preserves_comment_in_object_body() {
        let src = "config {\n  // describe a\n  a = 1\n  // describe b\n  b = 2\n}\n";
        let out = format_str(src);
        assert!(out.contains("// describe a"), "{out}");
        assert!(out.contains("// describe b"), "{out}");
    }

    #[test]
    fn preserves_doc_comment() {
        let src = "/// my prop\nfoo: Int = 1\n";
        let out = format_str(src);
        assert!(out.contains("/// my prop"), "{out}");
    }
}

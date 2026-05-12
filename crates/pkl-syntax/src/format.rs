//! Pretty-print a Pkl [`ast::Module`] back to source text.
//!
//! The formatter is deliberately conservative:
//!
//! * Two-space indentation, LF line endings.
//! * One blank line between top-level items.
//! * Object bodies and class bodies expand to multi-line form whenever
//!   they contain more than one member; otherwise they stay inline.
//! * Doc comments are preserved verbatim; ordinary line/block comments
//!   are lost (a known limitation that pairs with the not-yet-added
//!   lossless syntax tree).

use crate::ast::*;

/// Format an entire module. Always ends with a single trailing newline.
pub fn format_module(module: &Module) -> String {
    let mut out = String::new();
    let mut ctx = FmtCtx { indent: 0 };

    if let Some(header) = &module.header {
        write_module_header(&mut out, header);
        out.push('\n');
    }

    if !module.imports.is_empty() {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        for import in &module.imports {
            write_import(&mut out, import);
            out.push('\n');
        }
    }

    let mut first_item = true;
    for item in &module.items {
        if !out.is_empty() && (!first_item || !out.ends_with("\n\n")) {
            out.push('\n');
        }
        write_item(&mut out, item, &mut ctx);
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

fn write_module_header(out: &mut String, header: &ModuleHeader) {
    if let Some(doc) = &header.doc_comment {
        for line in doc.lines() {
            out.push_str("/// ");
            out.push_str(line);
            out.push('\n');
        }
    }
    for ann in &header.annotations {
        write_annotation(out, ann, FmtCtx { indent: 0 });
        out.push('\n');
    }
    let mut prefix = String::new();
    if !header.modifiers.is_empty() {
        prefix.push_str(&format_modifiers_list(&header.modifiers));
        prefix.push(' ');
    }
    if let Some(name) = &header.name {
        out.push_str(&prefix);
        out.push_str("module ");
        write_qualified_name(out, name);
    } else if let Some(clause) = &header.clause {
        match clause {
            ExtendsAmendsClause::Amends { target, .. } => {
                out.push_str(&prefix);
                out.push_str("amends ");
                out.push_str(&target.raw);
            }
            ExtendsAmendsClause::Extends { target, .. } => {
                out.push_str(&prefix);
                out.push_str("extends ");
                out.push_str(&target.raw);
            }
        }
    }
}

fn write_import(out: &mut String, import: &Import) {
    out.push_str(if import.is_glob {
        "import* "
    } else {
        "import "
    });
    out.push_str(&import.path.raw);
    if let Some(alias) = &import.alias {
        out.push_str(" as ");
        out.push_str(&alias.name);
    }
}

fn write_item(out: &mut String, item: &Item, ctx: &mut FmtCtx) {
    match item {
        Item::Class(c) => write_class(out, c, *ctx),
        Item::TypeAlias(t) => write_typealias(out, t, *ctx),
        Item::Property(p) => write_property(out, p, *ctx),
        Item::Method(m) => write_method(out, m, *ctx),
        Item::Error(_) => {}
    }
}

fn write_class(out: &mut String, c: &ClassDecl, ctx: FmtCtx) {
    write_doc(out, c.doc_comment.as_deref(), ctx);
    for a in &c.annotations {
        write_annotation(out, a, ctx);
        out.push('\n');
        out.push_str(&ctx.pad());
    }
    let pad = ctx.pad();
    out.push_str(&pad);
    if !c.modifiers.is_empty() {
        out.push_str(&format_modifiers_list(&c.modifiers));
        out.push(' ');
    }
    out.push_str("class ");
    out.push_str(&c.name.name);
    if !c.type_parameters.is_empty() {
        out.push_str(&format_type_parameters(&c.type_parameters));
    }
    if let Some(ext) = &c.extends {
        out.push_str(" extends ");
        out.push_str(&format_type(ext));
    }
    if let Some(body) = &c.body {
        if body.members.is_empty() {
            out.push_str(" {}");
        } else {
            out.push_str(" {\n");
            for member in &body.members {
                let child = ctx.child();
                out.push_str(&child.pad());
                match member {
                    ClassMember::Property(p) => write_property_inline(out, p, child),
                    ClassMember::Method(m) => write_method_inline(out, m, child),
                }
                out.push('\n');
            }
            out.push_str(&pad);
            out.push('}');
        }
    }
}

fn write_typealias(out: &mut String, t: &TypeAliasDecl, ctx: FmtCtx) {
    write_doc(out, t.doc_comment.as_deref(), ctx);
    out.push_str(&ctx.pad());
    if !t.modifiers.is_empty() {
        out.push_str(&format_modifiers_list(&t.modifiers));
        out.push(' ');
    }
    out.push_str("typealias ");
    out.push_str(&t.name.name);
    if !t.type_parameters.is_empty() {
        out.push_str(&format_type_parameters(&t.type_parameters));
    }
    if let Some(aliased) = &t.aliased {
        out.push_str(" = ");
        out.push_str(&format_type(aliased));
    }
}

fn write_property(out: &mut String, p: &PropertyDecl, ctx: FmtCtx) {
    write_doc(out, p.doc_comment.as_deref(), ctx);
    out.push_str(&ctx.pad());
    write_property_inline(out, p, ctx);
}

fn write_property_inline(out: &mut String, p: &PropertyDecl, ctx: FmtCtx) {
    for a in &p.annotations {
        write_annotation(out, a, ctx);
        out.push(' ');
    }
    if !p.modifiers.is_empty() {
        out.push_str(&format_modifiers_list(&p.modifiers));
        out.push(' ');
    }
    out.push_str(&p.name.name);
    if let Some(ty) = &p.ty {
        out.push_str(": ");
        out.push_str(&format_type(ty));
    }
    match &p.value {
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
    write_doc(out, m.doc_comment.as_deref(), ctx);
    out.push_str(&ctx.pad());
    write_method_inline(out, m, ctx);
}

fn write_method_inline(out: &mut String, m: &MethodDecl, ctx: FmtCtx) {
    for a in &m.annotations {
        write_annotation(out, a, ctx);
        out.push(' ');
    }
    if !m.modifiers.is_empty() {
        out.push_str(&format_modifiers_list(&m.modifiers));
        out.push(' ');
    }
    out.push_str("function ");
    out.push_str(&m.name.name);
    if !m.type_parameters.is_empty() {
        out.push_str(&format_type_parameters(&m.type_parameters));
    }
    out.push('(');
    for (i, p) in m.parameters.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&p.name.name);
        if let Some(ty) = &p.ty {
            out.push_str(": ");
            out.push_str(&format_type(ty));
        }
    }
    out.push(')');
    if let Some(ret) = &m.return_type {
        out.push_str(": ");
        out.push_str(&format_type(ret));
    }
    if let Some(body) = &m.body {
        out.push_str(" = ");
        write_expr(out, body, ctx);
    }
}

fn write_annotation(out: &mut String, a: &Annotation, ctx: FmtCtx) {
    out.push('@');
    write_qualified_name(out, &a.name);
    if let Some(body) = &a.body {
        out.push(' ');
        write_object_body(out, body, ctx);
    }
}

fn write_qualified_name(out: &mut String, name: &QualifiedName) {
    for (i, seg) in name.segments.iter().enumerate() {
        if i > 0 {
            out.push('.');
        }
        out.push_str(&seg.name);
    }
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

fn write_object_body(out: &mut String, body: &ObjectBody, ctx: FmtCtx) {
    if body.members.is_empty() && body.parameters.is_empty() {
        out.push_str("{}");
        return;
    }
    if body.members.len() == 1 && body.parameters.is_empty() {
        out.push_str("{ ");
        write_object_member(out, &body.members[0], ctx);
        out.push_str(" }");
        return;
    }
    out.push_str("{\n");
    let child = ctx.child();
    if !body.parameters.is_empty() {
        out.push_str(&child.pad());
        for (i, p) in body.parameters.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&p.name.name);
            if let Some(ty) = &p.ty {
                out.push_str(": ");
                out.push_str(&format_type(ty));
            }
        }
        out.push_str(" ->\n");
    }
    for member in &body.members {
        out.push_str(&child.pad());
        write_object_member(out, member, child);
        out.push('\n');
    }
    out.push_str(&ctx.pad());
    out.push('}');
}

fn write_object_member(out: &mut String, member: &ObjectMember, ctx: FmtCtx) {
    match member {
        ObjectMember::Property(p) => write_property_inline(out, p, ctx),
        ObjectMember::Method(m) => write_method_inline(out, m, ctx),
        ObjectMember::Element(e) => write_expr(out, e, ctx),
        ObjectMember::Entry { key, value, .. } => {
            out.push('[');
            write_expr(out, key, ctx);
            out.push_str("] ");
            match value {
                PropertyValue::Expr(e) => {
                    out.push_str("= ");
                    write_expr(out, e, ctx);
                }
                PropertyValue::ObjectBody(body) => write_object_body(out, body, ctx),
            }
        }
        ObjectMember::When {
            cond,
            then_body,
            else_body,
            ..
        } => {
            out.push_str("when (");
            write_expr(out, cond, ctx);
            out.push_str(") ");
            write_object_body(out, then_body, ctx);
            if let Some(e) = else_body {
                out.push_str(" else ");
                write_object_body(out, e, ctx);
            }
        }
        ObjectMember::For {
            bindings,
            iterable,
            body,
            ..
        } => {
            out.push_str("for (");
            for (i, b) in bindings.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&b.name.name);
            }
            out.push_str(" in ");
            write_expr(out, iterable, ctx);
            out.push_str(") ");
            write_object_body(out, body, ctx);
        }
        ObjectMember::Spread { expr, .. } => {
            out.push_str("...");
            write_expr(out, expr, ctx);
        }
    }
}

fn write_expr(out: &mut String, expr: &Expr, ctx: FmtCtx) {
    match expr {
        Expr::Literal(lit) => match lit {
            Literal::Int { raw, .. } | Literal::Float { raw, .. } => out.push_str(raw),
            Literal::Bool { value, .. } => out.push_str(if *value { "true" } else { "false" }),
            Literal::Null { .. } => out.push_str("null"),
            Literal::String(s) => out.push_str(&s.raw),
        },
        Expr::Ident(id) => out.push_str(&id.name),
        Expr::SpecialIdent { kind, .. } => out.push_str(match kind {
            SpecialIdentKind::This => "this",
            SpecialIdentKind::Super => "super",
            SpecialIdentKind::Outer => "outer",
            SpecialIdentKind::Module => "module",
        }),
        Expr::Paren { inner, .. } => {
            out.push('(');
            write_expr(out, inner, ctx);
            out.push(')');
        }
        Expr::Unary { op, operand, .. } => {
            out.push_str(match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "!",
            });
            write_expr(out, operand, ctx);
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            write_expr(out, lhs, ctx);
            out.push(' ');
            out.push_str(match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Rem => "%",
                BinaryOp::Pow => "**",
                BinaryOp::Eq => "==",
                BinaryOp::NotEq => "!=",
                BinaryOp::Lt => "<",
                BinaryOp::LtEq => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::GtEq => ">=",
                BinaryOp::And => "&&",
                BinaryOp::Or => "||",
                BinaryOp::NullCoalesce => "??",
                BinaryOp::Pipeline => "|>",
            });
            out.push(' ');
            write_expr(out, rhs, ctx);
        }
        Expr::TypeCheck { operand, ty, .. } => {
            write_expr(out, operand, ctx);
            out.push_str(" is ");
            out.push_str(&format_type(ty));
        }
        Expr::TypeCast { operand, ty, .. } => {
            write_expr(out, operand, ctx);
            out.push_str(" as ");
            out.push_str(&format_type(ty));
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            out.push_str("if (");
            write_expr(out, cond, ctx);
            out.push_str(") ");
            write_expr(out, then_branch, ctx);
            out.push_str(" else ");
            write_expr(out, else_branch, ctx);
        }
        Expr::Let {
            binding,
            value,
            body,
            ..
        } => {
            out.push_str("let (");
            out.push_str(&binding.name.name);
            if let Some(ty) = &binding.ty {
                out.push_str(": ");
                out.push_str(&format_type(ty));
            }
            out.push_str(" = ");
            write_expr(out, value, ctx);
            out.push_str(") ");
            write_expr(out, body, ctx);
        }
        Expr::Lambda {
            parameters, body, ..
        } => {
            out.push('(');
            for (i, p) in parameters.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&p.name.name);
                if let Some(ty) = &p.ty {
                    out.push_str(": ");
                    out.push_str(&format_type(ty));
                }
            }
            out.push_str(") -> ");
            write_expr(out, body, ctx);
        }
        Expr::Call { callee, args, .. } => {
            write_expr(out, callee, ctx);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_expr(out, a, ctx);
            }
            out.push(')');
        }
        Expr::Index {
            receiver, index, ..
        } => {
            write_expr(out, receiver, ctx);
            out.push('[');
            write_expr(out, index, ctx);
            out.push(']');
        }
        Expr::Member {
            receiver,
            name,
            nullable,
            ..
        } => {
            write_expr(out, receiver, ctx);
            out.push_str(if *nullable { "?." } else { "." });
            out.push_str(&name.name);
        }
        Expr::NonNull { operand, .. } => {
            write_expr(out, operand, ctx);
            out.push_str("!!");
        }
        Expr::New { ty, body, .. } => {
            out.push_str("new ");
            if let Some(t) = ty {
                out.push_str(&format_type(t));
                out.push(' ');
            }
            write_object_body(out, body, ctx);
        }
        Expr::AmendsObject { base, body, .. } => {
            write_expr(out, base, ctx);
            out.push(' ');
            write_object_body(out, body, ctx);
        }
        Expr::Throw { argument, .. } => {
            out.push_str("throw(");
            write_expr(out, argument, ctx);
            out.push(')');
        }
        Expr::Trace { argument, .. } => {
            out.push_str("trace(");
            write_expr(out, argument, ctx);
            out.push(')');
        }
        Expr::Read { argument, kind, .. } => {
            out.push_str(match kind {
                ReadKind::Read => "read(",
                ReadKind::ReadOrNull => "read?(",
                ReadKind::ReadGlob => "read*(",
            });
            write_expr(out, argument, ctx);
            out.push(')');
        }
        Expr::Error { .. } => out.push_str("<error>"),
    }
}

// ----------------------------------------------------------------------
// Pretty helpers (mirroring `pkl-analyze::pretty` but kept here so the
// formatter doesn't depend on the analyzer).

fn format_type(ty: &TypeRef) -> String {
    let mut out = String::new();
    write_type(&mut out, ty);
    out
}

fn write_type(out: &mut String, ty: &TypeRef) {
    match ty {
        TypeRef::Named {
            name, arguments, ..
        } => {
            for (i, seg) in name.segments.iter().enumerate() {
                if i > 0 {
                    out.push('.');
                }
                out.push_str(&seg.name);
            }
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
        TypeRef::Nullable { inner, .. } => {
            write_type(out, inner);
            out.push('?');
        }
        TypeRef::Union { members, .. } => {
            for (i, m) in members.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                write_type(out, m);
            }
        }
        TypeRef::Function {
            parameters, result, ..
        } => {
            out.push('(');
            for (i, p) in parameters.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_type(out, p);
            }
            out.push_str(") -> ");
            write_type(out, result);
        }
        TypeRef::Parenthesized { inner, .. } => {
            out.push('(');
            write_type(out, inner);
            out.push(')');
        }
        TypeRef::StringLiteral(s) => out.push_str(&s.raw),
        TypeRef::Unknown(_) => out.push_str("unknown"),
        TypeRef::Nothing(_) => out.push_str("nothing"),
        TypeRef::Module(_) => out.push_str("module"),
        TypeRef::Error { .. } => out.push_str("<error>"),
    }
}

fn format_modifiers_list(mods: &[Modifier]) -> String {
    mods.iter()
        .map(|m| match m.kind {
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
        match p.variance {
            Some(Variance::In) => out.push_str("in "),
            Some(Variance::Out) => out.push_str("out "),
            None => {}
        }
        out.push_str(&p.name.name);
    }
    out.push('>');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn round_trip(input: &str) -> String {
        let parsed = parse(input);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        format_module(&parsed.module)
    }

    #[test]
    fn formats_property() {
        let out = round_trip("name: String = \"alice\"");
        assert_eq!(out, "name: String = \"alice\"\n");
    }

    #[test]
    fn formats_class_with_members() {
        let src = "class Person { name: String\nage: Int = 0 }";
        let out = round_trip(src);
        let expected = "class Person {\n  name: String\n  age: Int = 0\n}\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn formats_import() {
        let out = round_trip(r#"import "other.pkl" as other"#);
        assert!(out.trim() == "import \"other.pkl\" as other", "{}", out);
    }

    #[test]
    fn idempotent_on_self() {
        let src = "class Foo {\n  bar: Int = 1\n}\n";
        let once = round_trip(src);
        let twice = round_trip(&once);
        assert_eq!(once, twice);
    }
}

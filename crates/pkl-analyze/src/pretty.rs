//! Pretty-printer for AST fragments used in hover output.

use std::fmt::Write;

use pkl_syntax::ast::*;

pub fn format_type(ty: &TypeRef) -> String {
    let mut out = String::new();
    write_type(&mut out, ty);
    out
}

fn write_type(out: &mut String, ty: &TypeRef) {
    match ty {
        TypeRef::Named {
            name, arguments, ..
        } => {
            write_qualified(out, name);
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

fn write_qualified(out: &mut String, name: &QualifiedName) {
    for (i, seg) in name.segments.iter().enumerate() {
        if i > 0 {
            out.push('.');
        }
        out.push_str(&seg.name);
    }
}

pub fn format_modifiers(mods: &[Modifier]) -> String {
    let mut out = String::new();
    for m in mods {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(match m.kind {
            ModifierKind::Abstract => "abstract",
            ModifierKind::Open => "open",
            ModifierKind::Local => "local",
            ModifierKind::Hidden => "hidden",
            ModifierKind::Fixed => "fixed",
            ModifierKind::External => "external",
        });
    }
    out
}

pub fn format_type_parameters(params: &[TypeParameter]) -> String {
    if params.is_empty() {
        return String::new();
    }
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

pub fn format_parameters(params: &[Parameter]) -> String {
    let mut out = String::from("(");
    for (i, p) in params.iter().enumerate() {
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
    out
}

pub fn format_class_signature(c: &ClassDecl) -> String {
    let mut out = String::new();
    let mods = format_modifiers(&c.modifiers);
    if !mods.is_empty() {
        out.push_str(&mods);
        out.push(' ');
    }
    out.push_str("class ");
    out.push_str(&c.name.name);
    out.push_str(&format_type_parameters(&c.type_parameters));
    if let Some(ext) = &c.extends {
        out.push_str(" extends ");
        out.push_str(&format_type(ext));
    }
    out
}

pub fn format_typealias_signature(t: &TypeAliasDecl) -> String {
    let mut out = String::new();
    let mods = format_modifiers(&t.modifiers);
    if !mods.is_empty() {
        out.push_str(&mods);
        out.push(' ');
    }
    out.push_str("typealias ");
    out.push_str(&t.name.name);
    out.push_str(&format_type_parameters(&t.type_parameters));
    if let Some(aliased) = &t.aliased {
        out.push_str(" = ");
        out.push_str(&format_type(aliased));
    }
    out
}

pub fn format_property_signature(p: &PropertyDecl) -> String {
    let mut out = String::new();
    let mods = format_modifiers(&p.modifiers);
    if !mods.is_empty() {
        out.push_str(&mods);
        out.push(' ');
    }
    out.push_str(&p.name.name);
    if let Some(ty) = &p.ty {
        out.push_str(": ");
        out.push_str(&format_type(ty));
    }
    out
}

pub fn format_method_signature(m: &MethodDecl) -> String {
    let mut out = String::new();
    let mods = format_modifiers(&m.modifiers);
    if !mods.is_empty() {
        out.push_str(&mods);
        out.push(' ');
    }
    out.push_str("function ");
    out.push_str(&m.name.name);
    out.push_str(&format_type_parameters(&m.type_parameters));
    out.push_str(&format_parameters(&m.parameters));
    if let Some(ret) = &m.return_type {
        out.push_str(": ");
        out.push_str(&format_type(ret));
    }
    out
}

pub fn format_parameter_signature(p: &Parameter) -> String {
    let mut out = String::new();
    out.push_str(&p.name.name);
    if let Some(ty) = &p.ty {
        out.push_str(": ");
        out.push_str(&format_type(ty));
    }
    out
}

/// Helper used by the test suite — kept here so callers don't have to know
/// about `std::fmt::Write`.
pub fn join(parts: &[&str], sep: &str) -> String {
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            let _ = write!(out, "{}", sep);
        }
        let _ = write!(out, "{}", p);
    }
    out
}

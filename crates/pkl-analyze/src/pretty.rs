//! Pretty-printer for CST fragments used in hover output.

use std::fmt::Write;

use pkl_syntax::cst::{
    self, ident_text, AstNode, ClassDecl, FunctionType, MethodDecl, Modifier, ModifierKind,
    NamedType, NullableType, Parameter, ParameterList, ParenthesizedType, QualifiedName, Type,
    TypeAliasDecl, TypeParameter, TypeParameterList, UnionType, Variance,
};
use pkl_syntax::{SyntaxKind, SyntaxNode};

pub fn format_type(ty: &Type) -> String {
    let mut out = String::new();
    write_type(&mut out, ty);
    out
}

fn write_type(out: &mut String, ty: &Type) {
    match ty {
        Type::Named(n) => write_named(out, n),
        Type::Nullable(n) => write_nullable(out, n),
        Type::Union(u) => write_union(out, u),
        Type::Function(f) => write_function(out, f),
        Type::Parenthesized(p) => write_parenthesized(out, p),
        Type::StringLiteral(s) => {
            if let Some(tok) = s.token() {
                out.push_str(tok.text());
            }
        }
        Type::Unknown(_) => out.push_str("unknown"),
        Type::Nothing(_) => out.push_str("nothing"),
        Type::Module(_) => out.push_str("module"),
        Type::Error(_) => out.push_str("<error>"),
    }
}

fn write_named(out: &mut String, n: &NamedType) {
    if let Some(qn) = n.name() {
        write_qualified(out, &qn);
    }
    if let Some(args) = n.type_arguments() {
        let args: Vec<Type> = args.arguments().collect();
        if !args.is_empty() {
            out.push('<');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_type(out, a);
            }
            out.push('>');
        }
    }
}

fn write_nullable(out: &mut String, n: &NullableType) {
    if let Some(inner) = n.inner() {
        write_type(out, &inner);
    }
    out.push('?');
}

fn write_union(out: &mut String, u: &UnionType) {
    for (i, m) in u.members().enumerate() {
        if i > 0 {
            out.push_str(" | ");
        }
        write_type(out, &m);
    }
}

fn write_function(out: &mut String, f: &FunctionType) {
    out.push('(');
    for (i, p) in f.parameters().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_type(out, &p);
    }
    out.push_str(") -> ");
    if let Some(result) = f.result() {
        write_type(out, &result);
    }
}

fn write_parenthesized(out: &mut String, p: &ParenthesizedType) {
    out.push('(');
    if let Some(inner) = p.inner() {
        write_type(out, &inner);
    }
    out.push(')');
}

fn write_qualified(out: &mut String, name: &QualifiedName) {
    for (i, seg) in name.segments().enumerate() {
        if i > 0 {
            out.push('.');
        }
        out.push_str(&ident_text(&seg));
    }
}

pub fn format_modifiers<'a>(mods: impl IntoIterator<Item = &'a Modifier>) -> String {
    let mut out = String::new();
    for m in mods {
        let Some(kind) = m.kind() else { continue };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(modifier_keyword(kind));
    }
    out
}

pub fn modifier_keyword(k: ModifierKind) -> &'static str {
    match k {
        ModifierKind::Abstract => "abstract",
        ModifierKind::Open => "open",
        ModifierKind::Local => "local",
        ModifierKind::Hidden => "hidden",
        ModifierKind::Fixed => "fixed",
        ModifierKind::External => "external",
    }
}

pub fn format_type_parameters(params: Option<&TypeParameterList>) -> String {
    let Some(list) = params else {
        return String::new();
    };
    let params: Vec<TypeParameter> = list.parameters().collect();
    if params.is_empty() {
        return String::new();
    }
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
            out.push_str(&ident_text(&name));
        }
    }
    out.push('>');
    out
}

pub fn format_parameters(params: Option<&ParameterList>) -> String {
    let mut out = String::from("(");
    if let Some(list) = params {
        for (i, p) in list.parameters().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_parameter(&mut out, &p);
        }
    }
    out.push(')');
    out
}

fn write_parameter(out: &mut String, p: &Parameter) {
    if let Some(name) = p.name() {
        out.push_str(&ident_text(&name));
    }
    if let Some(ty) = p.ty() {
        out.push_str(": ");
        out.push_str(&format_type(&ty));
    }
}

pub fn format_class_signature(c: &ClassDecl) -> String {
    let mut out = String::new();
    let mods: Vec<Modifier> = c.modifiers().collect();
    let mods_str = format_modifiers(mods.iter());
    if !mods_str.is_empty() {
        out.push_str(&mods_str);
        out.push(' ');
    }
    out.push_str("class ");
    if let Some(name) = c.name() {
        out.push_str(&ident_text(&name));
    }
    let tps = c.type_parameters();
    out.push_str(&format_type_parameters(tps.as_ref()));
    if let Some(ext) = c.extends() {
        out.push_str(" extends ");
        out.push_str(&format_type(&ext));
    }
    out
}

pub fn format_typealias_signature(t: &TypeAliasDecl) -> String {
    let mut out = String::new();
    let mods: Vec<Modifier> = t.modifiers().collect();
    let mods_str = format_modifiers(mods.iter());
    if !mods_str.is_empty() {
        out.push_str(&mods_str);
        out.push(' ');
    }
    out.push_str("typealias ");
    if let Some(name) = t.name() {
        out.push_str(&ident_text(&name));
    }
    let tps = t.type_parameters();
    out.push_str(&format_type_parameters(tps.as_ref()));
    if let Some(aliased) = t.aliased_type() {
        out.push_str(" = ");
        out.push_str(&format_type(&aliased));
    }
    out
}

/// Format a property declaration's signature line. Works uniformly across
/// the three "property" flavours in the CST: top-level [`cst::PropertyDecl`],
/// class member [`cst::ClassPropertyDecl`], and object body
/// [`cst::ObjectProperty`]. The accessors are identical thanks to
/// `impl_property_accessors!` in `pkl-syntax::cst`, so we just dispatch on
/// the syntax kind.
pub fn format_property_signature(syntax: &SyntaxNode) -> String {
    match syntax.kind() {
        SyntaxKind::PropertyDecl => cst::PropertyDecl::cast(syntax.clone())
            .map(|p| format_property_like(&p.modifiers().collect::<Vec<_>>(), p.name(), p.ty()))
            .unwrap_or_default(),
        SyntaxKind::ClassPropertyDecl => cst::ClassPropertyDecl::cast(syntax.clone())
            .map(|p| format_property_like(&p.modifiers().collect::<Vec<_>>(), p.name(), p.ty()))
            .unwrap_or_default(),
        SyntaxKind::ObjectProperty => cst::ObjectProperty::cast(syntax.clone())
            .map(|p| format_property_like(&p.modifiers().collect::<Vec<_>>(), p.name(), p.ty()))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn format_property_like(
    mods: &[Modifier],
    name: Option<pkl_syntax::SyntaxToken>,
    ty: Option<Type>,
) -> String {
    let mut out = String::new();
    let mods_str = format_modifiers(mods.iter());
    if !mods_str.is_empty() {
        out.push_str(&mods_str);
        out.push(' ');
    }
    if let Some(name) = name {
        out.push_str(&ident_text(&name));
    }
    if let Some(ty) = ty {
        out.push_str(": ");
        out.push_str(&format_type(&ty));
    }
    out
}

pub fn format_method_signature(m: &MethodDecl) -> String {
    format_method_like(
        &m.modifiers().collect::<Vec<_>>(),
        m.name(),
        m.type_parameters(),
        m.parameters(),
        m.return_type(),
    )
}

pub fn format_class_method_signature(m: &cst::ClassMethodDecl) -> String {
    format_method_like(
        &m.modifiers().collect::<Vec<_>>(),
        m.name(),
        m.type_parameters(),
        m.parameters(),
        m.return_type(),
    )
}

pub fn format_object_method_signature(m: &cst::ObjectMethod) -> String {
    format_method_like(
        &m.modifiers().collect::<Vec<_>>(),
        m.name(),
        m.type_parameters(),
        m.parameters(),
        m.return_type(),
    )
}

fn format_method_like(
    mods: &[Modifier],
    name: Option<pkl_syntax::SyntaxToken>,
    type_params: Option<TypeParameterList>,
    params: Option<ParameterList>,
    ret: Option<Type>,
) -> String {
    let mut out = String::new();
    let mods_str = format_modifiers(mods.iter());
    if !mods_str.is_empty() {
        out.push_str(&mods_str);
        out.push(' ');
    }
    out.push_str("function ");
    if let Some(name) = name {
        out.push_str(&ident_text(&name));
    }
    out.push_str(&format_type_parameters(type_params.as_ref()));
    out.push_str(&format_parameters(params.as_ref()));
    if let Some(ret) = ret {
        out.push_str(": ");
        out.push_str(&format_type(&ret));
    }
    out
}

pub fn format_parameter_signature(p: &Parameter) -> String {
    let mut out = String::new();
    write_parameter(&mut out, p);
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

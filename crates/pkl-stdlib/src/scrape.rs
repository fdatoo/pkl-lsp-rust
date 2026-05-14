//! Build a [`StdlibType`] catalogue at runtime from the vendored Pkl
//! sources.
//!
//! The hand-curated [`crate::base`] catalogue covers the most-used
//! surface of `pkl.base`. Scraping the vendored `base.pkl` once at first
//! access fills in the long tail (`Collection.zip`, `String.matches`,
//! etc.) with signatures and doc comments lifted directly from the
//! upstream source.
//!
//! `StdlibType` and `StdlibMember` are shaped around `&'static str`,
//! which the catalogue ultimately needs for cheap reuse. The scrape
//! produces owned strings and leaks them via [`Box::leak`] — done once
//! per process, so the cost is bounded.

use std::sync::OnceLock;

use pkl_syntax::cst::{
    self, AstNode, ClassDecl, ClassMember, ClassMethodDecl, ClassPropertyDecl, FunctionType, Item,
    MethodDecl, ModifierKind, Module, NamedType, Parameter, ParenthesizedType, PropertyDecl, Type,
    TypeParameter, UnionType,
};
use pkl_syntax::SyntaxToken;

use crate::{MemberKind, StdlibKind, StdlibMember, StdlibType};

/// Every type declared in the vendored `pkl.base` source. Lazily parsed
/// on first access and cached for the rest of the process.
pub fn parsed_base() -> &'static [&'static StdlibType] {
    static CELL: OnceLock<Vec<&'static StdlibType>> = OnceLock::new();
    CELL.get_or_init(|| {
        let parsed = pkl_syntax::parse(crate::vendored::find("base").unwrap().source);
        let module = Module::cast(parsed.syntax()).expect("Module root");
        module
            .items()
            .filter_map(|item| match item {
                Item::Class(c) => Some(leak_type(&c, "pkl.base")),
                _ => None,
            })
            .collect()
    })
}

fn leak_type(c: &ClassDecl, module: &'static str) -> &'static StdlibType {
    let mut members: Vec<StdlibMember> = Vec::new();
    if let Some(body) = c.body() {
        for m in body.members() {
            match m {
                ClassMember::Property(p) => members.push(leak_class_property(&p)),
                ClassMember::Method(m) => members.push(leak_class_method(&m)),
            }
        }
    }
    let members_leaked: &'static [StdlibMember] = Box::leak(members.into_boxed_slice());
    let modifier_kinds: Vec<ModifierKind> = c.modifiers().filter_map(|m| m.kind()).collect();
    let kind = if modifier_kinds.contains(&ModifierKind::Abstract) {
        StdlibKind::AbstractClass
    } else if modifier_kinds.contains(&ModifierKind::Open) {
        StdlibKind::OpenClass
    } else {
        StdlibKind::Class
    };
    let generics: Vec<&'static str> = c
        .type_parameters()
        .map(|tps| {
            tps.parameters()
                .filter_map(|p: TypeParameter| p.name().map(|t| static_str(&cst::ident_text(&t))))
                .collect()
        })
        .unwrap_or_default();
    let generics_leaked: &'static [&'static str] = Box::leak(generics.into_boxed_slice());
    let extends = c
        .extends()
        .as_ref()
        .map(format_type)
        .map(|s| static_str(&s));
    let leaked = Box::new(StdlibType {
        name: static_str(&token_name(c.name())),
        module,
        kind,
        generics: generics_leaked,
        extends,
        doc: doc_or_empty(c.doc_comment().as_deref()),
        members: members_leaked,
    });
    Box::leak(leaked)
}

fn leak_class_property(p: &ClassPropertyDecl) -> StdlibMember {
    let ty = p
        .ty()
        .as_ref()
        .map(format_type)
        .unwrap_or_else(|| "Any".to_string());
    let signature = static_str(&format!("{}: {}", token_name(p.name()), ty));
    StdlibMember {
        name: static_str(&token_name(p.name())),
        kind: MemberKind::Property,
        signature,
        doc: doc_or_empty(p.doc_comment().as_deref()),
    }
}

#[allow(dead_code)]
fn leak_property(p: &PropertyDecl) -> StdlibMember {
    let ty = p
        .ty()
        .as_ref()
        .map(format_type)
        .unwrap_or_else(|| "Any".to_string());
    let signature = static_str(&format!("{}: {}", token_name(p.name()), ty));
    StdlibMember {
        name: static_str(&token_name(p.name())),
        kind: MemberKind::Property,
        signature,
        doc: doc_or_empty(p.doc_comment().as_deref()),
    }
}

fn leak_class_method(m: &ClassMethodDecl) -> StdlibMember {
    let mut sig = String::new();
    sig.push_str(&token_name(m.name()));
    let tps: Vec<TypeParameter> = m
        .type_parameters()
        .map(|tps| tps.parameters().collect())
        .unwrap_or_default();
    if !tps.is_empty() {
        sig.push('<');
        for (i, tp) in tps.iter().enumerate() {
            if i > 0 {
                sig.push_str(", ");
            }
            sig.push_str(&token_name(tp.name()));
        }
        sig.push('>');
    }
    sig.push('(');
    let params: Vec<Parameter> = m
        .parameters()
        .map(|pl| pl.parameters().collect())
        .unwrap_or_default();
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            sig.push_str(", ");
        }
        sig.push_str(&token_name(p.name()));
        if let Some(ty) = p.ty() {
            sig.push_str(": ");
            sig.push_str(&format_type(&ty));
        }
    }
    sig.push(')');
    if let Some(ret) = m.return_type() {
        sig.push_str(": ");
        sig.push_str(&format_type(&ret));
    }
    StdlibMember {
        name: static_str(&token_name(m.name())),
        kind: MemberKind::Method,
        signature: static_str(&sig),
        doc: doc_or_empty(m.doc_comment().as_deref()),
    }
}

#[allow(dead_code)]
fn leak_method(m: &MethodDecl) -> StdlibMember {
    let mut sig = String::new();
    sig.push_str(&token_name(m.name()));
    let tps: Vec<TypeParameter> = m
        .type_parameters()
        .map(|tps| tps.parameters().collect())
        .unwrap_or_default();
    if !tps.is_empty() {
        sig.push('<');
        for (i, tp) in tps.iter().enumerate() {
            if i > 0 {
                sig.push_str(", ");
            }
            sig.push_str(&token_name(tp.name()));
        }
        sig.push('>');
    }
    sig.push('(');
    let params: Vec<Parameter> = m
        .parameters()
        .map(|pl| pl.parameters().collect())
        .unwrap_or_default();
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            sig.push_str(", ");
        }
        sig.push_str(&token_name(p.name()));
        if let Some(ty) = p.ty() {
            sig.push_str(": ");
            sig.push_str(&format_type(&ty));
        }
    }
    sig.push(')');
    if let Some(ret) = m.return_type() {
        sig.push_str(": ");
        sig.push_str(&format_type(&ret));
    }
    StdlibMember {
        name: static_str(&token_name(m.name())),
        kind: MemberKind::Method,
        signature: static_str(&sig),
        doc: doc_or_empty(m.doc_comment().as_deref()),
    }
}

fn token_name(t: Option<SyntaxToken>) -> String {
    t.map(|t| cst::ident_text(&t)).unwrap_or_default()
}

fn doc_or_empty(doc: Option<&str>) -> &'static str {
    match doc {
        Some(d) if !d.is_empty() => static_str(d),
        _ => "",
    }
}

fn static_str(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

// ----------------------------------------------------------------------
// Type formatting — duplicated from `pkl-analyze::pretty` to keep the
// scrape free of an analyzer dependency. Mirrors the same canonical
// shape so signatures match the hand-curated style.

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
        Type::Constrained(c) => {
            if let Some(inner) = c.inner() {
                write_type(out, &inner);
            }
            out.push_str("(...)");
        }
        Type::Default(d) => {
            out.push('*');
            if let Some(inner) = d.inner() {
                write_type(out, &inner);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrape_includes_string_class() {
        let parsed = parsed_base();
        let s = parsed
            .iter()
            .find(|t| t.name == "String")
            .expect("String present in parsed base");
        assert!(s.members.iter().any(|m| m.name == "length"));
        assert!(!s.doc.is_empty(), "String should carry its doc comment");
    }

    #[test]
    fn scrape_includes_collection_zip() {
        // `zip` is on the curated long-tail list — scrape should pick it
        // up from the upstream source.
        let parsed = parsed_base();
        let collection = parsed
            .iter()
            .find(|t| t.name == "Collection")
            .expect("Collection present");
        assert!(
            collection.members.iter().any(|m| m.name == "zip"),
            "Collection should expose zip from the scrape"
        );
    }
}

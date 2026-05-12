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

use pkl_syntax::ast::*;

use crate::{MemberKind, StdlibKind, StdlibMember, StdlibType};

/// Every type declared in the vendored `pkl.base` source. Lazily parsed
/// on first access and cached for the rest of the process.
pub fn parsed_base() -> &'static [&'static StdlibType] {
    static CELL: OnceLock<Vec<&'static StdlibType>> = OnceLock::new();
    CELL.get_or_init(|| {
        let module = pkl_syntax::parse(crate::vendored::find("base").unwrap().source).module;
        module
            .items
            .iter()
            .filter_map(class_decl)
            .map(|c| leak_type(c, "pkl.base"))
            .collect()
    })
}

fn class_decl(item: &Item) -> Option<&ClassDecl> {
    if let Item::Class(c) = item {
        Some(c)
    } else {
        None
    }
}

fn leak_type(c: &ClassDecl, module: &'static str) -> &'static StdlibType {
    let mut members: Vec<StdlibMember> = Vec::new();
    if let Some(body) = &c.body {
        for m in &body.members {
            match m {
                ClassMember::Property(p) => members.push(leak_property(p)),
                ClassMember::Method(m) => members.push(leak_method(m)),
            }
        }
    }
    let members_leaked: &'static [StdlibMember] = Box::leak(members.into_boxed_slice());
    let kind = if has_modifier(&c.modifiers, ModifierKind::Abstract) {
        StdlibKind::AbstractClass
    } else if has_modifier(&c.modifiers, ModifierKind::Open) {
        StdlibKind::OpenClass
    } else {
        StdlibKind::Class
    };
    let generics: Vec<&'static str> = c
        .type_parameters
        .iter()
        .map(|p| static_str(&p.name.name))
        .collect();
    let generics_leaked: &'static [&'static str] = Box::leak(generics.into_boxed_slice());
    let extends = c
        .extends
        .as_ref()
        .map(format_type)
        .as_deref()
        .map(static_str);
    let leaked = Box::new(StdlibType {
        name: static_str(&c.name.name),
        module,
        kind,
        generics: generics_leaked,
        extends,
        doc: doc_or_empty(c.doc_comment.as_deref()),
        members: members_leaked,
    });
    Box::leak(leaked)
}

fn leak_property(p: &PropertyDecl) -> StdlibMember {
    let ty =
        p.ty.as_ref()
            .map(format_type)
            .unwrap_or_else(|| "Any".to_string());
    let signature = static_str(&format!("{}: {}", p.name.name, ty));
    StdlibMember {
        name: static_str(&p.name.name),
        kind: MemberKind::Property,
        signature,
        doc: doc_or_empty(p.doc_comment.as_deref()),
    }
}

fn leak_method(m: &MethodDecl) -> StdlibMember {
    let mut sig = String::new();
    sig.push_str(&m.name.name);
    if !m.type_parameters.is_empty() {
        sig.push('<');
        for (i, tp) in m.type_parameters.iter().enumerate() {
            if i > 0 {
                sig.push_str(", ");
            }
            sig.push_str(&tp.name.name);
        }
        sig.push('>');
    }
    sig.push('(');
    for (i, p) in m.parameters.iter().enumerate() {
        if i > 0 {
            sig.push_str(", ");
        }
        sig.push_str(&p.name.name);
        if let Some(ty) = &p.ty {
            sig.push_str(": ");
            sig.push_str(&format_type(ty));
        }
    }
    sig.push(')');
    if let Some(ret) = &m.return_type {
        sig.push_str(": ");
        sig.push_str(&format_type(ret));
    }
    StdlibMember {
        name: static_str(&m.name.name),
        kind: MemberKind::Method,
        signature: static_str(&sig),
        doc: doc_or_empty(m.doc_comment.as_deref()),
    }
}

fn has_modifier(mods: &[Modifier], kind: ModifierKind) -> bool {
    mods.iter()
        .any(|m| std::mem::discriminant(&m.kind) == std::mem::discriminant(&kind))
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

//! Build hover content from resolved symbols.

use pkl_stdlib::MemberKind;

use crate::infer::MemberRef;
use crate::resolver::Resolution;
use crate::symbols::{Origin, Symbol};

/// Markdown hover string for a symbol.
///
/// The output looks like:
///
/// ```text
/// ```pkl
/// class Person extends Bar
/// ```
///
/// The user's class.
/// ```
pub fn hover_markdown(resolution: &Resolution, symbol: &Symbol) -> String {
    let mut out = String::new();
    let signature = symbol
        .signature
        .clone()
        .unwrap_or_else(|| default_signature(symbol));
    out.push_str("```pkl\n");
    out.push_str(&signature);
    out.push('\n');
    out.push_str("```");
    if let Some(container_id) = symbol.container {
        let container = resolution.symbols.get(container_id);
        out.push_str("\n\nin ");
        out.push_str(container.kind.describe());
        out.push_str(" `");
        out.push_str(&container.name);
        out.push('`');
    }
    if let Origin::Stdlib { module } = symbol.origin {
        out.push_str("\n\nfrom `");
        out.push_str(module);
        out.push('`');
    }
    if let Some(doc) = &symbol.doc {
        out.push_str("\n\n");
        out.push_str(doc);
    }
    out
}

fn default_signature(symbol: &Symbol) -> String {
    format!("{} {}", symbol.kind.describe(), symbol.name)
}

/// Markdown hover string for a resolved member access (`expr.name`).
///
/// Always includes the receiver type so the editor user can tell what the
/// dot-name landed on. If the stdlib catalogue knows the member we render
/// its full signature and doc; otherwise we try the resolution's symbol
/// table for a user-defined class member; failing both, we fall back to
/// "member of `Type`".
pub fn member_hover_markdown(member: &MemberRef, resolution: &Resolution) -> String {
    // Stdlib member: render the curated signature + module attribution.
    if let Some(m) = member.stdlib_member {
        let mut out = String::new();
        out.push_str("```pkl\n");
        let kind_kw = match m.kind {
            MemberKind::Property => "",
            MemberKind::Method => "function ",
        };
        out.push_str(kind_kw);
        out.push_str(m.signature);
        out.push_str("\n```");
        if let Some(t) = member.stdlib_type {
            out.push_str("\n\non `");
            out.push_str(t.name);
            out.push_str("` from `");
            out.push_str(t.module);
            out.push('`');
        }
        if !m.doc.is_empty() {
            out.push_str("\n\n");
            out.push_str(m.doc);
        }
        return out;
    }

    // User-defined member: delegate to the standard symbol renderer.
    if let Some(member_id) = member.user_member {
        let sym = resolution.symbol(member_id);
        return hover_markdown(resolution, sym);
    }

    // Unresolved.
    let mut out = String::new();
    out.push_str("```pkl\n");
    out.push_str(&member.member_name);
    out.push_str(": ?\n```\n\non `");
    out.push_str(&format!("{}", member.receiver_ty));
    out.push('`');
    out
}

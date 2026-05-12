//! A pragmatic subtype lattice for Pkl.
//!
//! The lattice is *conservative*: it returns `true` only when we're
//! reasonably confident `sub <: sup`. Any time the answer is unclear
//! (unknown receiver, unresolved class name, exotic type) it returns
//! `true` so the inferrer doesn't surface false-positive diagnostics.
//! Tightening this is future work that pairs with a real class graph.

use crate::types::Ty;
use crate::Resolution;

/// True if `sub` is a subtype of `sup` under our conservative lattice.
pub fn is_subtype(sub: &Ty, sup: &Ty, resolution: &Resolution) -> bool {
    // Anything goes when either side is fully unknown.
    if matches!(sub, Ty::Unknown) || matches!(sup, Ty::Unknown) {
        return true;
    }

    // Reflexivity.
    if sub == sup {
        return true;
    }

    // `Any` is top.
    if matches!(sup, Ty::Any) {
        return true;
    }

    // `Nothing` is bottom.
    if matches!(sub, Ty::Nothing) {
        return true;
    }

    // Nullable handling: `T <: T?`; `Null <: T?`; `T? <: T?` already covered.
    if let Ty::Nullable(inner_sup) = sup {
        if matches!(sub, Ty::Null) {
            return true;
        }
        return is_subtype(sub, inner_sup, resolution);
    }
    if let Ty::Nullable(_) = sub {
        // A nullable doesn't fit into a non-nullable.
        return false;
    }

    // Unions: covariant on the left, contravariant on the right.
    if let Ty::Union(members) = sub {
        return members.iter().all(|m| is_subtype(m, sup, resolution));
    }
    if let Ty::Union(members) = sup {
        return members.iter().any(|m| is_subtype(sub, m, resolution));
    }

    // Numeric hierarchy.
    match (sub, sup) {
        (Ty::Int, Ty::Number) | (Ty::Float, Ty::Number) => return true,
        _ => {}
    }

    // Generic collections: be covariant in element type (it's permissive
    // and matches user expectations for read-only Pkl values).
    match (sub, sup) {
        (Ty::List(a), Ty::List(b))
        | (Ty::Set(a), Ty::Set(b))
        | (Ty::Listing(a), Ty::Listing(b)) => return is_subtype(a, b, resolution),
        (Ty::Map(ak, av), Ty::Map(bk, bv)) | (Ty::Mapping(ak, av), Ty::Mapping(bk, bv)) => {
            return is_subtype(ak, bk, resolution) && is_subtype(av, bv, resolution);
        }
        (Ty::Pair(a1, b1), Ty::Pair(a2, b2)) => {
            return is_subtype(a1, a2, resolution) && is_subtype(b1, b2, resolution);
        }
        _ => {}
    }

    // Named types: if both name the same class, check args pairwise; if
    // the names differ, fall back to the user class's `extends` chain.
    if let (
        Ty::Named {
            name: ns,
            args: as_,
        },
        Ty::Named { name: nt, args: at },
    ) = (sub, sup)
    {
        if ns == nt {
            return as_
                .iter()
                .zip(at.iter())
                .all(|(a, b)| is_subtype(a, b, resolution));
        }
        // Walk user-class extends chain.
        return user_class_extends(resolution, ns, nt);
    }

    // Bare named-type vs primitive: e.g. user class `Foo extends Int`
    // (unusual but valid). We don't model that yet.
    false
}

/// Walk the user-class `extends` chain looking for `sup_name`. Returns
/// `true` when the chain reaches it, when `sub_name` isn't a known
/// user class (be permissive so we don't lie), or when we hit a cycle.
fn user_class_extends(resolution: &Resolution, sub_name: &str, sup_name: &str) -> bool {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut current = sub_name.to_string();
    while seen.insert(current.clone()) {
        if current == sup_name {
            return true;
        }
        let Some(sym) = resolution
            .symbols
            .iter()
            .find(|s| matches!(s.kind, crate::SymbolKind::Class) && s.name == current)
        else {
            // Unknown class — fall back to permissive so we don't emit
            // false-positive diagnostics.
            return true;
        };
        let Some(parent) = sym.parent_class.as_ref() else {
            // Reached a class with no parent and didn't find sup_name.
            return false;
        };
        current = parent.clone();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Resolution;

    fn empty_resolution() -> Resolution {
        // Build via the public entry point so we get the same shape as
        // the analyzer's real output (with the stdlib seeded).
        crate::resolve_module(&pkl_syntax::parse("").module)
    }

    #[test]
    fn numeric_promotion_holds() {
        let r = empty_resolution();
        assert!(is_subtype(&Ty::Int, &Ty::Number, &r));
        assert!(is_subtype(&Ty::Float, &Ty::Number, &r));
        assert!(!is_subtype(&Ty::Str, &Ty::Number, &r));
    }

    #[test]
    fn nullable_accepts_non_null() {
        let r = empty_resolution();
        assert!(is_subtype(&Ty::Int, &Ty::Nullable(Box::new(Ty::Int)), &r));
        assert!(is_subtype(&Ty::Null, &Ty::Nullable(Box::new(Ty::Str)), &r));
        assert!(!is_subtype(&Ty::Nullable(Box::new(Ty::Int)), &Ty::Int, &r));
    }

    #[test]
    fn covariant_list_subtype() {
        let r = empty_resolution();
        let int_list = Ty::List(Box::new(Ty::Int));
        let number_list = Ty::List(Box::new(Ty::Number));
        assert!(is_subtype(&int_list, &number_list, &r));
    }

    #[test]
    fn unknown_is_permissive() {
        let r = empty_resolution();
        assert!(is_subtype(&Ty::Unknown, &Ty::Int, &r));
        assert!(is_subtype(&Ty::Int, &Ty::Unknown, &r));
    }
}

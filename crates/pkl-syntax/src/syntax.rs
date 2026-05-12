//! `rowan` plumbing for the Pkl lossless syntax tree.
//!
//! `PklLang` ties our [`SyntaxKind`] enum to `rowan::Language`. Everything
//! downstream (`SyntaxNode`, `SyntaxToken`, `SyntaxElement`) is a thin
//! alias over rowan's parametric types so call sites stay readable.
//!
//! The green tree is produced by the parser via `rowan::GreenNodeBuilder`
//! and wrapped in red nodes by `SyntaxNode::new_root`. Both `SyntaxKind`
//! and `rowan::SyntaxKind` carry the same `u16` discriminant — the
//! `Language` impl just unwraps and validates it against the
//! `SyntaxKind::__Last` sentinel.

use crate::kind::SyntaxKind;

/// The pkl flavour of `rowan::Language`. Zero-sized; only the impl
/// matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PklLang {}

impl rowan::Language for PklLang {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> Self::Kind {
        SyntaxKind::from_raw(raw.0).expect("kind_from_raw: out-of-range SyntaxKind discriminant")
    }

    fn kind_to_raw(kind: Self::Kind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

/// A red node in the lossless syntax tree. Every node carries a kind,
/// a `TextRange`, parent / sibling / child pointers, and the underlying
/// green (immutable) tree.
pub type SyntaxNode = rowan::SyntaxNode<PklLang>;

/// A leaf in the syntax tree — either a "significant" token (keyword,
/// identifier, literal, punctuation) or trivia (whitespace, comment).
pub type SyntaxToken = rowan::SyntaxToken<PklLang>;

/// Either a node or a token. Convenient when walking children regardless
/// of whether trivia or sub-tree is next.
pub type SyntaxElement = rowan::SyntaxElement<PklLang>;

/// Same as `rowan::NodeOrToken<&SyntaxNode, &SyntaxToken>` but spelled
/// in our crate's vocabulary. Used by visitors that want to borrow.
pub type SyntaxElementRef<'a> = rowan::NodeOrToken<&'a SyntaxNode, &'a SyntaxToken>;

#[cfg(test)]
mod tests {
    use super::*;
    use rowan::{GreenNode, GreenNodeBuilder, Language};

    fn build_trivial() -> GreenNode {
        let mut b = GreenNodeBuilder::new();
        b.start_node(PklLang::kind_to_raw(SyntaxKind::Module));
        b.token(PklLang::kind_to_raw(SyntaxKind::Ident), "hello");
        b.finish_node();
        b.finish()
    }

    #[test]
    fn round_trips_through_language_trait() {
        let green = build_trivial();
        let root = SyntaxNode::new_root(green);
        assert_eq!(root.kind(), SyntaxKind::Module);
        let token = root.first_token().unwrap();
        assert_eq!(token.kind(), SyntaxKind::Ident);
        assert_eq!(token.text(), "hello");
        // The tree must reproduce its source verbatim.
        assert_eq!(root.text().to_string(), "hello");
    }

    #[test]
    fn from_raw_rejects_out_of_range() {
        assert!(SyntaxKind::from_raw(SyntaxKind::__Last as u16).is_none());
        assert!(SyntaxKind::from_raw(u16::MAX).is_none());
        assert!(SyntaxKind::from_raw(0).is_some());
    }
}

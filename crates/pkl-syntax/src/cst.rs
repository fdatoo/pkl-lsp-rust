//! Typed wrappers (the "AST view") over the lossless syntax tree.
//!
//! Each wrapper is a newtype around [`SyntaxNode`] with accessor methods
//! that walk the rowan tree on demand. None of the wrappers copy or own
//! grammar-level data — they are zero-cost views.
//!
//! Rather than carrying `String`/`Vec` field values directly, each
//! accessor returns either an `Option<TypedChild>`, an iterator of
//! typed children, or a `SyntaxToken` for leaf positions. Callers that
//! need span identity to match the analyzer's offset-keyed indices use
//! [`significant_span`] (skips leading/trailing trivia); callers that
//! need exact source coverage use [`AstNode::span`].
//!
//! See [`AstNode`] for the cast trait shared by every wrapper.

use rowan::TextRange;

use crate::kind::SyntaxKind;
use crate::span::Span;
use crate::syntax::{SyntaxNode, SyntaxToken};

/// A typed view over a [`SyntaxNode`]. Mirrors `rowan::ast::AstNode` but
/// is spelled out so call sites don't need to depend on rowan's `ast`
/// feature directly.
pub trait AstNode: Sized {
    fn can_cast(kind: SyntaxKind) -> bool;
    fn cast(syntax: SyntaxNode) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode;

    /// Source byte range of this node, including any leading or trailing
    /// trivia that falls within the node. For analyzer-style span
    /// identities (which need to match what the legacy hand-rolled
    /// parser produced), prefer [`significant_span`].
    fn span(&self) -> Span {
        text_range_to_span(self.syntax().text_range())
    }

    /// Verbatim source text covered by this node, including trivia.
    fn text(&self) -> String {
        self.syntax().text().to_string()
    }
}

fn text_range_to_span(range: TextRange) -> Span {
    Span::new(range.start().into(), range.end().into())
}

/// Span of `node` excluding leading and trailing trivia.
///
/// Mirrors the legacy hand-rolled parser's behavior: spans were computed
/// from significant-token bounds, but the lossless tree's `text_range`
/// includes leading trivia that fell inside the node. The analyzer's
/// offset-keyed lookups (e.g. `Inference::expr_types`) are keyed by
/// these significant spans, so callers that walk the CST must use this
/// helper to keep span identities matching across passes.
pub fn significant_span(node: &SyntaxNode) -> Span {
    let r = node.text_range();
    let start = node
        .descendants_with_tokens()
        .find_map(|el| match el {
            rowan::NodeOrToken::Token(t) if !t.kind().is_trivia() => Some(t.text_range().start()),
            _ => None,
        })
        .unwrap_or(r.start());
    let mut end = r.end();
    let mut last: Option<rowan::TextSize> = None;
    for el in node.descendants_with_tokens() {
        if let rowan::NodeOrToken::Token(t) = el {
            if !t.kind().is_trivia() {
                last = Some(t.text_range().end());
            }
        }
    }
    if let Some(e) = last {
        end = e;
    }
    Span::new(start.into(), end.into())
}

/// Span of a single token (always trivia-free since tokens are atomic).
pub fn token_span(token: &SyntaxToken) -> Span {
    let r = token.text_range();
    Span::new(r.start().into(), r.end().into())
}

// ----------------------------------------------------------------------
// Internal: macros to reduce wrapper boilerplate.

macro_rules! ast_node {
    ($name:ident, $kind:path) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub struct $name(pub(crate) SyntaxNode);

        impl AstNode for $name {
            #[inline]
            fn can_cast(kind: SyntaxKind) -> bool {
                kind == $kind
            }
            fn cast(syntax: SyntaxNode) -> Option<Self> {
                if syntax.kind() == $kind {
                    Some(Self(syntax))
                } else {
                    None
                }
            }
            #[inline]
            fn syntax(&self) -> &SyntaxNode {
                &self.0
            }
        }
    };
}

// ----------------------------------------------------------------------
// Helpers shared across wrappers.

/// Find the first direct child node castable to `T`.
fn child_node<T: AstNode>(parent: &SyntaxNode) -> Option<T> {
    parent.children().find_map(T::cast)
}

/// Iterate over direct child nodes castable to `T`.
fn children<T: AstNode + 'static>(parent: &SyntaxNode) -> impl Iterator<Item = T> + '_ {
    parent.children().filter_map(T::cast)
}

/// First direct *child token* whose kind matches `kind` (skipping trivia
/// and skipping anything nested inside child *nodes*).
fn child_token(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    parent
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == kind)
}

/// First direct child token whose kind is any of the given alternatives.
fn child_token_of(parent: &SyntaxNode, kinds: &[SyntaxKind]) -> Option<SyntaxToken> {
    parent
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| kinds.contains(&t.kind()))
}

/// Walk descendants in source order, yielding trivia tokens that
/// precede the first significant token. This descends through wrapper
/// nodes (`ModifierList`, `AnnotationList`, etc.) so that trivia
/// emitted inside the first modifier — which is where the doc comment
/// lands when a declaration has modifiers — is still treated as
/// leading trivia of the whole declaration.
fn leading_trivia(node: &SyntaxNode) -> impl Iterator<Item = SyntaxToken> + '_ {
    node.descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .take_while(|t| t.kind().is_trivia())
}

/// Pull the doc-comment text out of the leading trivia of `node`.
///
/// Mirrors the legacy parser's recovery: only `///`-style comments
/// immediately preceding the node attach; any non-trivia descendant
/// token or any other comment kind resets the accumulator. Trivia
/// inside nested first-children (e.g. the trivia that lands inside a
/// `Modifier` token sequence) is still considered leading for the
/// outer declaration.
pub fn doc_comment_for(node: &SyntaxNode) -> Option<String> {
    let mut out: Option<String> = None;
    for tok in leading_trivia(node) {
        match tok.kind() {
            SyntaxKind::DocComment => {
                let stripped = tok.text().trim_start_matches('/').trim_start();
                let buf = out.get_or_insert_with(String::new);
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(stripped);
            }
            SyntaxKind::Whitespace | SyntaxKind::Newline => {}
            _ => {
                // Any other comment kind breaks adjacency.
                out = None;
            }
        }
    }
    out
}

// ----------------------------------------------------------------------
// Module / header / imports

ast_node!(Module, SyntaxKind::Module);

impl Module {
    pub fn header(&self) -> Option<ModuleHeader> {
        child_node(&self.0)
    }

    pub fn imports(&self) -> impl Iterator<Item = ImportClause> + '_ {
        children(&self.0)
    }

    pub fn items(&self) -> impl Iterator<Item = Item> + '_ {
        self.0.children().filter_map(Item::cast)
    }
}

ast_node!(ModuleHeader, SyntaxKind::ModuleHeader);

impl ModuleHeader {
    pub fn doc_comment(&self) -> Option<String> {
        doc_comment_for(&self.0)
    }

    pub fn annotations(&self) -> impl Iterator<Item = Annotation> + '_ {
        annotation_list(&self.0)
    }

    pub fn modifiers(&self) -> impl Iterator<Item = Modifier> + '_ {
        modifier_list(&self.0)
    }

    pub fn module_kw(&self) -> Option<SyntaxToken> {
        child_token(&self.0, SyntaxKind::ModuleKw)
    }

    pub fn name(&self) -> Option<QualifiedName> {
        child_node(&self.0)
    }

    pub fn clause(&self) -> Option<ExtendsAmendsClause> {
        child_node(&self.0)
    }
}

ast_node!(QualifiedName, SyntaxKind::QualifiedName);

impl QualifiedName {
    pub fn segments(&self) -> impl Iterator<Item = SyntaxToken> + '_ {
        self.0
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::QuotedIdent))
    }

    /// `"foo.bar.baz"`-style joined name.
    pub fn text_joined(&self) -> String {
        let mut out = String::new();
        for (i, seg) in self.segments().enumerate() {
            if i > 0 {
                out.push('.');
            }
            out.push_str(&ident_text(&seg));
        }
        out
    }
}

ast_node!(ExtendsAmendsClause, SyntaxKind::ExtendsAmendsClause);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClauseKind {
    Extends,
    Amends,
}

impl ExtendsAmendsClause {
    pub fn kind(&self) -> ClauseKind {
        if child_token(&self.0, SyntaxKind::ExtendsKw).is_some() {
            ClauseKind::Extends
        } else {
            ClauseKind::Amends
        }
    }

    pub fn target(&self) -> Option<SyntaxToken> {
        child_token_of(&self.0, &[SyntaxKind::String, SyntaxKind::MultilineString])
    }
}

ast_node!(ImportClause, SyntaxKind::ImportClause);

impl ImportClause {
    pub fn is_glob(&self) -> bool {
        child_token(&self.0, SyntaxKind::ImportGlobKw).is_some()
    }

    pub fn path(&self) -> Option<SyntaxToken> {
        child_token_of(&self.0, &[SyntaxKind::String, SyntaxKind::MultilineString])
    }

    pub fn alias(&self) -> Option<SyntaxToken> {
        // The alias is the identifier following the `as` keyword.
        let mut saw_as = false;
        for el in self.0.children_with_tokens() {
            if let Some(t) = el.as_token() {
                if t.kind() == SyntaxKind::AsKw {
                    saw_as = true;
                } else if saw_as && matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::QuotedIdent)
                {
                    return Some(t.clone());
                }
            }
        }
        None
    }
}

// ----------------------------------------------------------------------
// Annotations / modifiers

ast_node!(AnnotationList, SyntaxKind::AnnotationList);
ast_node!(Annotation, SyntaxKind::Annotation);

impl Annotation {
    pub fn name(&self) -> Option<QualifiedName> {
        child_node(&self.0)
    }

    pub fn body(&self) -> Option<ObjectBody> {
        child_node(&self.0)
    }
}

ast_node!(ModifierList, SyntaxKind::ModifierList);
ast_node!(Modifier, SyntaxKind::Modifier);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ModifierKind {
    Abstract,
    Open,
    Local,
    Hidden,
    Fixed,
    External,
}

impl Modifier {
    pub fn kind(&self) -> Option<ModifierKind> {
        for el in self.0.children_with_tokens() {
            if let Some(t) = el.as_token() {
                let k = match t.kind() {
                    SyntaxKind::AbstractKw => ModifierKind::Abstract,
                    SyntaxKind::OpenKw => ModifierKind::Open,
                    SyntaxKind::LocalKw => ModifierKind::Local,
                    SyntaxKind::HiddenKw => ModifierKind::Hidden,
                    SyntaxKind::FixedKw => ModifierKind::Fixed,
                    SyntaxKind::ExternalKw => ModifierKind::External,
                    _ => continue,
                };
                return Some(k);
            }
        }
        None
    }
}

fn annotation_list(parent: &SyntaxNode) -> impl Iterator<Item = Annotation> + '_ {
    parent
        .children()
        .find(|n| n.kind() == SyntaxKind::AnnotationList)
        .into_iter()
        .flat_map(|n| {
            n.children()
                .filter_map(Annotation::cast)
                .collect::<Vec<_>>()
        })
}

fn modifier_list(parent: &SyntaxNode) -> impl Iterator<Item = Modifier> + '_ {
    parent
        .children()
        .find(|n| n.kind() == SyntaxKind::ModifierList)
        .into_iter()
        .flat_map(|n| n.children().filter_map(Modifier::cast).collect::<Vec<_>>())
}

// ----------------------------------------------------------------------
// Items (top-level declarations)

#[derive(Clone, Debug)]
pub enum Item {
    Class(ClassDecl),
    TypeAlias(TypeAliasDecl),
    Property(PropertyDecl),
    Method(MethodDecl),
    Error(ErrorNode),
}

impl AstNode for Item {
    fn can_cast(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            SyntaxKind::ClassDecl
                | SyntaxKind::TypeAliasDecl
                | SyntaxKind::PropertyDecl
                | SyntaxKind::MethodDecl
                | SyntaxKind::ErrorNode
        )
    }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        Some(match syntax.kind() {
            SyntaxKind::ClassDecl => Item::Class(ClassDecl(syntax)),
            SyntaxKind::TypeAliasDecl => Item::TypeAlias(TypeAliasDecl(syntax)),
            SyntaxKind::PropertyDecl => Item::Property(PropertyDecl(syntax)),
            SyntaxKind::MethodDecl => Item::Method(MethodDecl(syntax)),
            SyntaxKind::ErrorNode => Item::Error(ErrorNode(syntax)),
            _ => return None,
        })
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            Item::Class(c) => c.syntax(),
            Item::TypeAlias(t) => t.syntax(),
            Item::Property(p) => p.syntax(),
            Item::Method(m) => m.syntax(),
            Item::Error(e) => e.syntax(),
        }
    }
}

impl Item {
    pub fn name_token(&self) -> Option<SyntaxToken> {
        match self {
            Item::Class(c) => c.name(),
            Item::TypeAlias(t) => t.name(),
            Item::Property(p) => p.name(),
            Item::Method(m) => m.name(),
            Item::Error(_) => None,
        }
    }

    pub fn name(&self) -> Option<String> {
        self.name_token().map(|t| ident_text(&t))
    }
}

ast_node!(ErrorNode, SyntaxKind::ErrorNode);

ast_node!(ClassDecl, SyntaxKind::ClassDecl);

impl ClassDecl {
    pub fn doc_comment(&self) -> Option<String> {
        doc_comment_for(&self.0)
    }

    pub fn annotations(&self) -> impl Iterator<Item = Annotation> + '_ {
        annotation_list(&self.0)
    }

    pub fn modifiers(&self) -> impl Iterator<Item = Modifier> + '_ {
        modifier_list(&self.0)
    }

    pub fn class_kw(&self) -> Option<SyntaxToken> {
        child_token(&self.0, SyntaxKind::ClassKw)
    }

    pub fn name(&self) -> Option<SyntaxToken> {
        child_token_of(&self.0, &[SyntaxKind::Ident, SyntaxKind::QuotedIdent])
    }

    pub fn type_parameters(&self) -> Option<TypeParameterList> {
        child_node(&self.0)
    }

    pub fn extends(&self) -> Option<Type> {
        // The `extends` clause's type is the first Type-shaped node
        // that follows the `extends` keyword.
        let mut saw_extends = false;
        for el in self.0.children_with_tokens() {
            match el {
                rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::ExtendsKw => {
                    saw_extends = true;
                }
                rowan::NodeOrToken::Node(n) if saw_extends => {
                    if let Some(ty) = Type::cast(n) {
                        return Some(ty);
                    }
                }
                _ => {}
            }
        }
        None
    }

    pub fn body(&self) -> Option<ClassBody> {
        child_node(&self.0)
    }
}

ast_node!(ClassBody, SyntaxKind::ClassBody);

impl ClassBody {
    pub fn members(&self) -> impl Iterator<Item = ClassMember> + '_ {
        self.0.children().filter_map(ClassMember::cast)
    }
}

#[derive(Clone, Debug)]
pub enum ClassMember {
    Property(ClassPropertyDecl),
    Method(ClassMethodDecl),
}

impl AstNode for ClassMember {
    fn can_cast(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            SyntaxKind::ClassPropertyDecl | SyntaxKind::ClassMethodDecl
        )
    }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        Some(match syntax.kind() {
            SyntaxKind::ClassPropertyDecl => ClassMember::Property(ClassPropertyDecl(syntax)),
            SyntaxKind::ClassMethodDecl => ClassMember::Method(ClassMethodDecl(syntax)),
            _ => return None,
        })
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            ClassMember::Property(p) => p.syntax(),
            ClassMember::Method(m) => m.syntax(),
        }
    }
}

ast_node!(ClassPropertyDecl, SyntaxKind::ClassPropertyDecl);
ast_node!(ClassMethodDecl, SyntaxKind::ClassMethodDecl);

// Property/Method accessors are shared across the four "decl" flavours
// (PropertyDecl/MethodDecl at the top level, ClassPropertyDecl /
// ClassMethodDecl inside classes, and ObjectProperty/ObjectMethod inside
// object bodies). The accessor implementations live below.

ast_node!(TypeAliasDecl, SyntaxKind::TypeAliasDecl);

impl TypeAliasDecl {
    pub fn doc_comment(&self) -> Option<String> {
        doc_comment_for(&self.0)
    }
    pub fn annotations(&self) -> impl Iterator<Item = Annotation> + '_ {
        annotation_list(&self.0)
    }
    pub fn modifiers(&self) -> impl Iterator<Item = Modifier> + '_ {
        modifier_list(&self.0)
    }
    pub fn name(&self) -> Option<SyntaxToken> {
        child_token_of(&self.0, &[SyntaxKind::Ident, SyntaxKind::QuotedIdent])
    }
    pub fn type_parameters(&self) -> Option<TypeParameterList> {
        child_node(&self.0)
    }
    pub fn aliased_type(&self) -> Option<Type> {
        child_node(&self.0)
    }
}

ast_node!(PropertyDecl, SyntaxKind::PropertyDecl);
ast_node!(MethodDecl, SyntaxKind::MethodDecl);
ast_node!(ObjectProperty, SyntaxKind::ObjectProperty);
ast_node!(ObjectMethod, SyntaxKind::ObjectMethod);

macro_rules! impl_property_accessors {
    ($($t:ty),* $(,)?) => { $(
        impl $t {
            pub fn doc_comment(&self) -> Option<String> { doc_comment_for(&self.0) }
            pub fn annotations(&self) -> impl Iterator<Item = Annotation> + '_ {
                annotation_list(&self.0)
            }
            pub fn modifiers(&self) -> impl Iterator<Item = Modifier> + '_ {
                modifier_list(&self.0)
            }
            pub fn name(&self) -> Option<SyntaxToken> {
                child_token_of(&self.0, &[SyntaxKind::Ident, SyntaxKind::QuotedIdent])
            }
            pub fn ty(&self) -> Option<Type> {
                // The type follows a `:` direct-child token.
                let mut saw_colon = false;
                for el in self.0.children_with_tokens() {
                    match el {
                        rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::Colon => {
                            saw_colon = true;
                        }
                        rowan::NodeOrToken::Node(n) if saw_colon => {
                            if let Some(t) = Type::cast(n) { return Some(t); }
                        }
                        _ => {}
                    }
                }
                None
            }
            pub fn value(&self) -> Option<PropertyValue> {
                // Either `= expr` or `{ object body }`.
                let mut saw_eq = false;
                for el in self.0.children_with_tokens() {
                    match el {
                        rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::Eq => {
                            saw_eq = true;
                        }
                        rowan::NodeOrToken::Node(n) => {
                            if saw_eq {
                                if let Some(e) = Expr::cast(n.clone()) {
                                    return Some(PropertyValue::Expr(e));
                                }
                            }
                            if n.kind() == SyntaxKind::ObjectBody {
                                return ObjectBody::cast(n).map(PropertyValue::ObjectBody);
                            }
                        }
                        _ => {}
                    }
                }
                None
            }
        }
    )* };
}

impl_property_accessors!(PropertyDecl, ClassPropertyDecl, ObjectProperty);

#[derive(Clone, Debug)]
pub enum PropertyValue {
    Expr(Expr),
    ObjectBody(ObjectBody),
}

impl PropertyValue {
    pub fn span(&self) -> Span {
        match self {
            PropertyValue::Expr(e) => e.span(),
            PropertyValue::ObjectBody(b) => b.span(),
        }
    }
}

macro_rules! impl_method_accessors {
    ($($t:ty),* $(,)?) => { $(
        impl $t {
            pub fn doc_comment(&self) -> Option<String> { doc_comment_for(&self.0) }
            pub fn annotations(&self) -> impl Iterator<Item = Annotation> + '_ {
                annotation_list(&self.0)
            }
            pub fn modifiers(&self) -> impl Iterator<Item = Modifier> + '_ {
                modifier_list(&self.0)
            }
            pub fn function_kw(&self) -> Option<SyntaxToken> {
                child_token(&self.0, SyntaxKind::FunctionKw)
            }
            pub fn name(&self) -> Option<SyntaxToken> {
                child_token_of(&self.0, &[SyntaxKind::Ident, SyntaxKind::QuotedIdent])
            }
            pub fn type_parameters(&self) -> Option<TypeParameterList> {
                child_node(&self.0)
            }
            pub fn parameters(&self) -> Option<ParameterList> {
                child_node(&self.0)
            }
            pub fn return_type(&self) -> Option<Type> {
                let mut saw_colon = false;
                for el in self.0.children_with_tokens() {
                    match el {
                        rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::Colon => {
                            saw_colon = true;
                        }
                        rowan::NodeOrToken::Node(n) if saw_colon => {
                            if let Some(t) = Type::cast(n) { return Some(t); }
                        }
                        _ => {}
                    }
                }
                None
            }
            pub fn body(&self) -> Option<Expr> {
                let mut saw_eq = false;
                for el in self.0.children_with_tokens() {
                    match el {
                        rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::Eq => {
                            saw_eq = true;
                        }
                        rowan::NodeOrToken::Node(n) if saw_eq => {
                            if let Some(e) = Expr::cast(n) { return Some(e); }
                        }
                        _ => {}
                    }
                }
                None
            }
        }
    )* };
}

impl_method_accessors!(MethodDecl, ClassMethodDecl, ObjectMethod);

// ----------------------------------------------------------------------
// Parameters

ast_node!(ParameterList, SyntaxKind::ParameterList);

impl ParameterList {
    pub fn parameters(&self) -> impl Iterator<Item = Parameter> + '_ {
        children(&self.0)
    }
}

ast_node!(Parameter, SyntaxKind::Parameter);

impl Parameter {
    pub fn name(&self) -> Option<SyntaxToken> {
        child_token_of(&self.0, &[SyntaxKind::Ident, SyntaxKind::QuotedIdent])
    }

    pub fn ty(&self) -> Option<Type> {
        let mut saw_colon = false;
        for el in self.0.children_with_tokens() {
            match el {
                rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::Colon => {
                    saw_colon = true;
                }
                rowan::NodeOrToken::Node(n) if saw_colon => {
                    if let Some(t) = Type::cast(n) {
                        return Some(t);
                    }
                }
                _ => {}
            }
        }
        None
    }
}

ast_node!(TypeParameterList, SyntaxKind::TypeParameterList);

impl TypeParameterList {
    pub fn parameters(&self) -> impl Iterator<Item = TypeParameter> + '_ {
        children(&self.0)
    }
}

ast_node!(TypeParameter, SyntaxKind::TypeParameter);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Variance {
    In,
    Out,
}

impl TypeParameter {
    pub fn variance(&self) -> Option<Variance> {
        for el in self.0.children_with_tokens() {
            if let Some(t) = el.as_token() {
                return Some(match t.kind() {
                    SyntaxKind::InKw => Variance::In,
                    SyntaxKind::OutKw => Variance::Out,
                    _ => continue,
                });
            }
        }
        None
    }

    pub fn name(&self) -> Option<SyntaxToken> {
        child_token_of(&self.0, &[SyntaxKind::Ident, SyntaxKind::QuotedIdent])
    }
}

// ----------------------------------------------------------------------
// Types

#[derive(Clone, Debug)]
pub enum Type {
    Named(NamedType),
    Nullable(NullableType),
    Union(UnionType),
    Function(FunctionType),
    Parenthesized(ParenthesizedType),
    StringLiteral(StringLiteralType),
    Constrained(ConstrainedType),
    Default(DefaultType),
    Module(ModuleType),
    Unknown(UnknownType),
    Nothing(NothingType),
    Error(ErrorNode),
}

impl AstNode for Type {
    fn can_cast(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            SyntaxKind::TypeRef
                | SyntaxKind::TypeNullable
                | SyntaxKind::TypeUnion
                | SyntaxKind::TypeFunction
                | SyntaxKind::TypeParenthesized
                | SyntaxKind::TypeStringLiteral
                | SyntaxKind::TypeConstrained
                | SyntaxKind::TypeDefault
                | SyntaxKind::TypeModule
                | SyntaxKind::TypeUnknown
                | SyntaxKind::TypeNothing
                | SyntaxKind::ErrorNode
        )
    }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        Some(match syntax.kind() {
            SyntaxKind::TypeRef => Type::Named(NamedType(syntax)),
            SyntaxKind::TypeNullable => Type::Nullable(NullableType(syntax)),
            SyntaxKind::TypeUnion => Type::Union(UnionType(syntax)),
            SyntaxKind::TypeFunction => Type::Function(FunctionType(syntax)),
            SyntaxKind::TypeParenthesized => Type::Parenthesized(ParenthesizedType(syntax)),
            SyntaxKind::TypeStringLiteral => Type::StringLiteral(StringLiteralType(syntax)),
            SyntaxKind::TypeConstrained => Type::Constrained(ConstrainedType(syntax)),
            SyntaxKind::TypeDefault => Type::Default(DefaultType(syntax)),
            SyntaxKind::TypeModule => Type::Module(ModuleType(syntax)),
            SyntaxKind::TypeUnknown => Type::Unknown(UnknownType(syntax)),
            SyntaxKind::TypeNothing => Type::Nothing(NothingType(syntax)),
            SyntaxKind::ErrorNode => Type::Error(ErrorNode(syntax)),
            _ => return None,
        })
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            Type::Named(t) => t.syntax(),
            Type::Nullable(t) => t.syntax(),
            Type::Union(t) => t.syntax(),
            Type::Function(t) => t.syntax(),
            Type::Parenthesized(t) => t.syntax(),
            Type::StringLiteral(t) => t.syntax(),
            Type::Constrained(t) => t.syntax(),
            Type::Default(t) => t.syntax(),
            Type::Module(t) => t.syntax(),
            Type::Unknown(t) => t.syntax(),
            Type::Nothing(t) => t.syntax(),
            Type::Error(t) => t.syntax(),
        }
    }
}

ast_node!(NamedType, SyntaxKind::TypeRef);

impl NamedType {
    pub fn name(&self) -> Option<QualifiedName> {
        child_node(&self.0)
    }
    pub fn type_arguments(&self) -> Option<TypeArgumentList> {
        child_node(&self.0)
    }
}

ast_node!(NullableType, SyntaxKind::TypeNullable);

impl NullableType {
    pub fn inner(&self) -> Option<Type> {
        child_node(&self.0)
    }
}

ast_node!(UnionType, SyntaxKind::TypeUnion);

impl UnionType {
    pub fn members(&self) -> impl Iterator<Item = Type> + '_ {
        self.0.children().filter_map(Type::cast)
    }
}

ast_node!(FunctionType, SyntaxKind::TypeFunction);

impl FunctionType {
    pub fn parameters(&self) -> impl Iterator<Item = Type> + '_ {
        // All Type children except the last (which is the result).
        let children: Vec<Type> = self.0.children().filter_map(Type::cast).collect();
        let take = children.len().saturating_sub(1);
        children.into_iter().take(take)
    }

    pub fn result(&self) -> Option<Type> {
        self.0.children().filter_map(Type::cast).last()
    }
}

ast_node!(ParenthesizedType, SyntaxKind::TypeParenthesized);

impl ParenthesizedType {
    pub fn inner(&self) -> Option<Type> {
        child_node(&self.0)
    }
}

ast_node!(StringLiteralType, SyntaxKind::TypeStringLiteral);

impl StringLiteralType {
    pub fn token(&self) -> Option<SyntaxToken> {
        child_token_of(&self.0, &[SyntaxKind::String, SyntaxKind::MultilineString])
    }
}

ast_node!(ConstrainedType, SyntaxKind::TypeConstrained);

impl ConstrainedType {
    pub fn inner(&self) -> Option<Type> {
        child_node(&self.0)
    }

    pub fn constraints(&self) -> Vec<Expr> {
        self.0
            .children()
            .find_map(ArgList::cast)
            .map(|args| args.args().collect())
            .unwrap_or_default()
    }
}

ast_node!(DefaultType, SyntaxKind::TypeDefault);

impl DefaultType {
    pub fn inner(&self) -> Option<Type> {
        child_node(&self.0)
    }
}

ast_node!(ModuleType, SyntaxKind::TypeModule);
ast_node!(UnknownType, SyntaxKind::TypeUnknown);
ast_node!(NothingType, SyntaxKind::TypeNothing);

ast_node!(TypeArgumentList, SyntaxKind::TypeArgumentList);

impl TypeArgumentList {
    pub fn arguments(&self) -> impl Iterator<Item = Type> + '_ {
        self.0.children().filter_map(Type::cast)
    }
}

// ----------------------------------------------------------------------
// Expressions

#[derive(Clone, Debug)]
pub enum Expr {
    Literal(LiteralExpr),
    InterpolatedString(InterpolatedString),
    Ident(IdentExpr),
    Paren(ParenExpr),
    Unary(UnaryExpr),
    Binary(BinaryExpr),
    NullCoalesce(NullCoalesceExpr),
    TypeCheck(TypeCheckExpr),
    TypeCast(TypeCastExpr),
    If(IfExpr),
    Let(LetExpr),
    Lambda(LambdaExpr),
    Call(CallExpr),
    Index(IndexExpr),
    Member(MemberExpr),
    NonNull(NonNullExpr),
    New(NewExpr),
    Amends(AmendsExpr),
    Throw(ThrowExpr),
    Trace(TraceExpr),
    Read(ReadExpr),
    Import(ImportExpr),
    Error(ErrorNode),
}

impl AstNode for Expr {
    fn can_cast(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            SyntaxKind::LiteralExpr
                | SyntaxKind::InterpolatedString
                | SyntaxKind::InterpolatedMultilineString
                | SyntaxKind::IdentExpr
                | SyntaxKind::ParenExpr
                | SyntaxKind::UnaryExpr
                | SyntaxKind::BinaryExpr
                | SyntaxKind::NullCoalesceExpr
                | SyntaxKind::TypeCheckExpr
                | SyntaxKind::TypeCastExpr
                | SyntaxKind::IfExpr
                | SyntaxKind::LetExpr
                | SyntaxKind::LambdaExpr
                | SyntaxKind::CallExpr
                | SyntaxKind::IndexExpr
                | SyntaxKind::MemberExpr
                | SyntaxKind::NonNullExpr
                | SyntaxKind::NewExpr
                | SyntaxKind::AmendsExpr
                | SyntaxKind::ThrowExpr
                | SyntaxKind::TraceExpr
                | SyntaxKind::ReadExpr
                | SyntaxKind::ImportExpr
                | SyntaxKind::ErrorNode
        )
    }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        Some(match syntax.kind() {
            SyntaxKind::LiteralExpr => Expr::Literal(LiteralExpr(syntax)),
            SyntaxKind::InterpolatedString | SyntaxKind::InterpolatedMultilineString => {
                Expr::InterpolatedString(InterpolatedString(syntax))
            }
            SyntaxKind::IdentExpr => Expr::Ident(IdentExpr(syntax)),
            SyntaxKind::ParenExpr => Expr::Paren(ParenExpr(syntax)),
            SyntaxKind::UnaryExpr => Expr::Unary(UnaryExpr(syntax)),
            SyntaxKind::BinaryExpr => Expr::Binary(BinaryExpr(syntax)),
            SyntaxKind::NullCoalesceExpr => Expr::NullCoalesce(NullCoalesceExpr(syntax)),
            SyntaxKind::TypeCheckExpr => Expr::TypeCheck(TypeCheckExpr(syntax)),
            SyntaxKind::TypeCastExpr => Expr::TypeCast(TypeCastExpr(syntax)),
            SyntaxKind::IfExpr => Expr::If(IfExpr(syntax)),
            SyntaxKind::LetExpr => Expr::Let(LetExpr(syntax)),
            SyntaxKind::LambdaExpr => Expr::Lambda(LambdaExpr(syntax)),
            SyntaxKind::CallExpr => Expr::Call(CallExpr(syntax)),
            SyntaxKind::IndexExpr => Expr::Index(IndexExpr(syntax)),
            SyntaxKind::MemberExpr => Expr::Member(MemberExpr(syntax)),
            SyntaxKind::NonNullExpr => Expr::NonNull(NonNullExpr(syntax)),
            SyntaxKind::NewExpr => Expr::New(NewExpr(syntax)),
            SyntaxKind::AmendsExpr => Expr::Amends(AmendsExpr(syntax)),
            SyntaxKind::ThrowExpr => Expr::Throw(ThrowExpr(syntax)),
            SyntaxKind::TraceExpr => Expr::Trace(TraceExpr(syntax)),
            SyntaxKind::ReadExpr => Expr::Read(ReadExpr(syntax)),
            SyntaxKind::ImportExpr => Expr::Import(ImportExpr(syntax)),
            SyntaxKind::ErrorNode => Expr::Error(ErrorNode(syntax)),
            _ => return None,
        })
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            Expr::Literal(e) => e.syntax(),
            Expr::InterpolatedString(e) => e.syntax(),
            Expr::Ident(e) => e.syntax(),
            Expr::Paren(e) => e.syntax(),
            Expr::Unary(e) => e.syntax(),
            Expr::Binary(e) => e.syntax(),
            Expr::NullCoalesce(e) => e.syntax(),
            Expr::TypeCheck(e) => e.syntax(),
            Expr::TypeCast(e) => e.syntax(),
            Expr::If(e) => e.syntax(),
            Expr::Let(e) => e.syntax(),
            Expr::Lambda(e) => e.syntax(),
            Expr::Call(e) => e.syntax(),
            Expr::Index(e) => e.syntax(),
            Expr::Member(e) => e.syntax(),
            Expr::NonNull(e) => e.syntax(),
            Expr::New(e) => e.syntax(),
            Expr::Amends(e) => e.syntax(),
            Expr::Throw(e) => e.syntax(),
            Expr::Trace(e) => e.syntax(),
            Expr::Read(e) => e.syntax(),
            Expr::Import(e) => e.syntax(),
            Expr::Error(e) => e.syntax(),
        }
    }
}

ast_node!(LiteralExpr, SyntaxKind::LiteralExpr);

/// A `"..."` or `"""..."""` (or `#"..."#`, ...) literal that contains at
/// least one `\(...)` interpolation hole. Strings without any interpolation
/// stay on the simpler `LiteralExpr` path so existing tooling doesn't have
/// to grow new cases for the common literal-only shape.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InterpolatedString(pub(crate) SyntaxNode);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InterpolatedStringKind {
    /// Single-line `"..."` (or `#"..."#`).
    SingleLine,
    /// Multi-line `"""..."""` (or `#"""..."""#`).
    Multiline,
}

impl AstNode for InterpolatedString {
    #[inline]
    fn can_cast(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            SyntaxKind::InterpolatedString | SyntaxKind::InterpolatedMultilineString
        )
    }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        if Self::can_cast(syntax.kind()) {
            Some(Self(syntax))
        } else {
            None
        }
    }
    #[inline]
    fn syntax(&self) -> &SyntaxNode {
        &self.0
    }
}

impl InterpolatedString {
    pub fn kind(&self) -> InterpolatedStringKind {
        match self.0.kind() {
            SyntaxKind::InterpolatedMultilineString => InterpolatedStringKind::Multiline,
            _ => InterpolatedStringKind::SingleLine,
        }
    }

    /// The opening `"` / `"""` / `#"...` quote token.
    pub fn open_quote(&self) -> Option<SyntaxToken> {
        child_token(&self.0, SyntaxKind::StringQuoteOpen)
    }

    /// The closing quote token, if the literal was terminated.
    pub fn close_quote(&self) -> Option<SyntaxToken> {
        child_token(&self.0, SyntaxKind::StringQuoteClose)
    }

    /// Iterate over the literal-text `StringPart` / `MultilineStringPart`
    /// children, in source order.
    pub fn parts(&self) -> impl Iterator<Item = SyntaxToken> + '_ {
        self.0
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| {
                matches!(
                    t.kind(),
                    SyntaxKind::StringPart | SyntaxKind::MultilineStringPart
                )
            })
    }

    /// Iterate over the interpolation holes in source order.
    pub fn interpolations(&self) -> impl Iterator<Item = Interpolation> + '_ {
        self.0.children().filter_map(Interpolation::cast)
    }
}

ast_node!(Interpolation, SyntaxKind::Interpolation);

impl Interpolation {
    /// The opening `\(` (or `#\(` / `##\(` / ...) marker.
    pub fn open_marker(&self) -> Option<SyntaxToken> {
        child_token(&self.0, SyntaxKind::InterpolationStart)
    }

    /// The closing `)` (or `)#` / ...) marker, if present.
    pub fn close_marker(&self) -> Option<SyntaxToken> {
        child_token(&self.0, SyntaxKind::InterpolationEnd)
    }

    /// The expression sitting inside the `\(...)` hole.
    pub fn expr(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LiteralKind {
    Int,
    Float,
    String,
    MultilineString,
    Bool,
    Null,
}

impl LiteralExpr {
    pub fn kind(&self) -> Option<LiteralKind> {
        let t = self.token()?;
        Some(match t.kind() {
            SyntaxKind::IntNumber
            | SyntaxKind::HexNumber
            | SyntaxKind::BinNumber
            | SyntaxKind::OctNumber => LiteralKind::Int,
            SyntaxKind::FloatNumber => LiteralKind::Float,
            SyntaxKind::String => LiteralKind::String,
            SyntaxKind::MultilineString => LiteralKind::MultilineString,
            SyntaxKind::TrueKw | SyntaxKind::FalseKw => LiteralKind::Bool,
            SyntaxKind::NullKw => LiteralKind::Null,
            _ => return None,
        })
    }

    pub fn token(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| !t.kind().is_trivia())
    }

    pub fn bool_value(&self) -> Option<bool> {
        let t = self.token()?;
        match t.kind() {
            SyntaxKind::TrueKw => Some(true),
            SyntaxKind::FalseKw => Some(false),
            _ => None,
        }
    }
}

ast_node!(IdentExpr, SyntaxKind::IdentExpr);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SpecialIdentKind {
    This,
    Super,
    Outer,
    Module,
}

impl IdentExpr {
    /// The identifier or special keyword (`this`, `super`, `outer`, `module`).
    pub fn token(&self) -> Option<SyntaxToken> {
        self.0
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| !t.kind().is_trivia())
    }

    pub fn special(&self) -> Option<SpecialIdentKind> {
        let t = self.token()?;
        Some(match t.kind() {
            SyntaxKind::ThisKw => SpecialIdentKind::This,
            SyntaxKind::SuperKw => SpecialIdentKind::Super,
            SyntaxKind::OuterKw => SpecialIdentKind::Outer,
            SyntaxKind::ModuleKw => SpecialIdentKind::Module,
            _ => return None,
        })
    }
}

ast_node!(ParenExpr, SyntaxKind::ParenExpr);

impl ParenExpr {
    pub fn inner(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

ast_node!(UnaryExpr, SyntaxKind::UnaryExpr);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

impl UnaryExpr {
    pub fn op(&self) -> Option<UnaryOp> {
        for el in self.0.children_with_tokens() {
            if let Some(t) = el.as_token() {
                return Some(match t.kind() {
                    SyntaxKind::Minus => UnaryOp::Neg,
                    SyntaxKind::Bang => UnaryOp::Not,
                    _ => continue,
                });
            }
        }
        None
    }

    pub fn operand(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

ast_node!(BinaryExpr, SyntaxKind::BinaryExpr);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    TruncDiv,
    Rem,
    Pow,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    Pipeline,
}

impl BinaryExpr {
    pub fn lhs(&self) -> Option<Expr> {
        self.0.children().find_map(Expr::cast)
    }

    pub fn rhs(&self) -> Option<Expr> {
        let mut iter = self.0.children().filter_map(Expr::cast);
        iter.next();
        iter.next()
    }

    /// The operator token. For `||` and `&&` this returns the first
    /// `|`/`&` token (the second one is the next direct child).
    pub fn op_token(&self) -> Option<SyntaxToken> {
        // The operator is the first non-trivia token that sits between
        // the LHS and the RHS subtrees.
        let mut state = 0u8; // 0 = pre-lhs, 1 = inside lhs, 2 = post-lhs
        let mut saw_lhs = false;
        for el in self.0.children_with_tokens() {
            match el {
                rowan::NodeOrToken::Node(_) if !saw_lhs => {
                    saw_lhs = true;
                    state = 2;
                }
                rowan::NodeOrToken::Node(_) => {
                    // RHS reached — operator already consumed.
                    return None;
                }
                rowan::NodeOrToken::Token(t) if state == 2 && !t.kind().is_trivia() => {
                    return Some(t.clone());
                }
                _ => {}
            }
        }
        None
    }

    pub fn op(&self) -> Option<BinaryOp> {
        let t = self.op_token()?;
        Some(match t.kind() {
            SyntaxKind::Plus => BinaryOp::Add,
            SyntaxKind::Minus => BinaryOp::Sub,
            SyntaxKind::Star => BinaryOp::Mul,
            SyntaxKind::Slash => BinaryOp::Div,
            SyntaxKind::TildeSlash => BinaryOp::TruncDiv,
            SyntaxKind::Percent => BinaryOp::Rem,
            SyntaxKind::StarStar => BinaryOp::Pow,
            SyntaxKind::EqEq => BinaryOp::Eq,
            SyntaxKind::BangEq => BinaryOp::NotEq,
            SyntaxKind::Lt => BinaryOp::Lt,
            SyntaxKind::LtEq => BinaryOp::LtEq,
            SyntaxKind::Gt => BinaryOp::Gt,
            SyntaxKind::GtEq => BinaryOp::GtEq,
            SyntaxKind::Amp => BinaryOp::And,
            SyntaxKind::Pipe => BinaryOp::Or,
            SyntaxKind::PipeGt => BinaryOp::Pipeline,
            _ => return None,
        })
    }
}

ast_node!(NullCoalesceExpr, SyntaxKind::NullCoalesceExpr);

impl NullCoalesceExpr {
    pub fn lhs(&self) -> Option<Expr> {
        self.0.children().find_map(Expr::cast)
    }
    pub fn rhs(&self) -> Option<Expr> {
        let mut iter = self.0.children().filter_map(Expr::cast);
        iter.next();
        iter.next()
    }
}

ast_node!(TypeCheckExpr, SyntaxKind::TypeCheckExpr);

impl TypeCheckExpr {
    pub fn operand(&self) -> Option<Expr> {
        child_node(&self.0)
    }
    pub fn ty(&self) -> Option<Type> {
        child_node(&self.0)
    }
}

ast_node!(TypeCastExpr, SyntaxKind::TypeCastExpr);

impl TypeCastExpr {
    pub fn operand(&self) -> Option<Expr> {
        child_node(&self.0)
    }
    pub fn ty(&self) -> Option<Type> {
        child_node(&self.0)
    }
}

ast_node!(IfExpr, SyntaxKind::IfExpr);

impl IfExpr {
    pub fn condition(&self) -> Option<Expr> {
        nth_expr(&self.0, 0)
    }
    pub fn then_branch(&self) -> Option<Expr> {
        nth_expr(&self.0, 1)
    }
    pub fn else_branch(&self) -> Option<Expr> {
        nth_expr(&self.0, 2)
    }
}

ast_node!(LetExpr, SyntaxKind::LetExpr);

impl LetExpr {
    pub fn binding(&self) -> Option<Parameter> {
        child_node(&self.0)
    }
    pub fn value(&self) -> Option<Expr> {
        nth_expr(&self.0, 0)
    }
    pub fn body(&self) -> Option<Expr> {
        nth_expr(&self.0, 1)
    }
}

ast_node!(LambdaExpr, SyntaxKind::LambdaExpr);

impl LambdaExpr {
    pub fn parameters(&self) -> Option<ParameterList> {
        child_node(&self.0)
    }
    pub fn body(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

ast_node!(CallExpr, SyntaxKind::CallExpr);

impl CallExpr {
    pub fn callee(&self) -> Option<Expr> {
        child_node(&self.0)
    }
    pub fn arg_list(&self) -> Option<ArgList> {
        child_node(&self.0)
    }
    pub fn args(&self) -> Vec<Expr> {
        self.arg_list()
            .into_iter()
            .flat_map(|a| a.args().collect::<Vec<_>>())
            .collect()
    }
}

ast_node!(ArgList, SyntaxKind::ArgList);

impl ArgList {
    pub fn args(&self) -> impl Iterator<Item = Expr> + '_ {
        self.0.children().filter_map(Expr::cast)
    }
}

ast_node!(IndexExpr, SyntaxKind::IndexExpr);

impl IndexExpr {
    pub fn receiver(&self) -> Option<Expr> {
        nth_expr(&self.0, 0)
    }
    pub fn index(&self) -> Option<Expr> {
        nth_expr(&self.0, 1)
    }
}

ast_node!(MemberExpr, SyntaxKind::MemberExpr);

impl MemberExpr {
    pub fn receiver(&self) -> Option<Expr> {
        child_node(&self.0)
    }
    pub fn nullable(&self) -> bool {
        child_token(&self.0, SyntaxKind::QuestionDot).is_some()
    }
    pub fn name(&self) -> Option<SyntaxToken> {
        // The identifier after the `.` / `?.` token.
        let mut saw_dot = false;
        for el in self.0.children_with_tokens() {
            if let Some(t) = el.as_token() {
                if matches!(t.kind(), SyntaxKind::Dot | SyntaxKind::QuestionDot) {
                    saw_dot = true;
                } else if saw_dot && matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::QuotedIdent)
                {
                    return Some(t.clone());
                }
            }
        }
        None
    }
}

ast_node!(NonNullExpr, SyntaxKind::NonNullExpr);

impl NonNullExpr {
    pub fn operand(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

ast_node!(NewExpr, SyntaxKind::NewExpr);

impl NewExpr {
    pub fn ty(&self) -> Option<Type> {
        child_node(&self.0)
    }
    pub fn body(&self) -> Option<ObjectBody> {
        child_node(&self.0)
    }
}

ast_node!(AmendsExpr, SyntaxKind::AmendsExpr);

impl AmendsExpr {
    pub fn base(&self) -> Option<Expr> {
        child_node(&self.0)
    }
    pub fn body(&self) -> Option<ObjectBody> {
        child_node(&self.0)
    }
}

ast_node!(ThrowExpr, SyntaxKind::ThrowExpr);

impl ThrowExpr {
    pub fn argument(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

ast_node!(TraceExpr, SyntaxKind::TraceExpr);

impl TraceExpr {
    pub fn argument(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ReadKind {
    Read,
    ReadOrNull,
    ReadGlob,
}

ast_node!(ReadExpr, SyntaxKind::ReadExpr);

impl ReadExpr {
    pub fn read_kind(&self) -> Option<ReadKind> {
        for el in self.0.children_with_tokens() {
            if let Some(t) = el.as_token() {
                return Some(match t.kind() {
                    SyntaxKind::ReadKw => ReadKind::Read,
                    SyntaxKind::ReadOrNullKw => ReadKind::ReadOrNull,
                    SyntaxKind::ReadGlobKw => ReadKind::ReadGlob,
                    _ => continue,
                });
            }
        }
        None
    }

    pub fn argument(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

ast_node!(ImportExpr, SyntaxKind::ImportExpr);

impl ImportExpr {
    pub fn argument(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

// ----------------------------------------------------------------------
// Object bodies and members

ast_node!(ObjectBody, SyntaxKind::ObjectBody);

impl ObjectBody {
    pub fn parameters(&self) -> Option<ParameterList> {
        child_node(&self.0)
    }
    pub fn members(&self) -> impl Iterator<Item = ObjectMember> + '_ {
        self.0.children().filter_map(ObjectMember::cast)
    }
}

#[derive(Clone, Debug)]
pub enum ObjectMember {
    Property(ObjectProperty),
    Method(ObjectMethod),
    Element(ObjectElement),
    Entry(ObjectEntryComputed),
    When(WhenGenerator),
    For(ForGenerator),
    Spread(ObjectSpread),
}

impl AstNode for ObjectMember {
    fn can_cast(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            SyntaxKind::ObjectProperty
                | SyntaxKind::ObjectMethod
                | SyntaxKind::ObjectElement
                | SyntaxKind::ObjectEntryComputed
                | SyntaxKind::WhenGenerator
                | SyntaxKind::ForGenerator
                | SyntaxKind::ObjectSpread
        )
    }
    fn cast(syntax: SyntaxNode) -> Option<Self> {
        Some(match syntax.kind() {
            SyntaxKind::ObjectProperty => ObjectMember::Property(ObjectProperty(syntax)),
            SyntaxKind::ObjectMethod => ObjectMember::Method(ObjectMethod(syntax)),
            SyntaxKind::ObjectElement => ObjectMember::Element(ObjectElement(syntax)),
            SyntaxKind::ObjectEntryComputed => ObjectMember::Entry(ObjectEntryComputed(syntax)),
            SyntaxKind::WhenGenerator => ObjectMember::When(WhenGenerator(syntax)),
            SyntaxKind::ForGenerator => ObjectMember::For(ForGenerator(syntax)),
            SyntaxKind::ObjectSpread => ObjectMember::Spread(ObjectSpread(syntax)),
            _ => return None,
        })
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            ObjectMember::Property(p) => p.syntax(),
            ObjectMember::Method(m) => m.syntax(),
            ObjectMember::Element(e) => e.syntax(),
            ObjectMember::Entry(e) => e.syntax(),
            ObjectMember::When(w) => w.syntax(),
            ObjectMember::For(f) => f.syntax(),
            ObjectMember::Spread(s) => s.syntax(),
        }
    }
}

ast_node!(ObjectElement, SyntaxKind::ObjectElement);

impl ObjectElement {
    pub fn expr(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

ast_node!(ObjectEntryComputed, SyntaxKind::ObjectEntryComputed);

impl ObjectEntryComputed {
    pub fn key(&self) -> Option<Expr> {
        self.0.children().filter_map(Expr::cast).next()
    }
    pub fn value(&self) -> Option<PropertyValue> {
        // Skip the key, then look for an Expr (after `=`) or ObjectBody.
        let mut found_eq = false;
        for el in self.0.children_with_tokens() {
            match el {
                rowan::NodeOrToken::Token(t) if t.kind() == SyntaxKind::Eq => {
                    found_eq = true;
                }
                rowan::NodeOrToken::Node(n) => {
                    if found_eq {
                        if let Some(e) = Expr::cast(n.clone()) {
                            return Some(PropertyValue::Expr(e));
                        }
                    }
                    if n.kind() == SyntaxKind::ObjectBody {
                        return ObjectBody::cast(n).map(PropertyValue::ObjectBody);
                    }
                }
                _ => {}
            }
        }
        None
    }
}

ast_node!(WhenGenerator, SyntaxKind::WhenGenerator);

impl WhenGenerator {
    pub fn condition(&self) -> Option<Expr> {
        child_node(&self.0)
    }
    pub fn then_body(&self) -> Option<ObjectBody> {
        self.0.children().filter_map(ObjectBody::cast).next()
    }
    pub fn else_body(&self) -> Option<ObjectBody> {
        self.0.children().filter_map(ObjectBody::cast).nth(1)
    }
}

ast_node!(ForGenerator, SyntaxKind::ForGenerator);

impl ForGenerator {
    pub fn bindings(&self) -> impl Iterator<Item = Parameter> + '_ {
        self.0.children().filter_map(Parameter::cast)
    }
    pub fn iterable(&self) -> Option<Expr> {
        child_node(&self.0)
    }
    pub fn body(&self) -> Option<ObjectBody> {
        child_node(&self.0)
    }
}

ast_node!(ObjectSpread, SyntaxKind::ObjectSpread);

impl ObjectSpread {
    pub fn expr(&self) -> Option<Expr> {
        child_node(&self.0)
    }
}

// ----------------------------------------------------------------------
// Misc helpers

fn nth_expr(parent: &SyntaxNode, n: usize) -> Option<Expr> {
    parent.children().filter_map(Expr::cast).nth(n)
}

/// Strip surrounding backticks from a quoted identifier; returns the
/// token's text unchanged for unquoted identifiers.
pub fn ident_text(token: &SyntaxToken) -> String {
    let text = token.text();
    if token.kind() == SyntaxKind::QuotedIdent {
        text.trim_start_matches('`')
            .trim_end_matches('`')
            .to_owned()
    } else {
        text.to_owned()
    }
}

// ----------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::green::parse_green;

    fn parse(src: &str) -> Module {
        let parsed = parse_green(src);
        Module::cast(parsed.syntax).expect("Module root")
    }

    #[test]
    fn module_walks_header_imports_items() {
        let m = parse(
            "module foo.bar\n\
             import \"a.pkl\"\n\
             import \"b.pkl\" as bb\n\
             x: Int = 1\n\
             class Box { value: String }\n\
             function f(n: Int): Int = n + 1\n",
        );
        let header = m.header().expect("header");
        assert_eq!(header.name().expect("name").text_joined(), "foo.bar");

        let imports: Vec<_> = m.imports().collect();
        assert_eq!(imports.len(), 2);
        assert_eq!(
            imports[1].alias().map(|t| ident_text(&t)).as_deref(),
            Some("bb")
        );

        let items: Vec<_> = m.items().collect();
        assert_eq!(items.len(), 3);
        match &items[0] {
            Item::Property(p) => {
                assert_eq!(p.name().map(|t| ident_text(&t)).as_deref(), Some("x"));
                assert!(matches!(p.value(), Some(PropertyValue::Expr(_))));
            }
            _ => panic!("expected property"),
        }
        match &items[1] {
            Item::Class(c) => {
                assert_eq!(c.name().map(|t| ident_text(&t)).as_deref(), Some("Box"));
                let members: Vec<_> = c.body().expect("body").members().collect();
                assert_eq!(members.len(), 1);
            }
            _ => panic!("expected class"),
        }
        match &items[2] {
            Item::Method(m) => {
                assert_eq!(m.name().map(|t| ident_text(&t)).as_deref(), Some("f"));
                let params: Vec<_> = m.parameters().unwrap().parameters().collect();
                assert_eq!(params.len(), 1);
                assert_eq!(
                    params[0].name().map(|t| ident_text(&t)).as_deref(),
                    Some("n")
                );
            }
            _ => panic!("expected method"),
        }
    }

    #[test]
    fn binary_expr_op_recovery() {
        let m = parse("x = 1 + 2 * 3\n");
        let prop = m.items().next().unwrap();
        match prop {
            Item::Property(p) => match p.value() {
                Some(PropertyValue::Expr(Expr::Binary(b))) => {
                    assert_eq!(b.op(), Some(BinaryOp::Add));
                }
                other => panic!("expected binary expr, got {:?}", other),
            },
            _ => panic!("expected property"),
        }
    }

    #[test]
    fn doc_comment_for_property() {
        let m = parse("/// hello world\nx = 1\n");
        let item = m.items().next().unwrap();
        match item {
            Item::Property(p) => {
                assert_eq!(p.doc_comment().as_deref(), Some("hello world"));
            }
            _ => panic!("expected property"),
        }
    }

    #[test]
    fn import_glob_flag() {
        let m = parse("import* \"x/*.pkl\"\n");
        let import = m.imports().next().unwrap();
        assert!(import.is_glob());
    }

    #[test]
    fn class_extends_and_modifiers() {
        let m = parse("open class Foo extends Bar { x: Int }\n");
        let item = m.items().next().unwrap();
        match item {
            Item::Class(c) => {
                let mods: Vec<_> = c.modifiers().filter_map(|m| m.kind()).collect();
                assert_eq!(mods, vec![ModifierKind::Open]);
                assert!(c.extends().is_some());
            }
            _ => panic!("expected class"),
        }
    }

    #[test]
    fn round_trip_via_typed_view() {
        let src = "module foo\nx = 1\n";
        let parsed = parse_green(src);
        let m = Module::cast(parsed.syntax.clone()).unwrap();
        assert_eq!(m.text(), src);
        assert_eq!(m.span(), Span::new(0, src.len() as u32));
    }

    #[test]
    fn interpolated_string_exposes_parts_and_holes() {
        let m = parse("x = \"a\\(name)b\\(x + y)c\"\n");
        let prop = match m.items().next().unwrap() {
            Item::Property(p) => p,
            _ => panic!("expected property"),
        };
        let value = match prop.value() {
            Some(PropertyValue::Expr(e)) => e,
            _ => panic!("expected expression value"),
        };
        let s = match value {
            Expr::InterpolatedString(s) => s,
            other => panic!("expected interpolated string, got {:?}", other),
        };
        assert_eq!(s.kind(), InterpolatedStringKind::SingleLine);
        assert!(s.open_quote().is_some());
        assert!(s.close_quote().is_some());
        let parts: Vec<String> = s.parts().map(|t| t.text().to_owned()).collect();
        assert_eq!(parts, vec!["a", "b", "c"]);
        let holes: Vec<_> = s.interpolations().collect();
        assert_eq!(holes.len(), 2);
        // First hole resolves to `name` (an IdentExpr).
        let first_expr = holes[0].expr().expect("first interpolation expr");
        assert!(matches!(first_expr, Expr::Ident(_)));
        // Second is a binary expr.
        let second_expr = holes[1].expr().expect("second interpolation expr");
        assert!(matches!(second_expr, Expr::Binary(_)));
    }
}

//! Typed AST for Pkl.
//!
//! This AST is intentionally minimal for the foundation: it captures the
//! structural shape of a Pkl module — module header, imports, declarations,
//! and expressions — but does not attempt to encode every grammar production
//! up front. New variants are added as parser support lands.
//!
//! Every node carries a [`Span`] so the LSP layer can map back to a source
//! range without re-walking the original tokens.

use crate::span::Span;

#[derive(Clone, Debug)]
pub struct Module {
    pub span: Span,
    pub header: Option<ModuleHeader>,
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub struct ModuleHeader {
    pub span: Span,
    pub doc_comment: Option<String>,
    pub annotations: Vec<Annotation>,
    pub modifiers: Vec<Modifier>,
    /// The qualified name (e.g. `acme.config`). Empty if the file used
    /// `amends "..."` / `extends "..."` without an explicit module name.
    pub name: Option<QualifiedName>,
    pub clause: Option<ExtendsAmendsClause>,
}

#[derive(Clone, Debug)]
pub struct QualifiedName {
    pub span: Span,
    pub segments: Vec<Identifier>,
}

#[derive(Clone, Debug)]
pub struct Identifier {
    pub span: Span,
    pub name: String,
}

#[derive(Clone, Debug)]
pub enum ExtendsAmendsClause {
    Extends { span: Span, target: StringLit },
    Amends { span: Span, target: StringLit },
}

#[derive(Clone, Debug)]
pub struct StringLit {
    pub span: Span,
    /// Raw text including delimiters.
    pub raw: String,
    /// Decoded value when escape sequences are simple enough that we can do
    /// it during parsing. Interpolation, multi-line and custom-delimited
    /// strings leave this `None`.
    pub value: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub span: Span,
    pub is_glob: bool,
    pub path: StringLit,
    pub alias: Option<Identifier>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Class(ClassDecl),
    TypeAlias(TypeAliasDecl),
    Property(PropertyDecl),
    Method(MethodDecl),
    /// Unrecognised item — kept so the editor still sees a span we can hover
    /// or rename without crashing.
    Error(ErrorItem),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Class(c) => c.span,
            Item::TypeAlias(t) => t.span,
            Item::Property(p) => p.span,
            Item::Method(m) => m.span,
            Item::Error(e) => e.span,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Item::Class(c) => Some(&c.name.name),
            Item::TypeAlias(t) => Some(&t.name.name),
            Item::Property(p) => Some(&p.name.name),
            Item::Method(m) => Some(&m.name.name),
            Item::Error(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ErrorItem {
    pub span: Span,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ClassDecl {
    pub span: Span,
    pub doc_comment: Option<String>,
    pub annotations: Vec<Annotation>,
    pub modifiers: Vec<Modifier>,
    pub name: Identifier,
    pub type_parameters: Vec<TypeParameter>,
    pub extends: Option<TypeRef>,
    pub body: Option<ClassBody>,
}

#[derive(Clone, Debug, Default)]
pub struct ClassBody {
    pub span: Span,
    pub members: Vec<ClassMember>,
}

#[derive(Clone, Debug)]
pub enum ClassMember {
    Property(PropertyDecl),
    Method(MethodDecl),
}

impl ClassMember {
    pub fn span(&self) -> Span {
        match self {
            ClassMember::Property(p) => p.span,
            ClassMember::Method(m) => m.span,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TypeAliasDecl {
    pub span: Span,
    pub doc_comment: Option<String>,
    pub annotations: Vec<Annotation>,
    pub modifiers: Vec<Modifier>,
    pub name: Identifier,
    pub type_parameters: Vec<TypeParameter>,
    pub aliased: Option<TypeRef>,
}

#[derive(Clone, Debug)]
pub struct PropertyDecl {
    pub span: Span,
    pub doc_comment: Option<String>,
    pub annotations: Vec<Annotation>,
    pub modifiers: Vec<Modifier>,
    pub name: Identifier,
    pub ty: Option<TypeRef>,
    /// `= expr` for value bindings, `{ ... }` for object-body bindings, or
    /// `None` for type-only declarations inside a class.
    pub value: Option<PropertyValue>,
}

#[derive(Clone, Debug)]
pub enum PropertyValue {
    Expr(Expr),
    ObjectBody(ObjectBody),
}

#[derive(Clone, Debug)]
pub struct MethodDecl {
    pub span: Span,
    pub doc_comment: Option<String>,
    pub annotations: Vec<Annotation>,
    pub modifiers: Vec<Modifier>,
    pub name: Identifier,
    pub type_parameters: Vec<TypeParameter>,
    pub parameters: Vec<Parameter>,
    pub return_type: Option<TypeRef>,
    pub body: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct Parameter {
    pub span: Span,
    pub name: Identifier,
    pub ty: Option<TypeRef>,
}

#[derive(Clone, Debug)]
pub struct TypeParameter {
    pub span: Span,
    pub variance: Option<Variance>,
    pub name: Identifier,
}

#[derive(Clone, Copy, Debug)]
pub enum Variance {
    In,
    Out,
}

#[derive(Clone, Debug)]
pub struct Annotation {
    pub span: Span,
    pub name: QualifiedName,
    pub body: Option<ObjectBody>,
}

#[derive(Clone, Debug)]
pub struct Modifier {
    pub span: Span,
    pub kind: ModifierKind,
}

#[derive(Clone, Copy, Debug)]
pub enum ModifierKind {
    Abstract,
    Open,
    Local,
    Hidden,
    Fixed,
    External,
}

// ------------------------------------------------------------------
// Types

#[derive(Clone, Debug)]
pub enum TypeRef {
    /// `Foo` / `acme.Foo` / `Foo<Bar>`
    Named {
        span: Span,
        name: QualifiedName,
        arguments: Vec<TypeRef>,
    },
    Nullable {
        span: Span,
        inner: Box<TypeRef>,
    },
    Union {
        span: Span,
        members: Vec<TypeRef>,
    },
    Function {
        span: Span,
        parameters: Vec<TypeRef>,
        result: Box<TypeRef>,
    },
    Parenthesized {
        span: Span,
        inner: Box<TypeRef>,
    },
    StringLiteral(StringLit),
    /// `unknown`
    Unknown(Span),
    /// `nothing`
    Nothing(Span),
    /// `module`
    Module(Span),
    /// Recovery node.
    Error {
        span: Span,
        message: String,
    },
}

impl TypeRef {
    pub fn span(&self) -> Span {
        match self {
            TypeRef::Named { span, .. }
            | TypeRef::Nullable { span, .. }
            | TypeRef::Union { span, .. }
            | TypeRef::Function { span, .. }
            | TypeRef::Parenthesized { span, .. }
            | TypeRef::Error { span, .. } => *span,
            TypeRef::StringLiteral(s) => s.span,
            TypeRef::Unknown(s) | TypeRef::Nothing(s) | TypeRef::Module(s) => *s,
        }
    }
}

// ------------------------------------------------------------------
// Expressions

#[derive(Clone, Debug)]
pub enum Expr {
    Literal(Literal),
    Ident(Identifier),
    /// `this`, `super`, `outer`, `module`
    SpecialIdent {
        span: Span,
        kind: SpecialIdentKind,
    },
    Paren {
        span: Span,
        inner: Box<Expr>,
    },
    Unary {
        span: Span,
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        span: Span,
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `expr is Type` / `expr as Type`
    TypeCheck {
        span: Span,
        operand: Box<Expr>,
        ty: TypeRef,
    },
    TypeCast {
        span: Span,
        operand: Box<Expr>,
        ty: TypeRef,
    },
    If {
        span: Span,
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    Let {
        span: Span,
        binding: Box<Parameter>,
        value: Box<Expr>,
        body: Box<Expr>,
    },
    Lambda {
        span: Span,
        parameters: Vec<Parameter>,
        body: Box<Expr>,
    },
    Call {
        span: Span,
        callee: Box<Expr>,
        type_args: Vec<TypeRef>,
        args: Vec<Expr>,
    },
    Index {
        span: Span,
        receiver: Box<Expr>,
        index: Box<Expr>,
    },
    Member {
        span: Span,
        receiver: Box<Expr>,
        nullable: bool,
        name: Identifier,
    },
    NonNull {
        span: Span,
        operand: Box<Expr>,
    },
    New {
        span: Span,
        ty: Option<TypeRef>,
        body: ObjectBody,
    },
    AmendsObject {
        span: Span,
        base: Box<Expr>,
        body: ObjectBody,
    },
    Throw {
        span: Span,
        argument: Box<Expr>,
    },
    Trace {
        span: Span,
        argument: Box<Expr>,
    },
    Read {
        span: Span,
        kind: ReadKind,
        argument: Box<Expr>,
    },
    Error {
        span: Span,
        message: String,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Literal(l) => l.span(),
            Expr::Ident(i) => i.span,
            Expr::SpecialIdent { span, .. }
            | Expr::Paren { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::TypeCheck { span, .. }
            | Expr::TypeCast { span, .. }
            | Expr::If { span, .. }
            | Expr::Let { span, .. }
            | Expr::Lambda { span, .. }
            | Expr::Call { span, .. }
            | Expr::Index { span, .. }
            | Expr::Member { span, .. }
            | Expr::NonNull { span, .. }
            | Expr::New { span, .. }
            | Expr::AmendsObject { span, .. }
            | Expr::Throw { span, .. }
            | Expr::Trace { span, .. }
            | Expr::Read { span, .. }
            | Expr::Error { span, .. } => *span,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SpecialIdentKind {
    This,
    Super,
    Outer,
    Module,
}

#[derive(Clone, Copy, Debug)]
pub enum ReadKind {
    Read,
    ReadOrNull,
    ReadGlob,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
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
    NullCoalesce,
    Pipeline,
}

#[derive(Clone, Debug)]
pub enum Literal {
    Int { span: Span, raw: String },
    Float { span: Span, raw: String },
    String(StringLit),
    Bool { span: Span, value: bool },
    Null { span: Span },
}

impl Literal {
    pub fn span(&self) -> Span {
        match self {
            Literal::Int { span, .. }
            | Literal::Float { span, .. }
            | Literal::Bool { span, .. }
            | Literal::Null { span } => *span,
            Literal::String(s) => s.span,
        }
    }
}

// ------------------------------------------------------------------
// Object bodies (Pkl's `{ ... }` form)

#[derive(Clone, Debug, Default)]
pub struct ObjectBody {
    pub span: Span,
    pub parameters: Vec<Parameter>,
    pub members: Vec<ObjectMember>,
}

#[derive(Clone, Debug)]
pub enum ObjectMember {
    /// `name = expr` / `name { body }` / `name: T = expr`
    Property(PropertyDecl),
    /// `function f(...) = expr` inside an object body.
    Method(MethodDecl),
    /// Bare expression element (e.g. inside a `Listing { 1; 2; 3 }`).
    Element(Expr),
    /// `[key] = value`
    Entry {
        span: Span,
        key: Expr,
        value: PropertyValue,
    },
    /// `when (cond) { ... } else { ... }`
    When {
        span: Span,
        cond: Expr,
        then_body: ObjectBody,
        else_body: Option<ObjectBody>,
    },
    /// `for (x in expr) { ... }` / `for (k, v in expr) { ... }`
    For {
        span: Span,
        bindings: Vec<Parameter>,
        iterable: Expr,
        body: ObjectBody,
    },
    /// `...expr` (spread)
    Spread { span: Span, expr: Expr },
}

impl ObjectMember {
    pub fn span(&self) -> Span {
        match self {
            ObjectMember::Property(p) => p.span,
            ObjectMember::Method(m) => m.span,
            ObjectMember::Element(e) => e.span(),
            ObjectMember::Entry { span, .. }
            | ObjectMember::When { span, .. }
            | ObjectMember::For { span, .. }
            | ObjectMember::Spread { span, .. } => *span,
        }
    }
}

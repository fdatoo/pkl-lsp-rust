//! Symbol table: every declaration the analyzer cares about.

use pkl_syntax::cst::ModifierKind;
use pkl_syntax::span::Span;

use crate::types::Ty;

/// Stable handle to a [`Symbol`]. Indices into [`SymbolTable`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId(pub(crate) u32);

impl SymbolId {
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub id: SymbolId,
    pub kind: SymbolKind,
    pub name: String,
    /// Span of the name token at the definition site (used for goto-def).
    pub name_span: Span,
    /// Span of the full declaration (used for hover ranges).
    pub full_span: Span,
    /// Enclosing symbol, if any — e.g. a property's class.
    pub container: Option<SymbolId>,
    /// Pretty-printed signature shown in hover: `name: Type` for properties,
    /// `function f(p: T): R` for methods, `class Foo` etc. Optional so
    /// imports / parameters that have nothing extra to say leave it `None`.
    pub signature: Option<String>,
    pub doc: Option<String>,
    pub modifiers: Vec<ModifierKind>,
    /// Where this symbol came from. User-defined symbols carry
    /// [`Origin::User`]; built-in types from `pkl.base` carry
    /// [`Origin::Stdlib`]. Stdlib symbols have empty spans and goto-def
    /// returns no location for them.
    pub origin: Origin,
    /// Statically-known type of this symbol's *value* — the property's
    /// declared type, a parameter's declared type, a method's return
    /// type, etc. The inferrer uses this to seed expression types.
    pub declared_ty: Ty,
    /// For class symbols only: the bare name of the parent class
    /// (`extends Foo`). Walked by member lookup so inherited members
    /// resolve. `None` for non-class symbols and for classes without
    /// an explicit parent.
    pub parent_class: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    User,
    Stdlib { module: &'static str },
}

impl Origin {
    #[inline]
    pub fn is_stdlib(self) -> bool {
        matches!(self, Origin::Stdlib { .. })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Module,
    Import {
        is_glob: bool,
    },
    Class,
    TypeAlias,
    /// A property declared at module level or inside a class body. Object
    /// body members (Pkl's `{ ... }` form) are not registered as symbols
    /// during this analyzer pass — those require type resolution to know
    /// which class they amend.
    Property,
    Method,
    /// A method/function parameter.
    Parameter,
    /// A `<T>` type parameter on a class, typealias, or method.
    TypeParameter,
    /// The binding introduced by `let (x = ...) ...`.
    LetBinding,
    /// A binding in an object body's lambda head (e.g. `{ x -> ... }`).
    ObjectParameter,
    /// A binding in a `for (x in ...) { ... }` generator.
    ForBinding,
}

impl SymbolKind {
    pub fn describe(self) -> &'static str {
        match self {
            SymbolKind::Module => "module",
            SymbolKind::Import { is_glob: false } => "import",
            SymbolKind::Import { is_glob: true } => "glob import",
            SymbolKind::Class => "class",
            SymbolKind::TypeAlias => "type alias",
            SymbolKind::Property => "property",
            SymbolKind::Method => "function",
            SymbolKind::Parameter => "parameter",
            SymbolKind::TypeParameter => "type parameter",
            SymbolKind::LetBinding => "let binding",
            SymbolKind::ObjectParameter => "object parameter",
            SymbolKind::ForBinding => "for binding",
        }
    }
}

/// Inputs needed to register a new [`Symbol`]. `id` is assigned by
/// [`SymbolTable::insert`].
pub struct SymbolData {
    pub kind: SymbolKind,
    pub name: String,
    pub name_span: Span,
    pub full_span: Span,
    pub container: Option<SymbolId>,
    pub signature: Option<String>,
    pub doc: Option<String>,
    pub modifiers: Vec<ModifierKind>,
    pub origin: Origin,
    pub declared_ty: Ty,
    pub parent_class: Option<String>,
}

#[derive(Default)]
pub struct SymbolTable {
    symbols: Vec<Symbol>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, data: SymbolData) -> SymbolId {
        let id = SymbolId(self.symbols.len() as u32);
        self.symbols.push(Symbol {
            id,
            kind: data.kind,
            name: data.name,
            name_span: data.name_span,
            full_span: data.full_span,
            container: data.container,
            signature: data.signature,
            doc: data.doc,
            modifiers: data.modifiers,
            origin: data.origin,
            declared_ty: data.declared_ty,
            parent_class: data.parent_class,
        });
        id
    }

    #[inline]
    pub fn get(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id.index()]
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &Symbol> {
        self.symbols.iter()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }
}

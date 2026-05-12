//! Resolver: walks the AST, builds the symbol table and lexical scopes,
//! and records every name reference site with the symbol it resolves to.
//!
//! This is a single-file, single-pass resolver intended as the foundation
//! for richer semantic analysis. It tracks:
//!
//! * Module-level declarations (`class`, `typealias`, `function`, property
//!   bindings) and imports.
//! * Class members (properties, methods) as children of their class.
//! * Type parameters on classes, typealiases, and methods — visible inside
//!   their respective bodies.
//! * Local bindings introduced by lambdas, `let`, `for (... in ...)`, and
//!   object-body lambda heads (`{ x -> ... }`).
//!
//! Member access (`foo.bar`) is _not_ resolved yet — that requires a type
//! checker. We still emit a hover entry for `foo` so the editor can show
//! info on the receiver.

use std::collections::HashMap;

use pkl_syntax::ast::*;
use pkl_syntax::span::Span;

use pkl_stdlib::{self, render_type_signature, StdlibType};

use crate::pretty::*;
use crate::scopes::{ScopeArena, ScopeId};
use crate::symbols::{Origin, Symbol, SymbolData, SymbolId, SymbolKind, SymbolTable};
use crate::types::{return_type_of_signature, Ty};

/// A site in the source where an identifier resolves to a [`SymbolId`].
/// Sorted by `span.start` after resolution so the LSP can binary-search.
#[derive(Clone, Debug)]
pub struct Reference {
    pub span: Span,
    pub symbol: SymbolId,
}

/// Output of a single-file resolver run.
pub struct Resolution {
    pub symbols: SymbolTable,
    pub references: Vec<Reference>,
    /// Map from a Span (identifier-position) to its SymbolId. Used by the
    /// LSP for definition / hover lookups in O(1).
    pub by_span_start: HashMap<u32, SymbolId>,
    /// Imports declared in this module, keyed by their local name (alias if
    /// present, else the path stem). The module graph turns these into
    /// resolved URIs once the loader is available.
    pub imports: HashMap<String, ImportInfo>,
}

/// Raw information about one `import "..." [as alias]` clause.
#[derive(Clone, Debug)]
pub struct ImportInfo {
    pub local_name: String,
    /// The string contents of the import literal, without surrounding
    /// quotes.
    pub raw_path: String,
    pub is_glob: bool,
    pub symbol_id: SymbolId,
    pub local_name_span: Span,
}

pub fn resolve_module(module: &Module) -> Resolution {
    let mut r = Resolver {
        symbols: SymbolTable::new(),
        scopes: ScopeArena::new(),
        references: Vec::new(),
        imports: HashMap::new(),
    };
    let module_scope = r.scopes.alloc(None);
    r.seed_stdlib(module_scope);
    r.declare_module(module, module_scope);
    r.references.sort_by_key(|reference| reference.span.start);
    let mut by_span_start = HashMap::with_capacity(r.references.len() + r.symbols.len());
    for reference in &r.references {
        by_span_start.insert(reference.span.start, reference.symbol);
    }
    for sym in r.symbols.iter() {
        if sym.origin.is_stdlib() {
            // Stdlib symbols share synthetic empty spans; only real
            // user-defined names participate in span-keyed lookup.
            continue;
        }
        by_span_start.insert(sym.name_span.start, sym.id);
    }
    Resolution {
        symbols: r.symbols,
        references: r.references,
        by_span_start,
        imports: r.imports,
    }
}

struct Resolver {
    symbols: SymbolTable,
    scopes: ScopeArena,
    references: Vec<Reference>,
    imports: HashMap<String, ImportInfo>,
}

impl Resolver {
    // ------------------------------------------------------------------
    // Helpers

    fn record_reference(&mut self, span: Span, symbol: SymbolId) {
        self.references.push(Reference { span, symbol });
    }

    fn resolve_ident_in_scope(&mut self, scope: ScopeId, name: &str, span: Span) {
        if let Some(sym) = self.scopes.lookup(scope, name) {
            self.record_reference(span, sym);
        }
    }

    fn insert_symbol(&mut self, scope: ScopeId, data: SymbolData) -> SymbolId {
        let name = data.name.clone();
        let id = self.symbols.insert(data);
        self.scopes.bind(scope, name, id);
        id
    }

    /// Register every built-in stdlib type and top-level function into the
    /// module scope. User declarations registered later in
    /// [`declare_module`] win because [`ScopeArena::bind`] overwrites prior
    /// entries — i.e. `class String { ... }` shadows the built-in `String`.
    fn seed_stdlib(&mut self, scope: ScopeId) {
        for t in pkl_stdlib::types() {
            self.insert_symbol(scope, stdlib_type_data(t));
        }
        for f in pkl_stdlib::functions() {
            // Top-level functions are callable, so their value type is a
            // function whose result is the parsed return type. We leave
            // params empty — the inferrer doesn't (yet) check arity.
            let declared_ty = Ty::Function {
                params: Vec::new(),
                ret: Box::new(return_type_of_signature(f.signature)),
            };
            self.insert_symbol(
                scope,
                SymbolData {
                    kind: SymbolKind::Method,
                    name: f.name.to_string(),
                    name_span: Span::EMPTY,
                    full_span: Span::EMPTY,
                    container: None,
                    signature: Some(f.signature.to_string()),
                    doc: Some(f.doc.to_string()),
                    modifiers: Vec::new(),
                    origin: Origin::Stdlib { module: f.module },
                    parent_class: None,
                    declared_ty,
                },
            );
        }
    }

    // ------------------------------------------------------------------
    // Module-level

    fn declare_module(&mut self, module: &Module, scope: ScopeId) {
        // First pass: register every top-level name so forward references work.
        for import in &module.imports {
            self.declare_import(import, scope);
        }
        // Two-step pass over items: declare first so order doesn't matter.
        let mut item_symbols: Vec<Option<SymbolId>> = Vec::with_capacity(module.items.len());
        for item in &module.items {
            item_symbols.push(self.declare_item_header(item, scope));
        }
        // Second pass: walk bodies and resolve references.
        for (item, container) in module.items.iter().zip(item_symbols.iter()) {
            self.resolve_item(item, scope, *container);
        }
    }

    fn declare_import(&mut self, import: &Import, scope: ScopeId) {
        // The local name is the alias if present, else derived from the path.
        let (local_name, local_span) = import
            .alias
            .as_ref()
            .map(|a| (a.name.clone(), a.span))
            .unwrap_or_else(|| {
                let derived = derive_import_name(&import.path.raw);
                (derived, import.path.span)
            });
        let symbol_id = self.insert_symbol(
            scope,
            SymbolData {
                kind: SymbolKind::Import {
                    is_glob: import.is_glob,
                },
                name: local_name.clone(),
                name_span: local_span,
                full_span: import.span,
                container: None,
                signature: Some(import.path.raw.clone()),
                doc: None,
                modifiers: Vec::new(),
                origin: Origin::User,
                parent_class: None,
                declared_ty: Ty::Module,
            },
        );
        let raw_path = strip_string_quotes(&import.path.raw);
        self.imports.insert(
            local_name.clone(),
            ImportInfo {
                local_name,
                raw_path,
                is_glob: import.is_glob,
                symbol_id,
                local_name_span: local_span,
            },
        );
    }

    fn declare_item_header(&mut self, item: &Item, scope: ScopeId) -> Option<SymbolId> {
        match item {
            Item::Class(c) => Some(self.insert_symbol(
                scope,
                SymbolData {
                    kind: SymbolKind::Class,
                    name: c.name.name.clone(),
                    name_span: c.name.span,
                    full_span: c.span,
                    container: None,
                    signature: Some(format_class_signature(c)),
                    doc: c.doc_comment.clone(),
                    modifiers: modifier_kinds(&c.modifiers),
                    origin: Origin::User,
                    parent_class: extends_class_name(c.extends.as_ref()),
                    declared_ty: Ty::Named {
                        name: c.name.name.clone(),
                        args: Vec::new(),
                    },
                },
            )),
            Item::TypeAlias(t) => Some(
                self.insert_symbol(
                    scope,
                    SymbolData {
                        kind: SymbolKind::TypeAlias,
                        name: t.name.name.clone(),
                        name_span: t.name.span,
                        full_span: t.span,
                        container: None,
                        signature: Some(format_typealias_signature(t)),
                        doc: t.doc_comment.clone(),
                        modifiers: modifier_kinds(&t.modifiers),
                        origin: Origin::User,
                        parent_class: None,
                        declared_ty: t
                            .aliased
                            .as_ref()
                            .map(Ty::from_type_ref)
                            .unwrap_or(Ty::Unknown),
                    },
                ),
            ),
            Item::Property(p) => Some(self.insert_symbol(
                scope,
                SymbolData {
                    kind: SymbolKind::Property,
                    name: p.name.name.clone(),
                    name_span: p.name.span,
                    full_span: p.span,
                    container: None,
                    signature: Some(format_property_signature(p)),
                    doc: p.doc_comment.clone(),
                    modifiers: modifier_kinds(&p.modifiers),
                    origin: Origin::User,
                    parent_class: None,
                    declared_ty: p.ty.as_ref().map(Ty::from_type_ref).unwrap_or(Ty::Unknown),
                },
            )),
            Item::Method(m) => Some(
                self.insert_symbol(
                    scope,
                    SymbolData {
                        kind: SymbolKind::Method,
                        name: m.name.name.clone(),
                        name_span: m.name.span,
                        full_span: m.span,
                        container: None,
                        signature: Some(format_method_signature(m)),
                        doc: m.doc_comment.clone(),
                        modifiers: modifier_kinds(&m.modifiers),
                        origin: Origin::User,
                        parent_class: None,
                        declared_ty: m
                            .return_type
                            .as_ref()
                            .map(Ty::from_type_ref)
                            .unwrap_or(Ty::Unknown),
                    },
                ),
            ),
            Item::Error(_) => None,
        }
    }

    fn resolve_item(&mut self, item: &Item, scope: ScopeId, container: Option<SymbolId>) {
        match item {
            Item::Class(c) => self.resolve_class(c, scope, container),
            Item::TypeAlias(t) => self.resolve_typealias(t, scope),
            Item::Property(p) => self.resolve_property(p, scope, None),
            Item::Method(m) => self.resolve_method(m, scope, None),
            Item::Error(_) => {}
        }
    }

    fn resolve_class(
        &mut self,
        c: &ClassDecl,
        parent_scope: ScopeId,
        _container: Option<SymbolId>,
    ) {
        let class_sym = self.scopes.lookup(parent_scope, &c.name.name);
        // Class scope holds type parameters and members.
        let class_scope = self.scopes.alloc(Some(parent_scope));
        self.declare_type_parameters(&c.type_parameters, class_scope);
        if let Some(ext) = &c.extends {
            self.resolve_type(ext, class_scope);
        }
        let mut member_ids: Vec<Option<SymbolId>> = Vec::new();
        if let Some(body) = &c.body {
            for member in &body.members {
                let id = match member {
                    ClassMember::Property(p) => Some(self.insert_symbol(
                        class_scope,
                        SymbolData {
                            kind: SymbolKind::Property,
                            name: p.name.name.clone(),
                            name_span: p.name.span,
                            full_span: p.span,
                            container: class_sym,
                            signature: Some(format_property_signature(p)),
                            doc: p.doc_comment.clone(),
                            modifiers: modifier_kinds(&p.modifiers),
                            origin: Origin::User,
                            parent_class: None,
                            declared_ty:
                                p.ty.as_ref().map(Ty::from_type_ref).unwrap_or(Ty::Unknown),
                        },
                    )),
                    ClassMember::Method(m) => Some(
                        self.insert_symbol(
                            class_scope,
                            SymbolData {
                                kind: SymbolKind::Method,
                                name: m.name.name.clone(),
                                name_span: m.name.span,
                                full_span: m.span,
                                container: class_sym,
                                signature: Some(format_method_signature(m)),
                                doc: m.doc_comment.clone(),
                                modifiers: modifier_kinds(&m.modifiers),
                                origin: Origin::User,
                                parent_class: None,
                                declared_ty: m
                                    .return_type
                                    .as_ref()
                                    .map(Ty::from_type_ref)
                                    .unwrap_or(Ty::Unknown),
                            },
                        ),
                    ),
                };
                member_ids.push(id);
            }
            for (member, _) in body.members.iter().zip(member_ids.iter()) {
                match member {
                    ClassMember::Property(p) => self.resolve_property(p, class_scope, class_sym),
                    ClassMember::Method(m) => self.resolve_method(m, class_scope, class_sym),
                }
            }
        }
    }

    fn resolve_typealias(&mut self, t: &TypeAliasDecl, parent_scope: ScopeId) {
        let scope = self.scopes.alloc(Some(parent_scope));
        self.declare_type_parameters(&t.type_parameters, scope);
        if let Some(aliased) = &t.aliased {
            self.resolve_type(aliased, scope);
        }
    }

    fn resolve_property(&mut self, p: &PropertyDecl, scope: ScopeId, _container: Option<SymbolId>) {
        for a in &p.annotations {
            self.resolve_annotation(a, scope);
        }
        if let Some(ty) = &p.ty {
            self.resolve_type(ty, scope);
        }
        match &p.value {
            Some(PropertyValue::Expr(e)) => self.resolve_expr(e, scope),
            Some(PropertyValue::ObjectBody(body)) => self.resolve_object_body(body, scope),
            None => {}
        }
    }

    fn resolve_method(
        &mut self,
        m: &MethodDecl,
        parent_scope: ScopeId,
        _container: Option<SymbolId>,
    ) {
        let scope = self.scopes.alloc(Some(parent_scope));
        for a in &m.annotations {
            self.resolve_annotation(a, scope);
        }
        self.declare_type_parameters(&m.type_parameters, scope);
        self.declare_parameters(&m.parameters, scope);
        for p in &m.parameters {
            if let Some(ty) = &p.ty {
                self.resolve_type(ty, scope);
            }
        }
        if let Some(ret) = &m.return_type {
            self.resolve_type(ret, scope);
        }
        if let Some(body) = &m.body {
            self.resolve_expr(body, scope);
        }
    }

    fn resolve_annotation(&mut self, a: &Annotation, scope: ScopeId) {
        if let Some(head) = a.name.segments.first() {
            self.resolve_ident_in_scope(scope, &head.name, head.span);
        }
        if let Some(body) = &a.body {
            self.resolve_object_body(body, scope);
        }
    }

    fn declare_type_parameters(&mut self, params: &[TypeParameter], scope: ScopeId) {
        for p in params {
            self.insert_symbol(
                scope,
                SymbolData {
                    kind: SymbolKind::TypeParameter,
                    name: p.name.name.clone(),
                    name_span: p.name.span,
                    full_span: p.span,
                    container: None,
                    signature: None,
                    doc: None,
                    modifiers: Vec::new(),
                    origin: Origin::User,
                    parent_class: None,
                    declared_ty: Ty::Named {
                        name: p.name.name.clone(),
                        args: Vec::new(),
                    },
                },
            );
        }
    }

    fn declare_parameters(&mut self, params: &[Parameter], scope: ScopeId) {
        for p in params {
            self.insert_symbol(
                scope,
                SymbolData {
                    kind: SymbolKind::Parameter,
                    name: p.name.name.clone(),
                    name_span: p.name.span,
                    full_span: p.span,
                    container: None,
                    signature: Some(format_parameter_signature(p)),
                    doc: None,
                    modifiers: Vec::new(),
                    origin: Origin::User,
                    parent_class: None,
                    declared_ty: p.ty.as_ref().map(Ty::from_type_ref).unwrap_or(Ty::Unknown),
                },
            );
        }
    }

    // ------------------------------------------------------------------
    // Types

    fn resolve_type(&mut self, ty: &TypeRef, scope: ScopeId) {
        match ty {
            TypeRef::Named {
                name, arguments, ..
            } => {
                // Resolve the head segment only. Qualified type names like
                // `acme.Foo` require module-graph resolution to look up the
                // member of the imported module; we'll add that with
                // cross-file imports later.
                if let Some(head) = name.segments.first() {
                    self.resolve_ident_in_scope(scope, &head.name, head.span);
                }
                for a in arguments {
                    self.resolve_type(a, scope);
                }
            }
            TypeRef::Nullable { inner, .. } | TypeRef::Parenthesized { inner, .. } => {
                self.resolve_type(inner, scope)
            }
            TypeRef::Union { members, .. } => {
                for m in members {
                    self.resolve_type(m, scope);
                }
            }
            TypeRef::Function {
                parameters, result, ..
            } => {
                for p in parameters {
                    self.resolve_type(p, scope);
                }
                self.resolve_type(result, scope);
            }
            TypeRef::StringLiteral(_)
            | TypeRef::Unknown(_)
            | TypeRef::Nothing(_)
            | TypeRef::Module(_)
            | TypeRef::Error { .. } => {}
        }
    }

    // ------------------------------------------------------------------
    // Expressions

    fn resolve_expr(&mut self, expr: &Expr, scope: ScopeId) {
        match expr {
            Expr::Literal(_) | Expr::SpecialIdent { .. } => {}
            Expr::Ident(id) => self.resolve_ident_in_scope(scope, &id.name, id.span),
            Expr::Paren { inner, .. } | Expr::NonNull { operand: inner, .. } => {
                self.resolve_expr(inner, scope)
            }
            Expr::Unary { operand, .. } => self.resolve_expr(operand, scope),
            Expr::Binary { lhs, rhs, .. } => {
                self.resolve_expr(lhs, scope);
                self.resolve_expr(rhs, scope);
            }
            Expr::TypeCheck { operand, ty, .. } | Expr::TypeCast { operand, ty, .. } => {
                self.resolve_expr(operand, scope);
                self.resolve_type(ty, scope);
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                self.resolve_expr(cond, scope);
                self.resolve_expr(then_branch, scope);
                self.resolve_expr(else_branch, scope);
            }
            Expr::Let {
                binding,
                value,
                body,
                ..
            } => {
                // The binding's value is evaluated in the outer scope, but
                // the body sees the new binding.
                self.resolve_expr(value, scope);
                let inner = self.scopes.alloc(Some(scope));
                if let Some(ty) = &binding.ty {
                    self.resolve_type(ty, scope);
                }
                self.insert_symbol(
                    inner,
                    SymbolData {
                        kind: SymbolKind::LetBinding,
                        name: binding.name.name.clone(),
                        name_span: binding.name.span,
                        full_span: binding.span,
                        container: None,
                        signature: Some(format_parameter_signature(binding)),
                        doc: None,
                        modifiers: Vec::new(),
                        origin: Origin::User,
                        parent_class: None,
                        declared_ty: binding
                            .ty
                            .as_ref()
                            .map(Ty::from_type_ref)
                            .unwrap_or(Ty::Unknown),
                    },
                );
                self.resolve_expr(body, inner);
            }
            Expr::Lambda {
                parameters, body, ..
            } => {
                let inner = self.scopes.alloc(Some(scope));
                self.declare_parameters(parameters, inner);
                for p in parameters {
                    if let Some(ty) = &p.ty {
                        self.resolve_type(ty, scope);
                    }
                }
                self.resolve_expr(body, inner);
            }
            Expr::Call {
                callee,
                type_args,
                args,
                ..
            } => {
                self.resolve_expr(callee, scope);
                for a in type_args {
                    self.resolve_type(a, scope);
                }
                for a in args {
                    self.resolve_expr(a, scope);
                }
            }
            Expr::Index {
                receiver, index, ..
            } => {
                self.resolve_expr(receiver, scope);
                self.resolve_expr(index, scope);
            }
            Expr::Member { receiver, .. } => {
                // We resolve the receiver but leave `.name` for the type
                // checker — its meaning depends on the receiver's type.
                self.resolve_expr(receiver, scope);
            }
            Expr::New { ty, body, .. } => {
                if let Some(ty) = ty {
                    self.resolve_type(ty, scope);
                }
                self.resolve_object_body(body, scope);
            }
            Expr::AmendsObject { base, body, .. } => {
                self.resolve_expr(base, scope);
                self.resolve_object_body(body, scope);
            }
            Expr::Throw { argument, .. }
            | Expr::Trace { argument, .. }
            | Expr::Read { argument, .. } => self.resolve_expr(argument, scope),
            Expr::Error { .. } => {}
        }
    }

    fn resolve_object_body(&mut self, body: &ObjectBody, parent_scope: ScopeId) {
        let scope = if body.parameters.is_empty() {
            parent_scope
        } else {
            let inner = self.scopes.alloc(Some(parent_scope));
            for p in &body.parameters {
                self.insert_symbol(
                    inner,
                    SymbolData {
                        kind: SymbolKind::ObjectParameter,
                        name: p.name.name.clone(),
                        name_span: p.name.span,
                        full_span: p.span,
                        container: None,
                        signature: Some(format_parameter_signature(p)),
                        doc: None,
                        modifiers: Vec::new(),
                        origin: Origin::User,
                        parent_class: None,
                        declared_ty: p.ty.as_ref().map(Ty::from_type_ref).unwrap_or(Ty::Unknown),
                    },
                );
                if let Some(ty) = &p.ty {
                    self.resolve_type(ty, parent_scope);
                }
            }
            inner
        };
        for member in &body.members {
            self.resolve_object_member(member, scope);
        }
    }

    fn resolve_object_member(&mut self, member: &ObjectMember, scope: ScopeId) {
        match member {
            ObjectMember::Property(p) => self.resolve_property(p, scope, None),
            ObjectMember::Method(m) => self.resolve_method(m, scope, None),
            ObjectMember::Element(e) => self.resolve_expr(e, scope),
            ObjectMember::Entry { key, value, .. } => {
                self.resolve_expr(key, scope);
                match value {
                    PropertyValue::Expr(e) => self.resolve_expr(e, scope),
                    PropertyValue::ObjectBody(body) => self.resolve_object_body(body, scope),
                }
            }
            ObjectMember::When {
                cond,
                then_body,
                else_body,
                ..
            } => {
                self.resolve_expr(cond, scope);
                self.resolve_object_body(then_body, scope);
                if let Some(b) = else_body {
                    self.resolve_object_body(b, scope);
                }
            }
            ObjectMember::For {
                bindings,
                iterable,
                body,
                ..
            } => {
                self.resolve_expr(iterable, scope);
                let inner = self.scopes.alloc(Some(scope));
                for b in bindings {
                    self.insert_symbol(
                        inner,
                        SymbolData {
                            kind: SymbolKind::ForBinding,
                            name: b.name.name.clone(),
                            name_span: b.name.span,
                            full_span: b.span,
                            container: None,
                            signature: Some(format_parameter_signature(b)),
                            doc: None,
                            modifiers: Vec::new(),
                            origin: Origin::User,
                            parent_class: None,
                            declared_ty: b
                                .ty
                                .as_ref()
                                .map(Ty::from_type_ref)
                                .unwrap_or(Ty::Unknown),
                        },
                    );
                    if let Some(ty) = &b.ty {
                        self.resolve_type(ty, scope);
                    }
                }
                self.resolve_object_body(body, inner);
            }
            ObjectMember::Spread { expr, .. } => self.resolve_expr(expr, scope),
        }
    }
}

// ----------------------------------------------------------------------
// Helpers

fn modifier_kinds(mods: &[Modifier]) -> Vec<ModifierKind> {
    mods.iter().map(|m| m.kind).collect()
}

/// Extract the bare parent-class name from an `extends T` clause, stripping
/// generics and qualifiers. `extends acme.Foo<Bar>` becomes `Some("Foo")`.
/// Returns `None` for anything we can't reduce to a simple class name.
fn extends_class_name(extends: Option<&TypeRef>) -> Option<String> {
    let ty = extends?;
    let TypeRef::Named { name, .. } = ty else {
        return None;
    };
    name.segments.last().map(|seg| seg.name.clone())
}

fn stdlib_type_data(t: &'static StdlibType) -> SymbolData {
    // The "value" of a stdlib type reference is the type itself, used as a
    // class/typealias literal. We model this via `Ty::Named` so that an
    // identifier reference like `String` keeps its identity for member
    // lookup.
    let declared_ty = Ty::from_name(t.name).unwrap_or_else(|| Ty::Named {
        name: t.name.to_string(),
        args: Vec::new(),
    });
    SymbolData {
        kind: SymbolKind::Class,
        name: t.name.to_string(),
        name_span: Span::EMPTY,
        full_span: Span::EMPTY,
        container: None,
        signature: Some(render_type_signature(t)),
        doc: Some(t.doc.to_string()),
        modifiers: Vec::new(),
        origin: Origin::Stdlib { module: t.module },
        parent_class: None,
        declared_ty,
    }
}

/// Derive a sensible local name for an import path that did not specify
/// `as <ident>`. Pkl's normal rule is "last path component without extension"
/// — close enough for hover and goto-def.
fn derive_import_name(raw_path: &str) -> String {
    let trimmed = strip_string_quotes(raw_path);
    let last = trimmed.rsplit('/').next().unwrap_or(&trimmed);
    let stem = match last.find('.') {
        Some(idx) => &last[..idx],
        None => last,
    };
    stem.to_string()
}

/// Strip the surrounding `"..."` (and the `#`-padding on Pkl's
/// custom-delimited strings) from a string literal's raw text. Returns the
/// inner contents unchanged if the input isn't quoted.
fn strip_string_quotes(raw: &str) -> String {
    let mut s = raw;
    // Drop leading `#`s, then a leading `"` or `"""`. Same for the trailing
    // side, mirrored.
    let lead_hashes = s.bytes().take_while(|&b| b == b'#').count();
    s = &s[lead_hashes..];
    if s.starts_with("\"\"\"") && s.ends_with("\"\"\"") && s.len() >= 6 {
        s = &s[3..s.len() - 3];
    } else if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s = &s[1..s.len() - 1];
    }
    // Trim trailing `#`s that matched the lead.
    let trim_trail = s
        .bytes()
        .rev()
        .take_while(|&b| b == b'#')
        .count()
        .min(lead_hashes);
    s = &s[..s.len() - trim_trail];
    s.to_string()
}

impl Resolution {
    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        self.symbols.get(id)
    }

    /// Find a SymbolId associated with a span at the given byte offset.
    /// Both definition name-spans and reference spans are considered.
    pub fn symbol_at_offset(&self, offset: u32) -> Option<SymbolId> {
        // User-defined definitions are short ranges; check those exactly
        // first. Stdlib symbols carry synthetic empty spans and never
        // participate in offset-based lookup — references to them come in
        // via `self.references` instead.
        for s in self.symbols.iter() {
            if s.origin.is_stdlib() {
                continue;
            }
            if s.name_span.touches(offset) {
                return Some(s.id);
            }
        }
        // References sorted by span.start — linear scan is fine for now;
        // we'll move to binary search if it becomes hot.
        for r in &self.references {
            if r.span.touches(offset) {
                return Some(r.symbol);
            }
            if r.span.start > offset {
                break;
            }
        }
        None
    }
}

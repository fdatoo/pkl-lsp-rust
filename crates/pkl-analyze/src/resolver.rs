//! Resolver: walks the CST, builds the symbol table and lexical scopes,
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

use pkl_syntax::cst::{
    self, ident_text, significant_span, token_span, AstNode, ClassDecl, ClassMember, Expr,
    ImportClause, Item, MethodDecl, Module, ObjectBody, ObjectMember, Parameter, PropertyDecl,
    PropertyValue, Type, TypeAliasDecl, TypeParameter,
};
use pkl_syntax::span::Span;
use pkl_syntax::SyntaxToken;

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
        for import in module.imports() {
            self.declare_import(&import, scope);
        }
        // Two-step pass over items: declare first so order doesn't matter.
        let items: Vec<Item> = module.items().collect();
        let mut item_symbols: Vec<Option<SymbolId>> = Vec::with_capacity(items.len());
        for item in &items {
            item_symbols.push(self.declare_item_header(item, scope));
        }
        // Second pass: walk bodies and resolve references.
        for (item, container) in items.iter().zip(item_symbols.iter()) {
            self.resolve_item(item, scope, *container);
        }
    }

    fn declare_import(&mut self, import: &ImportClause, scope: ScopeId) {
        let path_tok = match import.path() {
            Some(t) => t,
            None => return,
        };
        let path_raw = path_tok.text().to_string();
        let import_span = significant_span(import.syntax());
        let alias_tok = import.alias();

        let (local_name, local_span) = match alias_tok.as_ref() {
            Some(a) => (ident_text(a), token_span(a)),
            None => (derive_import_name(&path_raw), token_span(&path_tok)),
        };
        let symbol_id = self.insert_symbol(
            scope,
            SymbolData {
                kind: SymbolKind::Import {
                    is_glob: import.is_glob(),
                },
                name: local_name.clone(),
                name_span: local_span,
                full_span: import_span,
                container: None,
                signature: Some(path_raw.clone()),
                doc: None,
                modifiers: Vec::new(),
                origin: Origin::User,
                parent_class: None,
                declared_ty: Ty::Module,
            },
        );
        let raw_path = strip_string_quotes(&path_raw);
        self.imports.insert(
            local_name.clone(),
            ImportInfo {
                local_name,
                raw_path,
                is_glob: import.is_glob(),
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
                    name: name_token_text(c.name()),
                    name_span: token_span_or_empty(c.name()),
                    full_span: significant_span(c.syntax()),
                    container: None,
                    signature: Some(format_class_signature(c)),
                    doc: c.doc_comment(),
                    modifiers: modifier_kinds(c),
                    origin: Origin::User,
                    parent_class: extends_class_name(c.extends().as_ref()),
                    declared_ty: Ty::Named {
                        name: name_token_text(c.name()),
                        args: Vec::new(),
                    },
                },
            )),
            Item::TypeAlias(t) => Some(
                self.insert_symbol(
                    scope,
                    SymbolData {
                        kind: SymbolKind::TypeAlias,
                        name: name_token_text(t.name()),
                        name_span: token_span_or_empty(t.name()),
                        full_span: significant_span(t.syntax()),
                        container: None,
                        signature: Some(format_typealias_signature(t)),
                        doc: t.doc_comment(),
                        modifiers: modifier_kinds(t),
                        origin: Origin::User,
                        parent_class: None,
                        declared_ty: t
                            .aliased_type()
                            .as_ref()
                            .map(Ty::from_cst_type)
                            .unwrap_or(Ty::Unknown),
                    },
                ),
            ),
            Item::Property(p) => Some(
                self.insert_symbol(
                    scope,
                    SymbolData {
                        kind: SymbolKind::Property,
                        name: name_token_text(p.name()),
                        name_span: token_span_or_empty(p.name()),
                        full_span: significant_span(p.syntax()),
                        container: None,
                        signature: Some(format_property_signature(p.syntax())),
                        doc: p.doc_comment(),
                        modifiers: modifier_kinds(p),
                        origin: Origin::User,
                        parent_class: None,
                        declared_ty: p
                            .ty()
                            .as_ref()
                            .map(Ty::from_cst_type)
                            .unwrap_or(Ty::Unknown),
                    },
                ),
            ),
            Item::Method(m) => Some(
                self.insert_symbol(
                    scope,
                    SymbolData {
                        kind: SymbolKind::Method,
                        name: name_token_text(m.name()),
                        name_span: token_span_or_empty(m.name()),
                        full_span: significant_span(m.syntax()),
                        container: None,
                        signature: Some(format_method_signature(m)),
                        doc: m.doc_comment(),
                        modifiers: modifier_kinds(m),
                        origin: Origin::User,
                        parent_class: None,
                        declared_ty: m
                            .return_type()
                            .as_ref()
                            .map(Ty::from_cst_type)
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
        let class_name = name_token_text(c.name());
        let class_sym = self.scopes.lookup(parent_scope, &class_name);
        // Class scope holds type parameters and members.
        let class_scope = self.scopes.alloc(Some(parent_scope));
        let type_params: Vec<TypeParameter> = c
            .type_parameters()
            .map(|tp| tp.parameters().collect())
            .unwrap_or_default();
        self.declare_type_parameters(&type_params, class_scope);
        if let Some(ext) = &c.extends() {
            self.resolve_type(ext, class_scope);
        }
        let members: Vec<ClassMember> = c.body().map(|b| b.members().collect()).unwrap_or_default();
        let mut member_ids: Vec<Option<SymbolId>> = Vec::new();
        for member in &members {
            let id = match member {
                ClassMember::Property(p) => Some(
                    self.insert_symbol(
                        class_scope,
                        SymbolData {
                            kind: SymbolKind::Property,
                            name: name_token_text(p.name()),
                            name_span: token_span_or_empty(p.name()),
                            full_span: significant_span(p.syntax()),
                            container: class_sym,
                            signature: Some(format_property_signature(p.syntax())),
                            doc: p.doc_comment(),
                            modifiers: modifier_kinds(p),
                            origin: Origin::User,
                            parent_class: None,
                            declared_ty: p
                                .ty()
                                .as_ref()
                                .map(Ty::from_cst_type)
                                .unwrap_or(Ty::Unknown),
                        },
                    ),
                ),
                ClassMember::Method(m) => Some(
                    self.insert_symbol(
                        class_scope,
                        SymbolData {
                            kind: SymbolKind::Method,
                            name: name_token_text(m.name()),
                            name_span: token_span_or_empty(m.name()),
                            full_span: significant_span(m.syntax()),
                            container: class_sym,
                            signature: Some(format_class_method_signature(m)),
                            doc: m.doc_comment(),
                            modifiers: modifier_kinds(m),
                            origin: Origin::User,
                            parent_class: None,
                            declared_ty: m
                                .return_type()
                                .as_ref()
                                .map(Ty::from_cst_type)
                                .unwrap_or(Ty::Unknown),
                        },
                    ),
                ),
            };
            member_ids.push(id);
        }
        for (member, _) in members.iter().zip(member_ids.iter()) {
            match member {
                ClassMember::Property(p) => self.resolve_class_property(p, class_scope, class_sym),
                ClassMember::Method(m) => self.resolve_class_method(m, class_scope, class_sym),
            }
        }
    }

    fn resolve_typealias(&mut self, t: &TypeAliasDecl, parent_scope: ScopeId) {
        let scope = self.scopes.alloc(Some(parent_scope));
        let type_params: Vec<TypeParameter> = t
            .type_parameters()
            .map(|tp| tp.parameters().collect())
            .unwrap_or_default();
        self.declare_type_parameters(&type_params, scope);
        if let Some(aliased) = t.aliased_type() {
            self.resolve_type(&aliased, scope);
        }
    }

    fn resolve_property(&mut self, p: &PropertyDecl, scope: ScopeId, _container: Option<SymbolId>) {
        for a in p.annotations() {
            self.resolve_annotation(&a, scope);
        }
        if let Some(ty) = p.ty() {
            self.resolve_type(&ty, scope);
        }
        match p.value() {
            Some(PropertyValue::Expr(e)) => self.resolve_expr(&e, scope),
            Some(PropertyValue::ObjectBody(body)) => self.resolve_object_body(&body, scope),
            None => {}
        }
    }

    fn resolve_class_property(
        &mut self,
        p: &cst::ClassPropertyDecl,
        scope: ScopeId,
        _container: Option<SymbolId>,
    ) {
        for a in p.annotations() {
            self.resolve_annotation(&a, scope);
        }
        if let Some(ty) = p.ty() {
            self.resolve_type(&ty, scope);
        }
        match p.value() {
            Some(PropertyValue::Expr(e)) => self.resolve_expr(&e, scope),
            Some(PropertyValue::ObjectBody(body)) => self.resolve_object_body(&body, scope),
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
        for a in m.annotations() {
            self.resolve_annotation(&a, scope);
        }
        let type_params: Vec<TypeParameter> = m
            .type_parameters()
            .map(|tp| tp.parameters().collect())
            .unwrap_or_default();
        self.declare_type_parameters(&type_params, scope);
        let params: Vec<Parameter> = m
            .parameters()
            .map(|pl| pl.parameters().collect())
            .unwrap_or_default();
        self.declare_parameters(&params, scope);
        for p in &params {
            if let Some(ty) = p.ty() {
                self.resolve_type(&ty, scope);
            }
        }
        if let Some(ret) = m.return_type() {
            self.resolve_type(&ret, scope);
        }
        if let Some(body) = m.body() {
            self.resolve_expr(&body, scope);
        }
    }

    fn resolve_class_method(
        &mut self,
        m: &cst::ClassMethodDecl,
        parent_scope: ScopeId,
        _container: Option<SymbolId>,
    ) {
        let scope = self.scopes.alloc(Some(parent_scope));
        for a in m.annotations() {
            self.resolve_annotation(&a, scope);
        }
        let type_params: Vec<TypeParameter> = m
            .type_parameters()
            .map(|tp| tp.parameters().collect())
            .unwrap_or_default();
        self.declare_type_parameters(&type_params, scope);
        let params: Vec<Parameter> = m
            .parameters()
            .map(|pl| pl.parameters().collect())
            .unwrap_or_default();
        self.declare_parameters(&params, scope);
        for p in &params {
            if let Some(ty) = p.ty() {
                self.resolve_type(&ty, scope);
            }
        }
        if let Some(ret) = m.return_type() {
            self.resolve_type(&ret, scope);
        }
        if let Some(body) = m.body() {
            self.resolve_expr(&body, scope);
        }
    }

    fn resolve_object_method(&mut self, m: &cst::ObjectMethod, parent_scope: ScopeId) {
        let scope = self.scopes.alloc(Some(parent_scope));
        for a in m.annotations() {
            self.resolve_annotation(&a, scope);
        }
        let type_params: Vec<TypeParameter> = m
            .type_parameters()
            .map(|tp| tp.parameters().collect())
            .unwrap_or_default();
        self.declare_type_parameters(&type_params, scope);
        let params: Vec<Parameter> = m
            .parameters()
            .map(|pl| pl.parameters().collect())
            .unwrap_or_default();
        self.declare_parameters(&params, scope);
        for p in &params {
            if let Some(ty) = p.ty() {
                self.resolve_type(&ty, scope);
            }
        }
        if let Some(ret) = m.return_type() {
            self.resolve_type(&ret, scope);
        }
        if let Some(body) = m.body() {
            self.resolve_expr(&body, scope);
        }
    }

    fn resolve_object_property(&mut self, p: &cst::ObjectProperty, scope: ScopeId) {
        for a in p.annotations() {
            self.resolve_annotation(&a, scope);
        }
        if let Some(ty) = p.ty() {
            self.resolve_type(&ty, scope);
        }
        match p.value() {
            Some(PropertyValue::Expr(e)) => self.resolve_expr(&e, scope),
            Some(PropertyValue::ObjectBody(body)) => self.resolve_object_body(&body, scope),
            None => {}
        }
    }

    fn resolve_annotation(&mut self, a: &cst::Annotation, scope: ScopeId) {
        if let Some(name) = a.name() {
            if let Some(head) = name.segments().next() {
                self.resolve_ident_in_scope(scope, &ident_text(&head), token_span(&head));
            }
        }
        if let Some(body) = a.body() {
            self.resolve_object_body(&body, scope);
        }
    }

    fn declare_type_parameters(&mut self, params: &[TypeParameter], scope: ScopeId) {
        for p in params {
            let name = name_token_text(p.name());
            self.insert_symbol(
                scope,
                SymbolData {
                    kind: SymbolKind::TypeParameter,
                    name: name.clone(),
                    name_span: token_span_or_empty(p.name()),
                    full_span: significant_span(p.syntax()),
                    container: None,
                    signature: None,
                    doc: None,
                    modifiers: Vec::new(),
                    origin: Origin::User,
                    parent_class: None,
                    declared_ty: Ty::Named {
                        name,
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
                    name: name_token_text(p.name()),
                    name_span: token_span_or_empty(p.name()),
                    full_span: significant_span(p.syntax()),
                    container: None,
                    signature: Some(format_parameter_signature(p)),
                    doc: None,
                    modifiers: Vec::new(),
                    origin: Origin::User,
                    parent_class: None,
                    declared_ty: p
                        .ty()
                        .as_ref()
                        .map(Ty::from_cst_type)
                        .unwrap_or(Ty::Unknown),
                },
            );
        }
    }

    // ------------------------------------------------------------------
    // Types

    fn resolve_type(&mut self, ty: &Type, scope: ScopeId) {
        match ty {
            Type::Named(n) => {
                if let Some(name) = n.name() {
                    // Resolve the head segment only. Qualified type names
                    // require module-graph resolution to look up the
                    // member of the imported module; we'll add that with
                    // cross-file imports later.
                    if let Some(head) = name.segments().next() {
                        self.resolve_ident_in_scope(scope, &ident_text(&head), token_span(&head));
                    }
                }
                if let Some(args) = n.type_arguments() {
                    for a in args.arguments() {
                        self.resolve_type(&a, scope);
                    }
                }
            }
            Type::Nullable(n) => {
                if let Some(inner) = n.inner() {
                    self.resolve_type(&inner, scope);
                }
            }
            Type::Constrained(c) => {
                if let Some(inner) = c.inner() {
                    self.resolve_type(&inner, scope);
                }
                for constraint in c.constraints() {
                    self.resolve_expr(&constraint, scope);
                }
            }
            Type::Default(d) => {
                if let Some(inner) = d.inner() {
                    self.resolve_type(&inner, scope);
                }
            }
            Type::Parenthesized(p) => {
                if let Some(inner) = p.inner() {
                    self.resolve_type(&inner, scope);
                }
            }
            Type::Union(u) => {
                for m in u.members() {
                    self.resolve_type(&m, scope);
                }
            }
            Type::Function(f) => {
                for p in f.parameters() {
                    self.resolve_type(&p, scope);
                }
                if let Some(result) = f.result() {
                    self.resolve_type(&result, scope);
                }
            }
            Type::StringLiteral(_)
            | Type::Unknown(_)
            | Type::Nothing(_)
            | Type::Module(_)
            | Type::Error(_) => {}
        }
    }

    // ------------------------------------------------------------------
    // Expressions

    fn resolve_expr(&mut self, expr: &Expr, scope: ScopeId) {
        match expr {
            Expr::Literal(_) => {}
            Expr::InterpolatedString(s) => {
                for hole in s.interpolations() {
                    if let Some(inner) = hole.expr() {
                        self.resolve_expr(&inner, scope);
                    }
                }
            }
            Expr::Ident(id) => {
                if let Some(tok) = id.token() {
                    // Skip special identifiers (this/super/outer/module)
                    if id.special().is_some() {
                        return;
                    }
                    self.resolve_ident_in_scope(scope, &ident_text(&tok), token_span(&tok));
                }
            }
            Expr::Paren(p) => {
                if let Some(inner) = p.inner() {
                    self.resolve_expr(&inner, scope);
                }
            }
            Expr::NonNull(n) => {
                if let Some(operand) = n.operand() {
                    self.resolve_expr(&operand, scope);
                }
            }
            Expr::Unary(u) => {
                if let Some(operand) = u.operand() {
                    self.resolve_expr(&operand, scope);
                }
            }
            Expr::Binary(b) => {
                if let Some(lhs) = b.lhs() {
                    self.resolve_expr(&lhs, scope);
                }
                if let Some(rhs) = b.rhs() {
                    self.resolve_expr(&rhs, scope);
                }
            }
            Expr::NullCoalesce(n) => {
                if let Some(lhs) = n.lhs() {
                    self.resolve_expr(&lhs, scope);
                }
                if let Some(rhs) = n.rhs() {
                    self.resolve_expr(&rhs, scope);
                }
            }
            Expr::TypeCheck(t) => {
                if let Some(operand) = t.operand() {
                    self.resolve_expr(&operand, scope);
                }
                if let Some(ty) = t.ty() {
                    self.resolve_type(&ty, scope);
                }
            }
            Expr::TypeCast(t) => {
                if let Some(operand) = t.operand() {
                    self.resolve_expr(&operand, scope);
                }
                if let Some(ty) = t.ty() {
                    self.resolve_type(&ty, scope);
                }
            }
            Expr::If(i) => {
                if let Some(c) = i.condition() {
                    self.resolve_expr(&c, scope);
                }
                if let Some(t) = i.then_branch() {
                    self.resolve_expr(&t, scope);
                }
                if let Some(e) = i.else_branch() {
                    self.resolve_expr(&e, scope);
                }
            }
            Expr::Let(l) => {
                // The binding's value is evaluated in the outer scope, but
                // the body sees the new binding.
                if let Some(value) = l.value() {
                    self.resolve_expr(&value, scope);
                }
                let inner = self.scopes.alloc(Some(scope));
                if let Some(binding) = l.binding() {
                    if let Some(ty) = binding.ty() {
                        self.resolve_type(&ty, scope);
                    }
                    self.insert_symbol(
                        inner,
                        SymbolData {
                            kind: SymbolKind::LetBinding,
                            name: name_token_text(binding.name()),
                            name_span: token_span_or_empty(binding.name()),
                            full_span: significant_span(binding.syntax()),
                            container: None,
                            signature: Some(format_parameter_signature(&binding)),
                            doc: None,
                            modifiers: Vec::new(),
                            origin: Origin::User,
                            parent_class: None,
                            declared_ty: binding
                                .ty()
                                .as_ref()
                                .map(Ty::from_cst_type)
                                .unwrap_or(Ty::Unknown),
                        },
                    );
                }
                if let Some(body) = l.body() {
                    self.resolve_expr(&body, inner);
                }
            }
            Expr::Lambda(lam) => {
                let inner = self.scopes.alloc(Some(scope));
                let params: Vec<Parameter> = lam
                    .parameters()
                    .map(|pl| pl.parameters().collect())
                    .unwrap_or_default();
                self.declare_parameters(&params, inner);
                for p in &params {
                    if let Some(ty) = p.ty() {
                        self.resolve_type(&ty, scope);
                    }
                }
                if let Some(body) = lam.body() {
                    self.resolve_expr(&body, inner);
                }
            }
            Expr::Call(c) => {
                if let Some(callee) = c.callee() {
                    self.resolve_expr(&callee, scope);
                }
                for a in c.args() {
                    self.resolve_expr(&a, scope);
                }
            }
            Expr::Index(i) => {
                if let Some(receiver) = i.receiver() {
                    self.resolve_expr(&receiver, scope);
                }
                if let Some(idx) = i.index() {
                    self.resolve_expr(&idx, scope);
                }
            }
            Expr::Member(m) => {
                // We resolve the receiver but leave `.name` for the type
                // checker — its meaning depends on the receiver's type.
                if let Some(receiver) = m.receiver() {
                    self.resolve_expr(&receiver, scope);
                }
            }
            Expr::New(n) => {
                if let Some(ty) = n.ty() {
                    self.resolve_type(&ty, scope);
                }
                if let Some(body) = n.body() {
                    self.resolve_object_body(&body, scope);
                }
            }
            Expr::Amends(a) => {
                if let Some(base) = a.base() {
                    self.resolve_expr(&base, scope);
                }
                if let Some(body) = a.body() {
                    self.resolve_object_body(&body, scope);
                }
            }
            Expr::Throw(t) => {
                if let Some(arg) = t.argument() {
                    self.resolve_expr(&arg, scope);
                }
            }
            Expr::Trace(t) => {
                if let Some(arg) = t.argument() {
                    self.resolve_expr(&arg, scope);
                }
            }
            Expr::Read(r) => {
                if let Some(arg) = r.argument() {
                    self.resolve_expr(&arg, scope);
                }
            }
            Expr::Import(i) => {
                if let Some(arg) = i.argument() {
                    self.resolve_expr(&arg, scope);
                }
            }
            Expr::Error(_) => {}
        }
    }

    fn resolve_object_body(&mut self, body: &ObjectBody, parent_scope: ScopeId) {
        let params: Vec<Parameter> = body
            .parameters()
            .map(|pl| pl.parameters().collect())
            .unwrap_or_default();
        let scope = if params.is_empty() {
            parent_scope
        } else {
            let inner = self.scopes.alloc(Some(parent_scope));
            for p in &params {
                self.insert_symbol(
                    inner,
                    SymbolData {
                        kind: SymbolKind::ObjectParameter,
                        name: name_token_text(p.name()),
                        name_span: token_span_or_empty(p.name()),
                        full_span: significant_span(p.syntax()),
                        container: None,
                        signature: Some(format_parameter_signature(p)),
                        doc: None,
                        modifiers: Vec::new(),
                        origin: Origin::User,
                        parent_class: None,
                        declared_ty: p
                            .ty()
                            .as_ref()
                            .map(Ty::from_cst_type)
                            .unwrap_or(Ty::Unknown),
                    },
                );
                if let Some(ty) = p.ty() {
                    self.resolve_type(&ty, parent_scope);
                }
            }
            inner
        };
        for member in body.members() {
            self.resolve_object_member(&member, scope);
        }
    }

    fn resolve_object_member(&mut self, member: &ObjectMember, scope: ScopeId) {
        match member {
            ObjectMember::Property(p) => self.resolve_object_property(p, scope),
            ObjectMember::Method(m) => self.resolve_object_method(m, scope),
            ObjectMember::Element(e) => {
                if let Some(expr) = e.expr() {
                    self.resolve_expr(&expr, scope);
                }
            }
            ObjectMember::Entry(e) => {
                if let Some(key) = e.key() {
                    self.resolve_expr(&key, scope);
                }
                match e.value() {
                    Some(PropertyValue::Expr(expr)) => self.resolve_expr(&expr, scope),
                    Some(PropertyValue::ObjectBody(body)) => self.resolve_object_body(&body, scope),
                    None => {}
                }
            }
            ObjectMember::When(w) => {
                if let Some(c) = w.condition() {
                    self.resolve_expr(&c, scope);
                }
                if let Some(then_body) = w.then_body() {
                    self.resolve_object_body(&then_body, scope);
                }
                if let Some(else_body) = w.else_body() {
                    self.resolve_object_body(&else_body, scope);
                }
            }
            ObjectMember::For(f) => {
                if let Some(iter) = f.iterable() {
                    self.resolve_expr(&iter, scope);
                }
                let inner = self.scopes.alloc(Some(scope));
                let bindings: Vec<Parameter> = f.bindings().collect();
                for b in &bindings {
                    self.insert_symbol(
                        inner,
                        SymbolData {
                            kind: SymbolKind::ForBinding,
                            name: name_token_text(b.name()),
                            name_span: token_span_or_empty(b.name()),
                            full_span: significant_span(b.syntax()),
                            container: None,
                            signature: Some(format_parameter_signature(b)),
                            doc: None,
                            modifiers: Vec::new(),
                            origin: Origin::User,
                            parent_class: None,
                            declared_ty: b
                                .ty()
                                .as_ref()
                                .map(Ty::from_cst_type)
                                .unwrap_or(Ty::Unknown),
                        },
                    );
                    if let Some(ty) = b.ty() {
                        self.resolve_type(&ty, scope);
                    }
                }
                if let Some(body) = f.body() {
                    self.resolve_object_body(&body, inner);
                }
            }
            ObjectMember::Spread(s) => {
                if let Some(expr) = s.expr() {
                    self.resolve_expr(&expr, scope);
                }
            }
        }
    }
}

// ----------------------------------------------------------------------
// Helpers

fn name_token_text(t: Option<SyntaxToken>) -> String {
    t.map(|t| ident_text(&t)).unwrap_or_default()
}

fn token_span_or_empty(t: Option<SyntaxToken>) -> Span {
    t.map(|t| token_span(&t)).unwrap_or(Span::EMPTY)
}

/// Collect modifier kinds off any node with `modifiers()`.
trait HasModifiers {
    fn modifier_kinds(&self) -> Vec<cst::ModifierKind>;
}

macro_rules! impl_has_modifiers {
    ($($t:ty),*) => {
        $(
            impl HasModifiers for $t {
                fn modifier_kinds(&self) -> Vec<cst::ModifierKind> {
                    self.modifiers().filter_map(|m| m.kind()).collect()
                }
            }
        )*
    };
}

impl_has_modifiers!(
    cst::ClassDecl,
    cst::TypeAliasDecl,
    cst::PropertyDecl,
    cst::ClassPropertyDecl,
    cst::ObjectProperty,
    cst::MethodDecl,
    cst::ClassMethodDecl,
    cst::ObjectMethod
);

fn modifier_kinds<T: HasModifiers>(node: &T) -> Vec<cst::ModifierKind> {
    node.modifier_kinds()
}

/// Extract the bare parent-class name from an `extends T` clause, stripping
/// generics and qualifiers. `extends acme.Foo<Bar>` becomes `Some("Foo")`.
/// Returns `None` for anything we can't reduce to a simple class name.
fn extends_class_name(extends: Option<&Type>) -> Option<String> {
    let ty = extends?;
    let Type::Named(n) = ty else {
        return None;
    };
    let name = n.name()?;
    name.segments().last().map(|seg| ident_text(&seg))
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

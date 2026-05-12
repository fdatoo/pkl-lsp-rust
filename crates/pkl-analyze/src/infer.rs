//! Single-pass type inference for Pkl expressions.
//!
//! The inferrer walks the CST and assigns a [`Ty`] to every expression and
//! property/method body. It also resolves member access against the stdlib
//! catalogue, producing a [`MemberRef`] entry the LSP can use for hover and
//! goto-definition on `expr.name` forms.
//!
//! This is a pragmatic first pass: we do not do generic substitution,
//! flow-sensitive narrowing, or cross-file resolution. Anything we don't
//! know becomes [`Ty::Unknown`].

use std::collections::{HashMap, HashSet};

use pkl_stdlib::{StdlibMember, StdlibType};
use pkl_syntax::cst::{
    self, ident_text, significant_span, token_span, AstNode, BinaryOp, ClassMember, Expr, Item,
    LiteralExpr, LiteralKind, MethodDecl, Module, ObjectBody, ObjectMember, Parameter,
    PropertyDecl, PropertyValue, QualifiedName, ReadKind, SpecialIdentKind, Type, UnaryOp,
};
use pkl_syntax::span::Span;
use pkl_syntax::{SyntaxDiagnostic, SyntaxToken};

use crate::resolver::Resolution;
use crate::subtyping::is_subtype;
use crate::symbols::SymbolId;
use crate::types::{parse_signature, unify, ParsedSignature, Ty};

/// Output of the inference pass.
pub struct Inference {
    /// Type assigned to each expression, keyed by `expr.span().start`.
    pub expr_types: HashMap<u32, Ty>,
    /// Same data keyed by `expr.span().end`, used by completion to find
    /// the receiver expression that ends right at a `.` cursor.
    pub expr_types_by_end: HashMap<u32, Ty>,
    /// Member-access resolution, keyed by the member-name span's start.
    pub member_refs: HashMap<u32, MemberRef>,
    /// Type-mismatch diagnostics discovered by the inferrer (e.g. assigning
    /// `Int` to a `String` property). Only emitted when both sides have a
    /// non-`Unknown` type so the noise floor stays low.
    pub diagnostics: Vec<SyntaxDiagnostic>,
}

impl Inference {
    /// Type of the expression beginning at byte `offset`, if known.
    pub fn type_of(&self, offset: u32) -> Option<&Ty> {
        self.expr_types.get(&offset)
    }

    /// Type of the expression *ending* at byte `offset` (i.e. whose
    /// last byte is at `offset - 1`).
    pub fn type_ending_at(&self, offset: u32) -> Option<&Ty> {
        self.expr_types_by_end.get(&offset)
    }

    /// Member resolution at the given byte offset. The offset must be the
    /// start of the member-name span — the LSP layer narrows the cursor
    /// to the identifier under it before calling.
    pub fn member_ref_at(&self, offset: u32) -> Option<&MemberRef> {
        // Linear scan over a typically tiny set; the cursor offset rarely
        // matches a span start exactly, so callers usually walk the set
        // to find one that "touches" the position.
        self.member_refs.get(&offset)
    }

    /// Find the [`MemberRef`] whose name-span contains the given offset.
    pub fn member_ref_touching(&self, offset: u32) -> Option<&MemberRef> {
        self.member_refs
            .values()
            .find(|r| r.member_name_span.touches(offset))
    }
}

/// A resolved member access: `expr.name` where `expr` has a known type and
/// `name` is a property or method on that type (or one of its ancestors).
///
/// Either the stdlib pair (`stdlib_type` + `stdlib_member`) is set, the
/// user pair (`user_class` + `user_member`) is set, or both are `None`
/// when the receiver's type was too unknown to resolve.
#[derive(Clone, Debug)]
pub struct MemberRef {
    pub receiver_ty: Ty,
    /// Span of the receiver expression. The LSP layer uses this to walk
    /// back to the receiver's symbol (e.g. an import alias) for cross-file
    /// resolution.
    pub receiver_span: Span,
    pub member_name: String,
    pub member_name_span: Span,
    pub stdlib_member: Option<&'static StdlibMember>,
    /// The stdlib type the member was resolved against (after walking up
    /// the inheritance chain).
    pub stdlib_type: Option<&'static StdlibType>,
    /// User-defined class whose members were searched.
    pub user_class: Option<SymbolId>,
    /// Resolved user-defined member symbol.
    pub user_member: Option<SymbolId>,
}

impl MemberRef {
    pub fn is_resolved(&self) -> bool {
        self.stdlib_member.is_some() || self.user_member.is_some()
    }
}

pub fn infer_module(module: &Module, resolution: &Resolution) -> Inference {
    let mut inf = Inferrer {
        resolution,
        expr_types: HashMap::new(),
        expr_types_by_end: HashMap::new(),
        member_refs: HashMap::new(),
        diagnostics: Vec::new(),
        expected_context: None,
        current_class: None,
        narrowings: HashMap::new(),
        inferred_property_types: HashMap::new(),
    };
    inf.walk_module(module);
    Inference {
        expr_types: inf.expr_types,
        expr_types_by_end: inf.expr_types_by_end,
        member_refs: inf.member_refs,
        diagnostics: inf.diagnostics,
    }
}

struct Inferrer<'a> {
    resolution: &'a Resolution,
    expr_types: HashMap<u32, Ty>,
    expr_types_by_end: HashMap<u32, Ty>,
    member_refs: HashMap<u32, MemberRef>,
    diagnostics: Vec<SyntaxDiagnostic>,
    /// "Expected type" the current expression is being checked against.
    /// Used so `prop: Person = new { ... }` knows to resolve `name = ...`
    /// inside the body against `Person`. Pushed by [`walk_property`] and
    /// consulted by [`infer_expr`] when handling `Expr::New`.
    expected_context: Option<Ty>,
    /// The user-defined class whose body we're currently visiting, if any.
    /// Used to type `this` and `super` references inside method bodies.
    current_class: Option<SymbolId>,
    /// Active flow-sensitive narrowings: `if (x is String) ...` makes `x`
    /// look like `String` inside the then-branch. Stacked by `walk_if`
    /// and consulted when typing `Expr::Ident`.
    narrowings: HashMap<SymbolId, Ty>,
    /// File-scope inferred types for properties whose value the inferrer
    /// has walked (e.g. `xs = List(1,2,3)` infers `xs` as `List<Int>`).
    /// Consulted by `Expr::Ident` when the symbol's declared type is
    /// `Unknown`.
    inferred_property_types: HashMap<SymbolId, Ty>,
}

impl Inferrer<'_> {
    fn record(&mut self, span: Span, ty: Ty) -> Ty {
        self.expr_types.insert(span.start, ty.clone());
        self.expr_types_by_end.insert(span.end, ty.clone());
        ty
    }

    fn walk_module(&mut self, module: &Module) {
        for item in module.items() {
            self.walk_item(&item);
        }
    }

    fn walk_item(&mut self, item: &Item) {
        match item {
            Item::Class(c) => {
                self.walk_annotations_node(c.annotations());
                if let Some(ext) = c.extends() {
                    self.walk_type(&ext);
                }
                if let Some(body) = c.body() {
                    let class_name = c.name().map(|t| ident_text(&t)).unwrap_or_default();
                    let class_id = find_user_class(self.resolution, &class_name);
                    let prev_class = self.current_class.take();
                    self.current_class = class_id.or(prev_class);
                    for m in body.members() {
                        match m {
                            ClassMember::Property(p) => self.walk_class_property(&p),
                            ClassMember::Method(m) => self.walk_class_method(&m),
                        }
                    }
                    self.current_class = prev_class;
                }
            }
            Item::TypeAlias(t) => {
                self.walk_annotations_node(t.annotations());
                if let Some(aliased) = t.aliased_type() {
                    self.walk_type(&aliased);
                }
            }
            Item::Property(p) => self.walk_property(p),
            Item::Method(m) => self.walk_method(m),
            Item::Error(_) => {}
        }
    }

    fn walk_property(&mut self, p: &PropertyDecl) {
        self.walk_annotations_node(p.annotations());
        let expected = p.ty().as_ref().map(Ty::from_cst_type);
        if let Some(ty) = p.ty() {
            self.walk_type(&ty);
        }
        let prev = self.expected_context.take();
        self.expected_context = expected.clone();
        let name_span = name_span_or_empty(p.name());
        let name_text = name_token_text(p.name());
        match p.value() {
            Some(PropertyValue::Expr(e)) => {
                let actual = self.infer_expr(&e);
                if let Some(exp) = &expected {
                    self.check_assignable(exp, &actual, expr_span(&e), &name_text);
                }
                // Cache the inferred value type for unannotated properties
                // so later references can resolve into it.
                if expected.is_none() && !matches!(actual, Ty::Unknown) {
                    if let Some(sym_id) =
                        self.resolution.by_span_start.get(&name_span.start).copied()
                    {
                        self.inferred_property_types.insert(sym_id, actual);
                    }
                }
            }
            Some(PropertyValue::ObjectBody(body)) => {
                self.walk_object_body(&body, expected.as_ref())
            }
            None => {}
        }
        self.expected_context = prev;
    }

    fn walk_class_property(&mut self, p: &cst::ClassPropertyDecl) {
        self.walk_annotations_node(p.annotations());
        let expected = p.ty().as_ref().map(Ty::from_cst_type);
        if let Some(ty) = p.ty() {
            self.walk_type(&ty);
        }
        let prev = self.expected_context.take();
        self.expected_context = expected.clone();
        let name_span = name_span_or_empty(p.name());
        let name_text = name_token_text(p.name());
        match p.value() {
            Some(PropertyValue::Expr(e)) => {
                let actual = self.infer_expr(&e);
                if let Some(exp) = &expected {
                    self.check_assignable(exp, &actual, expr_span(&e), &name_text);
                }
                if expected.is_none() && !matches!(actual, Ty::Unknown) {
                    if let Some(sym_id) =
                        self.resolution.by_span_start.get(&name_span.start).copied()
                    {
                        self.inferred_property_types.insert(sym_id, actual);
                    }
                }
            }
            Some(PropertyValue::ObjectBody(body)) => {
                self.walk_object_body(&body, expected.as_ref())
            }
            None => {}
        }
        self.expected_context = prev;
    }

    fn walk_object_property(&mut self, p: &cst::ObjectProperty, expected: Option<&Ty>) {
        if let Some(expected_ty) = expected {
            self.record_object_member(p, expected_ty);
        }
        // Then recurse with the property's own declared type, if any.
        self.walk_annotations_node(p.annotations());
        let declared = p.ty().as_ref().map(Ty::from_cst_type);
        if let Some(ty) = p.ty() {
            self.walk_type(&ty);
        }
        let prev = self.expected_context.take();
        self.expected_context = declared.clone();
        let name_span = name_span_or_empty(p.name());
        let name_text = name_token_text(p.name());
        match p.value() {
            Some(PropertyValue::Expr(e)) => {
                let actual = self.infer_expr(&e);
                if let Some(exp) = &declared {
                    self.check_assignable(exp, &actual, expr_span(&e), &name_text);
                }
                if declared.is_none() && !matches!(actual, Ty::Unknown) {
                    if let Some(sym_id) =
                        self.resolution.by_span_start.get(&name_span.start).copied()
                    {
                        self.inferred_property_types.insert(sym_id, actual);
                    }
                }
            }
            Some(PropertyValue::ObjectBody(body)) => {
                self.walk_object_body(&body, declared.as_ref())
            }
            None => {}
        }
        self.expected_context = prev;
    }

    /// Emit a type-mismatch diagnostic when `actual` isn't a subtype of
    /// `expected`. Skipped silently when either side is `Unknown`.
    fn check_assignable(&mut self, expected: &Ty, actual: &Ty, span: Span, name: &str) {
        if matches!(expected, Ty::Unknown) || matches!(actual, Ty::Unknown) {
            return;
        }
        if is_subtype(actual, expected, self.resolution) {
            return;
        }
        self.diagnostics.push(SyntaxDiagnostic::error(
            span,
            format!(
                "type mismatch in property `{}`: expected `{}`, found `{}`",
                name, expected, actual
            ),
        ));
    }

    fn walk_method(&mut self, m: &MethodDecl) {
        self.walk_annotations_node(m.annotations());
        let params: Vec<Parameter> = m
            .parameters()
            .map(|pl| pl.parameters().collect())
            .unwrap_or_default();
        for p in &params {
            if let Some(ty) = p.ty() {
                self.walk_type(&ty);
            }
        }
        if let Some(ret) = m.return_type() {
            self.walk_type(&ret);
        }
        let expected = m.return_type().as_ref().map(Ty::from_cst_type);
        let prev = self.expected_context.take();
        self.expected_context = expected.clone();
        let name_text = name_token_text(m.name());
        if let Some(body) = m.body() {
            let actual = self.infer_expr(&body);
            if let Some(exp) = &expected {
                self.check_assignable(exp, &actual, expr_span(&body), &name_text);
            }
        }
        self.expected_context = prev;
    }

    fn walk_class_method(&mut self, m: &cst::ClassMethodDecl) {
        self.walk_annotations_node(m.annotations());
        let params: Vec<Parameter> = m
            .parameters()
            .map(|pl| pl.parameters().collect())
            .unwrap_or_default();
        for p in &params {
            if let Some(ty) = p.ty() {
                self.walk_type(&ty);
            }
        }
        if let Some(ret) = m.return_type() {
            self.walk_type(&ret);
        }
        let expected = m.return_type().as_ref().map(Ty::from_cst_type);
        let prev = self.expected_context.take();
        self.expected_context = expected.clone();
        let name_text = name_token_text(m.name());
        if let Some(body) = m.body() {
            let actual = self.infer_expr(&body);
            if let Some(exp) = &expected {
                self.check_assignable(exp, &actual, expr_span(&body), &name_text);
            }
        }
        self.expected_context = prev;
    }

    fn walk_object_method(&mut self, m: &cst::ObjectMethod) {
        self.walk_annotations_node(m.annotations());
        let params: Vec<Parameter> = m
            .parameters()
            .map(|pl| pl.parameters().collect())
            .unwrap_or_default();
        for p in &params {
            if let Some(ty) = p.ty() {
                self.walk_type(&ty);
            }
        }
        if let Some(ret) = m.return_type() {
            self.walk_type(&ret);
        }
        let expected = m.return_type().as_ref().map(Ty::from_cst_type);
        let prev = self.expected_context.take();
        self.expected_context = expected.clone();
        let name_text = name_token_text(m.name());
        if let Some(body) = m.body() {
            let actual = self.infer_expr(&body);
            if let Some(exp) = &expected {
                self.check_assignable(exp, &actual, expr_span(&body), &name_text);
            }
        }
        self.expected_context = prev;
    }

    /// Walk a type annotation, recording cross-file member refs for
    /// qualified names. `acme.Foo` becomes a member ref pinned to the
    /// `Foo` segment whose receiver is the import alias `acme`; the LSP
    /// layer routes that through the module graph for hover and goto.
    fn walk_type(&mut self, ty: &Type) {
        match ty {
            Type::Named(n) => {
                if let Some(name) = n.name() {
                    self.record_qualified_member_ref(&name);
                }
                if let Some(args) = n.type_arguments() {
                    for a in args.arguments() {
                        self.walk_type(&a);
                    }
                }
            }
            Type::Nullable(n) => {
                if let Some(inner) = n.inner() {
                    self.walk_type(&inner);
                }
            }
            Type::Parenthesized(p) => {
                if let Some(inner) = p.inner() {
                    self.walk_type(&inner);
                }
            }
            Type::Union(u) => {
                for m in u.members() {
                    self.walk_type(&m);
                }
            }
            Type::Function(f) => {
                for p in f.parameters() {
                    self.walk_type(&p);
                }
                if let Some(result) = f.result() {
                    self.walk_type(&result);
                }
            }
            _ => {}
        }
    }

    /// Walk each argument of a member call, pre-narrowing the parameters
    /// of untyped lambda arguments to the type the callee expects. This
    /// gives `xs.map((x) -> x.length)` an inferred type for `x` before
    /// we visit the lambda body.
    fn infer_args_with_narrowing(
        &mut self,
        receiver_ty: &Ty,
        method_name: Option<&SyntaxToken>,
        args: &[Expr],
    ) -> Vec<Ty> {
        // Look up the parsed signature once.
        let parsed = method_name.and_then(|n| {
            let n_text = ident_text(n);
            let receiver_unwrapped = receiver_ty.unwrap_nullable();
            let stdlib_type = receiver_unwrapped
                .stdlib_name()
                .and_then(pkl_stdlib::find_type)?;
            let (member, owner) = find_member(stdlib_type, &n_text)?;
            let parsed = parse_signature(member.signature);
            // Receiver-side substitution env: bind the owner's class
            // generics from the receiver's positional args.
            let mut env: HashMap<String, Ty> = HashMap::new();
            let recv_args = receiver_unwrapped.type_args();
            for (i, g) in owner?.generics.iter().enumerate() {
                if let Some(arg) = recv_args.get(i) {
                    env.insert((*g).to_string(), arg.clone());
                }
            }
            Some((parsed, env))
        });

        let mut out: Vec<Ty> = Vec::with_capacity(args.len());
        for (i, arg) in args.iter().enumerate() {
            // Apply lambda narrowing only when:
            //   - we know the signature,
            //   - the i-th declared param is a function type,
            //   - the actual arg is a lambda with at least one untyped param.
            let mut handled = false;
            if let Some((parsed, env)) = &parsed {
                if let (Some(declared), Expr::Lambda(lam)) = (parsed.param_types.get(i), arg) {
                    let declared = declared.substitute(env);
                    if let Ty::Function {
                        params: declared_params,
                        ..
                    } = declared
                    {
                        let parameters: Vec<Parameter> = lam
                            .parameters()
                            .map(|pl| pl.parameters().collect())
                            .unwrap_or_default();
                        let mut saves: Vec<(SymbolId, Option<Ty>)> = Vec::new();
                        for (lp, dp) in parameters.iter().zip(declared_params.iter()) {
                            if lp.ty().is_some() {
                                continue;
                            }
                            let lp_name_span = name_span_or_empty(lp.name());
                            let Some(sym_id) =
                                self.resolution.by_span_start.get(&lp_name_span.start)
                            else {
                                continue;
                            };
                            let prev = self.narrowings.insert(*sym_id, dp.clone());
                            saves.push((*sym_id, prev));
                        }
                        let ty = self.infer_expr(arg);
                        for (sym, prev) in saves {
                            match prev {
                                Some(p) => {
                                    self.narrowings.insert(sym, p);
                                }
                                None => {
                                    self.narrowings.remove(&sym);
                                }
                            }
                        }
                        out.push(ty);
                        handled = true;
                    }
                }
            }
            if !handled {
                out.push(self.infer_expr(arg));
            }
        }
        out
    }

    /// LUB-style join: combine two branch types into a single type the
    /// LSP can show in hover and use for further inference. Falls back to
    /// a `Ty::Union` only when the lattice can't decide.
    fn join_class_types(&self, a: Ty, b: Ty) -> Ty {
        if a == b {
            return a;
        }
        if matches!(a, Ty::Unknown) {
            return b;
        }
        if matches!(b, Ty::Unknown) {
            return a;
        }

        // Null/Nullable handling.
        match (&a, &b) {
            (Ty::Null, other) | (other, Ty::Null) => return Ty::Nullable(Box::new(other.clone())),
            (Ty::Nullable(inner_a), other) | (other, Ty::Nullable(inner_a)) => {
                let joined = self.join_class_types((**inner_a).clone(), other.clone());
                return Ty::Nullable(Box::new(joined));
            }
            _ => {}
        }

        // Primitive numeric promotion.
        if matches!((&a, &b), (Ty::Int, Ty::Float) | (Ty::Float, Ty::Int)) {
            return Ty::Number;
        }
        if matches!(
            (&a, &b),
            (Ty::Int, Ty::Number)
                | (Ty::Number, Ty::Int)
                | (Ty::Float, Ty::Number)
                | (Ty::Number, Ty::Float)
        ) {
            return Ty::Number;
        }

        // Class graph: find a common ancestor.
        if let (Some(name_a), Some(name_b)) = (a.stdlib_name(), b.stdlib_name()) {
            if let Some(common) = self.common_class_ancestor(name_a, name_b) {
                return Ty::Named {
                    name: common,
                    args: Vec::new(),
                };
            }
        }

        Ty::Union(vec![a, b])
    }

    /// Compute every class in the extends chain of `name`, in order
    /// (closest first), terminating at the root. Stdlib classes use the
    /// catalogue; user classes use `Symbol::parent_class`.
    fn class_chain(&self, name: &str) -> Vec<String> {
        let mut out = vec![name.to_string()];
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        seen.insert(name.to_string());
        let mut current = name.to_string();
        loop {
            let parent = self.parent_of(&current);
            let Some(parent) = parent else { break };
            if !seen.insert(parent.clone()) {
                break;
            }
            out.push(parent.clone());
            current = parent;
        }
        out
    }

    /// Find the parent of `name` — stdlib first, then a user class.
    fn parent_of(&self, name: &str) -> Option<String> {
        if let Some(t) = pkl_stdlib::find_type(name) {
            if let Some(extends) = t.extends {
                return Some(extends.split('<').next().unwrap_or(extends).to_string());
            }
        }
        let sym = self
            .resolution
            .symbols
            .iter()
            .find(|s| matches!(s.kind, crate::SymbolKind::Class) && s.name == name)?;
        sym.parent_class.clone()
    }

    fn common_class_ancestor(&self, a: &str, b: &str) -> Option<String> {
        let chain_b = self.class_chain(b);
        self.class_chain(a)
            .into_iter()
            .find(|ancestor| chain_b.iter().any(|c| c == ancestor))
    }

    /// Push a narrowing onto the active map, returning the previous value
    /// (if any) so callers can restore it via `pop_narrowing`.
    fn push_narrowing(
        &mut self,
        narrowing: Option<&(SymbolId, Ty)>,
    ) -> Option<(SymbolId, Option<Ty>)> {
        let (sym, ty) = narrowing?;
        let prev = self.narrowings.insert(*sym, ty.clone());
        Some((*sym, prev))
    }

    fn pop_narrowing(
        &mut self,
        narrowing: Option<&(SymbolId, Ty)>,
        prev: Option<(SymbolId, Option<Ty>)>,
    ) {
        let Some((sym, _)) = narrowing else {
            return;
        };
        match prev.and_then(|(_, p)| p) {
            Some(p) => {
                self.narrowings.insert(*sym, p);
            }
            None => {
                self.narrowings.remove(sym);
            }
        }
    }

    /// Narrow each `for (...)` binding from the iterable's type. Returns
    /// the SymbolIds we touched + their previous narrowings so the caller
    /// can restore them.
    fn push_for_bindings(
        &mut self,
        bindings: &[Parameter],
        iter_ty: &Ty,
    ) -> Vec<(SymbolId, Option<Ty>)> {
        let mut saves: Vec<(SymbolId, Option<Ty>)> = Vec::new();
        let iter_ty = iter_ty.unwrap_nullable();
        match (bindings.len(), iter_ty) {
            (1, Ty::List(t)) | (1, Ty::Set(t)) | (1, Ty::Listing(t)) => {
                let binding = &bindings[0];
                let binding_name_span = name_span_or_empty(binding.name());
                if let Some(sym_id) = self.resolution.by_span_start.get(&binding_name_span.start) {
                    if binding.ty().is_none() {
                        let prev = self.narrowings.insert(*sym_id, (**t).clone());
                        saves.push((*sym_id, prev));
                    }
                }
            }
            (1, Ty::Map(k, v)) | (1, Ty::Mapping(k, v)) => {
                // A single binding over a Map iterates over Pair<K, V>.
                let binding = &bindings[0];
                let binding_name_span = name_span_or_empty(binding.name());
                if let Some(sym_id) = self.resolution.by_span_start.get(&binding_name_span.start) {
                    if binding.ty().is_none() {
                        let pair = Ty::Pair(Box::new((**k).clone()), Box::new((**v).clone()));
                        let prev = self.narrowings.insert(*sym_id, pair);
                        saves.push((*sym_id, prev));
                    }
                }
            }
            (2, Ty::Map(k, v)) | (2, Ty::Mapping(k, v)) => {
                let pairs: [&Ty; 2] = [k, v];
                for (binding, ty) in bindings.iter().zip(pairs.iter().copied()) {
                    if binding.ty().is_some() {
                        continue;
                    }
                    let binding_name_span = name_span_or_empty(binding.name());
                    let Some(sym_id) = self.resolution.by_span_start.get(&binding_name_span.start)
                    else {
                        continue;
                    };
                    let prev = self.narrowings.insert(*sym_id, ty.clone());
                    saves.push((*sym_id, prev));
                }
            }
            _ => {}
        }
        saves
    }

    fn pop_for_bindings(&mut self, saves: Vec<(SymbolId, Option<Ty>)>) {
        for (sym, prev) in saves {
            match prev {
                Some(p) => {
                    self.narrowings.insert(sym, p);
                }
                None => {
                    self.narrowings.remove(&sym);
                }
            }
        }
    }

    /// Extract a flow narrowing from an `if` condition.
    /// `x is T` ⇒ narrow `x` to `T` inside the then-branch.
    fn extract_narrowing(&self, cond: &Expr) -> Option<(SymbolId, Ty)> {
        let Expr::TypeCheck(tc) = cond else {
            return None;
        };
        let operand = tc.operand()?;
        let Expr::Ident(id) = operand else {
            return None;
        };
        let tok = id.token()?;
        if id.special().is_some() {
            return None;
        }
        let id_span = token_span(&tok);
        let sym_id = *self.resolution.by_span_start.get(&id_span.start)?;
        let ty = tc.ty()?;
        Some((sym_id, Ty::from_cst_type(&ty)))
    }

    /// `Ty` of `this` inside the currently-walking class body, if any.
    fn current_class_ty(&self) -> Option<Ty> {
        let id = self.current_class?;
        let sym = self.resolution.symbol(id);
        Some(Ty::Named {
            name: sym.name.clone(),
            args: Vec::new(),
        })
    }

    /// `Ty` of `super` inside the currently-walking class body. Reads the
    /// parent class name off the symbol; returns `None` when there isn't
    /// one (no `extends` clause).
    fn super_class_ty(&self) -> Option<Ty> {
        let id = self.current_class?;
        let sym = self.resolution.symbol(id);
        let parent = sym.parent_class.as_ref()?;
        Some(Ty::Named {
            name: parent.clone(),
            args: Vec::new(),
        })
    }

    /// Register a `head.tail` qualified name as a cross-module member ref
    /// when the head resolves to an `Import` symbol in this file.
    fn record_qualified_member_ref(&mut self, name: &QualifiedName) {
        let segs: Vec<SyntaxToken> = name.segments().collect();
        if segs.len() < 2 {
            return;
        }
        let head = &segs[0];
        let tail = segs.last().unwrap();
        let head_span = token_span(head);
        let tail_span = token_span(tail);
        let Some(sym_id) = self.resolution.by_span_start.get(&head_span.start) else {
            return;
        };
        let sym = self.resolution.symbol(*sym_id);
        if !matches!(sym.kind, crate::SymbolKind::Import { .. }) {
            return;
        }
        self.member_refs.insert(
            tail_span.start,
            MemberRef {
                receiver_ty: Ty::Module,
                receiver_span: head_span,
                member_name: ident_text(tail),
                member_name_span: tail_span,
                stdlib_member: None,
                stdlib_type: None,
                user_class: None,
                user_member: None,
            },
        );
    }

    fn walk_annotations_node<I: Iterator<Item = cst::Annotation>>(&mut self, anns: I) {
        for a in anns {
            if let Some(name) = a.name() {
                self.record_qualified_member_ref(&name);
            }
            if let Some(body) = a.body() {
                self.walk_object_body(&body, None);
            }
        }
    }

    fn walk_object_body(&mut self, body: &ObjectBody, expected: Option<&Ty>) {
        for member in body.members() {
            match member {
                ObjectMember::Property(p) => self.walk_object_property(&p, expected),
                ObjectMember::Method(m) => self.walk_object_method(&m),
                ObjectMember::Element(e) => {
                    if let Some(expr) = e.expr() {
                        self.infer_expr(&expr);
                    }
                }
                ObjectMember::Entry(e) => {
                    if let Some(key) = e.key() {
                        self.infer_expr(&key);
                    }
                    match e.value() {
                        Some(PropertyValue::Expr(expr)) => {
                            self.infer_expr(&expr);
                        }
                        Some(PropertyValue::ObjectBody(body)) => self.walk_object_body(&body, None),
                        None => {}
                    }
                }
                ObjectMember::When(w) => {
                    if let Some(c) = w.condition() {
                        self.infer_expr(&c);
                        let narrowing = self.extract_narrowing(&c);
                        let prev = self.push_narrowing(narrowing.as_ref());
                        if let Some(then_body) = w.then_body() {
                            self.walk_object_body(&then_body, expected);
                        }
                        self.pop_narrowing(narrowing.as_ref(), prev);
                        if let Some(b) = w.else_body() {
                            self.walk_object_body(&b, expected);
                        }
                    } else if let Some(then_body) = w.then_body() {
                        self.walk_object_body(&then_body, expected);
                    }
                }
                ObjectMember::For(f) => {
                    let bindings: Vec<Parameter> = f.bindings().collect();
                    let iter_ty = if let Some(iter) = f.iterable() {
                        self.infer_expr(&iter)
                    } else {
                        Ty::Unknown
                    };
                    let pushed = self.push_for_bindings(&bindings, &iter_ty);
                    if let Some(body) = f.body() {
                        self.walk_object_body(&body, expected);
                    }
                    self.pop_for_bindings(pushed);
                }
                ObjectMember::Spread(s) => {
                    if let Some(expr) = s.expr() {
                        self.infer_expr(&expr);
                    }
                }
            }
        }
    }

    /// Visit a property declaration that appears inside an object body. If
    /// the surrounding object has a known type (from `new T { ... }` or a
    /// `prop: T = new { ... }` binding), record a [`MemberRef`] so the
    /// editor can hover / goto-def on `name = value` inside the body.
    fn record_object_member(&mut self, p: &cst::ObjectProperty, expected: &Ty) {
        let expected_unwrapped = expected.unwrap_nullable();
        let name_text = name_token_text(p.name());
        let name_span = name_span_or_empty(p.name());
        let stdlib_type = expected_unwrapped
            .stdlib_name()
            .and_then(pkl_stdlib::find_type);
        let (stdlib_member, owner) = stdlib_type.and_then(|t| find_member(t, &name_text)).unzip();
        let resolved_owner = owner.flatten();

        let mut user_class: Option<SymbolId> = None;
        let mut user_member: Option<SymbolId> = None;
        if stdlib_member.is_none() {
            if let Some(class_name) = expected_unwrapped.stdlib_name() {
                if let Some(class_id) = find_user_class(self.resolution, class_name) {
                    user_class = Some(class_id);
                    if let Some(m_id) = find_class_member(self.resolution, class_id, &name_text) {
                        user_member = Some(m_id);
                    }
                }
            }
        }

        if stdlib_member.is_none() && user_member.is_none() {
            // No anchor to resolve against; skip the record so we don't
            // spam editors with unresolved hovers for free-form objects.
            return;
        }

        self.member_refs.insert(
            name_span.start,
            MemberRef {
                receiver_ty: expected.clone(),
                receiver_span: name_span,
                member_name: name_text,
                member_name_span: name_span,
                stdlib_member,
                stdlib_type: resolved_owner,
                user_class,
                user_member,
            },
        );
    }

    // ------------------------------------------------------------------
    // Expressions

    fn infer_expr(&mut self, expr: &Expr) -> Ty {
        let span = expr_span(expr);
        let ty = match expr {
            Expr::Literal(lit) => self.infer_literal(lit),
            Expr::Ident(id) => {
                if let Some(kind) = id.special() {
                    match kind {
                        SpecialIdentKind::This => self.current_class_ty().unwrap_or(Ty::Unknown),
                        SpecialIdentKind::Super => self.super_class_ty().unwrap_or(Ty::Unknown),
                        SpecialIdentKind::Outer => Ty::Unknown,
                        SpecialIdentKind::Module => Ty::Module,
                    }
                } else if let Some(tok) = id.token() {
                    let id_span = token_span(&tok);
                    if let Some(sym) = self.resolution.by_span_start.get(&id_span.start) {
                        if let Some(narrowed) = self.narrowings.get(sym) {
                            return self.record(span, narrowed.clone());
                        }
                        let symbol = self.resolution.symbol(*sym);
                        if !matches!(symbol.declared_ty, Ty::Unknown) {
                            symbol.declared_ty.clone()
                        } else if let Some(inferred) = self.inferred_property_types.get(sym) {
                            inferred.clone()
                        } else {
                            Ty::Unknown
                        }
                    } else {
                        Ty::Unknown
                    }
                } else {
                    Ty::Unknown
                }
            }
            Expr::Paren(p) => {
                if let Some(inner) = p.inner() {
                    self.infer_expr(&inner)
                } else {
                    Ty::Unknown
                }
            }
            Expr::NonNull(n) => {
                let inner_ty = if let Some(operand) = n.operand() {
                    self.infer_expr(&operand)
                } else {
                    Ty::Unknown
                };
                // `expr!!` strips the nullable wrapper.
                match inner_ty {
                    Ty::Nullable(t) => *t,
                    other => other,
                }
            }
            Expr::Unary(u) => {
                let operand_ty = if let Some(operand) = u.operand() {
                    self.infer_expr(&operand)
                } else {
                    Ty::Unknown
                };
                match u.op() {
                    Some(UnaryOp::Not) => Ty::Boolean,
                    Some(UnaryOp::Neg) => match operand_ty {
                        Ty::Int | Ty::Float | Ty::Number => operand_ty,
                        _ => Ty::Unknown,
                    },
                    None => Ty::Unknown,
                }
            }
            Expr::Binary(b) => {
                let l = if let Some(lhs) = b.lhs() {
                    self.infer_expr(&lhs)
                } else {
                    Ty::Unknown
                };
                let r = if let Some(rhs) = b.rhs() {
                    self.infer_expr(&rhs)
                } else {
                    Ty::Unknown
                };
                match b.op() {
                    Some(op) => infer_binary(op, &l, &r),
                    None => Ty::Unknown,
                }
            }
            Expr::NullCoalesce(n) => {
                let l = if let Some(lhs) = n.lhs() {
                    self.infer_expr(&lhs)
                } else {
                    Ty::Unknown
                };
                let r = if let Some(rhs) = n.rhs() {
                    self.infer_expr(&rhs)
                } else {
                    Ty::Unknown
                };
                match l {
                    Ty::Nullable(inner) => join_types((*inner).clone(), r),
                    _ => join_types(l, r),
                }
            }
            Expr::TypeCheck(tc) => {
                if let Some(operand) = tc.operand() {
                    self.infer_expr(&operand);
                }
                if let Some(ty) = tc.ty() {
                    self.walk_type(&ty);
                }
                Ty::Boolean
            }
            Expr::TypeCast(tc) => {
                if let Some(operand) = tc.operand() {
                    self.infer_expr(&operand);
                }
                if let Some(ty) = tc.ty() {
                    self.walk_type(&ty);
                    Ty::from_cst_type(&ty)
                } else {
                    Ty::Unknown
                }
            }
            Expr::If(i) => {
                let cond = i.condition();
                if let Some(c) = &cond {
                    self.infer_expr(c);
                }
                let narrowing = cond.as_ref().and_then(|c| self.extract_narrowing(c));
                let prev = self.push_narrowing(narrowing.as_ref());
                let t = if let Some(then_branch) = i.then_branch() {
                    self.infer_expr(&then_branch)
                } else {
                    Ty::Unknown
                };
                self.pop_narrowing(narrowing.as_ref(), prev);
                let e = if let Some(else_branch) = i.else_branch() {
                    self.infer_expr(&else_branch)
                } else {
                    Ty::Unknown
                };
                self.join_class_types(t, e)
            }
            Expr::Let(l) => {
                if let Some(value) = l.value() {
                    self.infer_expr(&value);
                }
                if let Some(body) = l.body() {
                    self.infer_expr(&body)
                } else {
                    Ty::Unknown
                }
            }
            Expr::Lambda(lam) => {
                let ret_ty = if let Some(body) = lam.body() {
                    self.infer_expr(&body)
                } else {
                    Ty::Unknown
                };
                let parameters: Vec<Parameter> = lam
                    .parameters()
                    .map(|pl| pl.parameters().collect())
                    .unwrap_or_default();
                let params = parameters
                    .iter()
                    .map(|p| {
                        p.ty()
                            .as_ref()
                            .map(Ty::from_cst_type)
                            .unwrap_or(Ty::Unknown)
                    })
                    .collect();
                Ty::Function {
                    params,
                    ret: Box::new(ret_ty),
                }
            }
            Expr::Call(c) => {
                let callee = c.callee();
                let args: Vec<Expr> = c.args();
                // For member calls we want to pre-narrow untyped lambda
                // parameters from the member's signature *before* walking
                // each argument, so that `xs.map((x) -> x.length)` types
                // `x` as the element type.
                if let Some(Expr::Member(mem)) = &callee {
                    let receiver = mem.receiver();
                    let receiver_span = receiver.as_ref().map(expr_span).unwrap_or(Span::EMPTY);
                    let receiver_ty = receiver
                        .as_ref()
                        .map(|r| self.infer_expr(r))
                        .unwrap_or(Ty::Unknown);
                    let name = mem.name();
                    let arg_types =
                        self.infer_args_with_narrowing(&receiver_ty, name.as_ref(), &args);
                    let (member_ty, _is_member) = if let Some(name_tok) = name {
                        self.resolve_member_access(
                            &receiver_ty,
                            receiver_span,
                            &name_tok,
                            Some(&arg_types),
                        )
                    } else {
                        (Ty::Unknown, false)
                    };
                    if let Some(callee_expr) = &callee {
                        self.record(expr_span(callee_expr), member_ty.clone());
                    }
                    member_ty
                } else {
                    let arg_types: Vec<Ty> = args.iter().map(|a| self.infer_expr(a)).collect();
                    if let Some(callee_expr) = callee {
                        self.infer_top_level_call(&callee_expr, &arg_types)
                    } else {
                        Ty::Unknown
                    }
                }
            }
            Expr::Index(i) => {
                let recv_ty = if let Some(receiver) = i.receiver() {
                    self.infer_expr(&receiver)
                } else {
                    Ty::Unknown
                };
                if let Some(index) = i.index() {
                    self.infer_expr(&index);
                }
                index_result_type(&recv_ty)
            }
            Expr::Member(m) => {
                let receiver = m.receiver();
                let receiver_span = receiver.as_ref().map(expr_span).unwrap_or(Span::EMPTY);
                let receiver_ty = receiver
                    .as_ref()
                    .map(|r| self.infer_expr(r))
                    .unwrap_or(Ty::Unknown);
                let (member_ty, _) = if let Some(name_tok) = m.name() {
                    self.resolve_member_access(&receiver_ty, receiver_span, &name_tok, None)
                } else {
                    (Ty::Unknown, false)
                };
                member_ty
            }
            Expr::New(n) => {
                let ty = n.ty();
                if let Some(t) = &ty {
                    self.walk_type(t);
                }
                // Prefer the explicit `new Type { ... }` annotation, but
                // fall back to whatever the surrounding binding expected.
                let new_ty = ty
                    .as_ref()
                    .map(Ty::from_cst_type)
                    .filter(|t| !matches!(t, Ty::Unknown))
                    .or_else(|| self.expected_context.clone())
                    .unwrap_or(Ty::Unknown);
                if let Some(body) = n.body() {
                    self.walk_object_body(&body, Some(&new_ty));
                }
                new_ty
            }
            Expr::Amends(a) => {
                let base_ty = if let Some(base) = a.base() {
                    self.infer_expr(&base)
                } else {
                    Ty::Unknown
                };
                if let Some(body) = a.body() {
                    self.walk_object_body(&body, Some(&base_ty));
                }
                base_ty
            }
            Expr::Throw(t) => {
                if let Some(arg) = t.argument() {
                    self.infer_expr(&arg);
                }
                Ty::Nothing
            }
            Expr::Trace(t) => {
                if let Some(arg) = t.argument() {
                    self.infer_expr(&arg)
                } else {
                    Ty::Unknown
                }
            }
            Expr::Read(r) => {
                if let Some(arg) = r.argument() {
                    self.infer_expr(&arg);
                }
                match r.read_kind() {
                    Some(ReadKind::Read) => Ty::Resource,
                    Some(ReadKind::ReadOrNull) => Ty::Nullable(Box::new(Ty::Resource)),
                    Some(ReadKind::ReadGlob) => Ty::Map(Box::new(Ty::Str), Box::new(Ty::Resource)),
                    None => Ty::Unknown,
                }
            }
            Expr::Error(_) => Ty::Unknown,
        };
        self.record(span, ty.clone());
        ty
    }

    fn infer_literal(&self, lit: &LiteralExpr) -> Ty {
        match lit.kind() {
            Some(LiteralKind::Int) => Ty::Int,
            Some(LiteralKind::Float) => Ty::Float,
            Some(LiteralKind::String) | Some(LiteralKind::MultilineString) => Ty::Str,
            Some(LiteralKind::Bool) => Ty::Boolean,
            Some(LiteralKind::Null) => Ty::Null,
            None => Ty::Unknown,
        }
    }

    /// Infer the return type of a top-level call whose callee is *not* a
    /// member access. Uses the callee identifier's resolved symbol; if
    /// that symbol is a stdlib function, we re-parse its signature and
    /// substitute generics from the actual argument types.
    fn infer_top_level_call(&mut self, callee: &Expr, args: &[Ty]) -> Ty {
        // Try to find the stdlib function backing this callee.
        if let Expr::Ident(id) = callee {
            if let Some(tok) = id.token() {
                if id.special().is_none() {
                    let id_span = token_span(&tok);
                    if let Some(sym_id) = self.resolution.by_span_start.get(&id_span.start) {
                        let sym = self.resolution.symbol(*sym_id);
                        if sym.origin.is_stdlib() {
                            if let Some(f) = pkl_stdlib::find_function(&sym.name) {
                                let parsed = parse_signature(f.signature);
                                let mut env: HashMap<String, Ty> = HashMap::new();
                                let type_vars: HashSet<String> =
                                    parsed.type_params.iter().cloned().collect();
                                // For variadic stdlib constructors like
                                // `List(T...)` the parsed signature has a
                                // single param of type T; unify it against
                                // every actual argument.
                                for actual in args {
                                    for declared in &parsed.param_types {
                                        unify(declared, actual, &type_vars, &mut env);
                                    }
                                }
                                return parsed.return_ty.substitute(&env);
                            }
                        }
                    }
                }
            }
        }
        // Fallback: trust the callee's declared function type.
        let callee_ty = self.infer_expr(callee);
        match callee_ty {
            Ty::Function { ret, .. } => *ret,
            _ => Ty::Unknown,
        }
    }

    // ------------------------------------------------------------------
    // Member resolution

    /// Resolve `receiver.name` against the stdlib model and the user's
    /// own class declarations. Records a [`MemberRef`] for hover/goto
    /// and returns the member's value type.
    fn resolve_member_access(
        &mut self,
        receiver_ty: &Ty,
        receiver_span: Span,
        name: &SyntaxToken,
        call_args: Option<&[Ty]>,
    ) -> (Ty, bool) {
        let name_text = ident_text(name);
        let name_span = token_span(name);
        // Unwrap Nullable so `x?.length` still finds `length` on `String`.
        let receiver_unwrapped = receiver_ty.unwrap_nullable();
        let stdlib_type = receiver_unwrapped
            .stdlib_name()
            .and_then(pkl_stdlib::find_type);
        let (member, owner) = stdlib_type.and_then(|t| find_member(t, &name_text)).unzip();
        let resolved_owner = owner.flatten();

        // Build the substitution environment from the receiver's own generic
        // args, then unify the method's declared parameter types against the
        // caller's actual argument types.
        let mut member_ty = match member {
            Some(m) => {
                let parsed = parse_signature(m.signature);
                let env =
                    build_substitution_env(receiver_unwrapped, resolved_owner, &parsed, call_args);
                parsed.return_ty.substitute(&env)
            }
            None => Ty::Unknown,
        };

        // Fall back to a user-defined class if no stdlib member matched.
        // `instance.name` for `class Person { name: String }` resolves to the
        // class's property symbol.
        let mut user_class: Option<SymbolId> = None;
        let mut user_member: Option<SymbolId> = None;
        if member.is_none() {
            if let Some(class_name) = receiver_unwrapped.stdlib_name() {
                if let Some(class_id) = find_user_class(self.resolution, class_name) {
                    user_class = Some(class_id);
                    if let Some(m_id) = find_class_member(self.resolution, class_id, &name_text) {
                        user_member = Some(m_id);
                        member_ty = self.resolution.symbol(m_id).declared_ty.clone();
                    }
                }
            }
        }

        let is_resolved = member.is_some() || user_member.is_some();

        // Record the access — even if unresolved — so hover-on-member knows
        // a member is being accessed and can render the receiver type.
        self.member_refs.insert(
            name_span.start,
            MemberRef {
                receiver_ty: receiver_ty.clone(),
                receiver_span,
                member_name: name_text,
                member_name_span: name_span,
                stdlib_member: member,
                stdlib_type: resolved_owner,
                user_class,
                user_member,
            },
        );
        (member_ty, is_resolved)
    }
}

/// Find a user-defined class symbol by bare name. Returns `None` for
/// stdlib classes (they have synthetic spans) or unresolved names.
fn find_user_class(resolution: &Resolution, name: &str) -> Option<SymbolId> {
    resolution.symbols.iter().find_map(|s| {
        if !s.origin.is_stdlib() && matches!(s.kind, crate::SymbolKind::Class) && s.name == name {
            Some(s.id)
        } else {
            None
        }
    })
}

/// Find a property/method member declared inside the user-defined class
/// with id `class_id`, walking up the `extends` chain.
fn find_class_member(
    resolution: &Resolution,
    class_id: SymbolId,
    member_name: &str,
) -> Option<SymbolId> {
    let mut visited: std::collections::HashSet<SymbolId> = std::collections::HashSet::new();
    let mut current: Option<SymbolId> = Some(class_id);
    while let Some(cid) = current {
        if !visited.insert(cid) {
            break;
        }
        if let Some(direct) = resolution.symbols.iter().find_map(|s| {
            if s.container == Some(cid) && s.name == member_name {
                Some(s.id)
            } else {
                None
            }
        }) {
            return Some(direct);
        }
        let class_sym = resolution.symbol(cid);
        current = class_sym
            .parent_class
            .as_deref()
            .and_then(|p| find_user_class(resolution, p));
    }
    None
}

// ----------------------------------------------------------------------
// Public helpers for callers (LSP completion / references) that need to
// enumerate the surface of a type.

/// Every stdlib member visible on `ty`, walking up the `extends` chain
/// for class types. Returns `(member, declaring_type)` pairs.
pub fn stdlib_members_of(ty: &Ty) -> Vec<(&'static StdlibMember, &'static StdlibType)> {
    let mut out = Vec::new();
    let unwrapped = ty.unwrap_nullable();
    let Some(stdlib_name) = unwrapped.stdlib_name() else {
        return out;
    };
    let mut current = pkl_stdlib::find_type(stdlib_name);
    let mut seen_names = std::collections::HashSet::new();
    while let Some(t) = current {
        for m in t.members {
            if seen_names.insert(m.name) {
                out.push((m, t));
            }
        }
        current = t.extends.and_then(|parent| {
            let bare = parent.split('<').next().unwrap_or(parent);
            pkl_stdlib::find_type(bare)
        });
    }
    out
}

/// Every user-defined member visible on a type whose name matches a
/// class declared in `resolution`. Returns SymbolIds.
pub fn user_members_of(resolution: &Resolution, ty: &Ty) -> Vec<SymbolId> {
    let unwrapped = ty.unwrap_nullable();
    let Some(class_name) = unwrapped.stdlib_name() else {
        return Vec::new();
    };
    let Some(class_id) = find_user_class(resolution, class_name) else {
        return Vec::new();
    };
    resolution
        .symbols
        .iter()
        .filter(|s| s.container == Some(class_id))
        .map(|s| s.id)
        .collect()
}

/// Build a `TypeVar -> Ty` substitution map for a stdlib member access:
///
/// * Names listed in the receiver's stdlib `generics` are bound from the
///   positional args carried on the receiver's `Ty`.
/// * Names listed in the member's own `type_params` are bound by
///   unifying the declared parameter types against the call's actual
///   argument types (when known).
fn build_substitution_env(
    receiver_ty: &Ty,
    owner: Option<&'static StdlibType>,
    sig: &ParsedSignature,
    call_args: Option<&[Ty]>,
) -> HashMap<String, Ty> {
    let mut env: HashMap<String, Ty> = HashMap::new();

    // Receiver-side: zip generics with positional args.
    if let Some(t) = owner {
        let recv_args = receiver_ty.type_args();
        for (i, g) in t.generics.iter().enumerate() {
            if let Some(arg) = recv_args.get(i) {
                env.insert((*g).to_string(), arg.clone());
            }
        }
    }

    // Method-local type params: collect names, then unify declared params
    // against call args. We unify against the receiver-bound env too so
    // type vars introduced higher up don't reset.
    if !sig.type_params.is_empty() {
        let mut type_vars: HashSet<String> = sig.type_params.iter().cloned().collect();
        if let Some(t) = owner {
            for g in t.generics {
                type_vars.insert((*g).to_string());
            }
        }
        if let Some(args) = call_args {
            for (declared, actual) in sig.param_types.iter().zip(args.iter()) {
                unify(declared, actual, &type_vars, &mut env);
            }
        }
    }

    env
}

/// Look up `member_name` on `ty`, walking the `extends` chain.
///
/// At each class we try the hand-curated member list first; if nothing
/// matches we fall back to `pkl_stdlib::find_member`, which folds in
/// scraped entries from the vendored stdlib source. Returns the found
/// member alongside the type that actually declares it.
fn find_member(
    ty: &'static StdlibType,
    member_name: &str,
) -> Option<(&'static StdlibMember, Option<&'static StdlibType>)> {
    let mut current = Some(ty);
    while let Some(t) = current {
        if let Some(m) = t.members.iter().find(|m| m.name == member_name) {
            return Some((m, Some(t)));
        }
        if let Some(m) = pkl_stdlib::find_member(t.name, member_name) {
            return Some((m, Some(t)));
        }
        // Walk up by stripping any generic suffix from the `extends` string.
        current = t.extends.and_then(|parent| {
            let bare = parent.split('<').next().unwrap_or(parent);
            pkl_stdlib::find_type(bare)
        });
    }
    None
}

// ----------------------------------------------------------------------
// Helpers

fn infer_binary(op: BinaryOp, l: &Ty, r: &Ty) -> Ty {
    use BinaryOp::*;
    match op {
        Eq | NotEq | Lt | LtEq | Gt | GtEq | And | Or => Ty::Boolean,
        Add => {
            if matches!(l, Ty::Str) || matches!(r, Ty::Str) {
                Ty::Str
            } else {
                join_numeric(l, r)
            }
        }
        Sub | Mul | Rem | Pow | Div => join_numeric(l, r),
        Pipeline => r.clone(),
    }
}

fn join_numeric(l: &Ty, r: &Ty) -> Ty {
    match (l, r) {
        (Ty::Int, Ty::Int) => Ty::Int,
        (Ty::Float, _) | (_, Ty::Float) => Ty::Float,
        (Ty::Number, _) | (_, Ty::Number) => Ty::Number,
        (Ty::Int, _) | (_, Ty::Int) => Ty::Number,
        _ => Ty::Unknown,
    }
}

fn join_types(a: Ty, b: Ty) -> Ty {
    if a == b {
        return a;
    }
    if matches!(a, Ty::Unknown) {
        return b;
    }
    if matches!(b, Ty::Unknown) {
        return a;
    }
    Ty::Union(vec![a, b])
}

/// Type produced by `xs[k]` for various receiver shapes.
fn index_result_type(recv: &Ty) -> Ty {
    match recv {
        Ty::List(t) | Ty::Listing(t) | Ty::Set(t) => (**t).clone(),
        Ty::Map(_, v) | Ty::Mapping(_, v) => (**v).clone(),
        Ty::Str => Ty::Char,
        _ => Ty::Unknown,
    }
}

/// Significant span of an expression (excludes leading/trailing trivia).
/// Used as the universal key for `expr_types` lookups so the inferrer's
/// output matches the LSP's offset queries.
pub(crate) fn expr_span(e: &Expr) -> Span {
    significant_span(e.syntax())
}

fn name_token_text(t: Option<SyntaxToken>) -> String {
    t.map(|t| ident_text(&t)).unwrap_or_default()
}

fn name_span_or_empty(t: Option<SyntaxToken>) -> Span {
    t.map(|t| token_span(&t)).unwrap_or(Span::EMPTY)
}

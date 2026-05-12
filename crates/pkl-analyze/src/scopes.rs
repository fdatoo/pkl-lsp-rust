//! Lexical scope arena used by the resolver.
//!
//! Scopes form a tree: each non-root scope has a parent. Resolution walks
//! up the parent chain looking for the first scope that binds a name.
//!
//! We deliberately avoid `Rc`/`RefCell` here. The arena owns every scope by
//! index, and the resolver tracks its "current" scope as a single
//! [`ScopeId`] that it pushes/pops as it walks the AST.

use std::collections::HashMap;

use crate::symbols::SymbolId;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScopeId(pub(crate) u32);

impl ScopeId {
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Default)]
pub struct Scope {
    pub parent: Option<ScopeId>,
    pub bindings: HashMap<String, SymbolId>,
}

#[derive(Default)]
pub struct ScopeArena {
    scopes: Vec<Scope>,
}

impl ScopeArena {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc(&mut self, parent: Option<ScopeId>) -> ScopeId {
        let id = ScopeId(self.scopes.len() as u32);
        self.scopes.push(Scope {
            parent,
            bindings: HashMap::new(),
        });
        id
    }

    pub fn bind(&mut self, scope: ScopeId, name: impl Into<String>, symbol: SymbolId) {
        self.scopes[scope.index()]
            .bindings
            .insert(name.into(), symbol);
    }

    /// Resolve `name` starting from `scope` and walking up the parent chain.
    pub fn lookup(&self, scope: ScopeId, name: &str) -> Option<SymbolId> {
        let mut cur = Some(scope);
        while let Some(id) = cur {
            let s = &self.scopes[id.index()];
            if let Some(sym) = s.bindings.get(name) {
                return Some(*sym);
            }
            cur = s.parent;
        }
        None
    }

    pub fn parent_of(&self, scope: ScopeId) -> Option<ScopeId> {
        self.scopes[scope.index()].parent
    }
}

//! Cross-file module graph: parses + analyzes any Pkl module the LSP cares
//! about, and resolves `import "..."` statements to other entries.
//!
//! The graph is populated in two ways:
//!
//! 1. **Push** — the LSP calls [`ModuleGraph::upsert`] when a document is
//!    opened or edited. The graph reuses the supplied source verbatim and
//!    re-runs analysis.
//! 2. **Pull** — after analyzing a module, the graph asks the
//!    [`ModuleLoader`] to fetch every import target and recursively
//!    analyzes them as well. Cycles are guarded by tracking modules
//!    currently being loaded.
//!
//! The graph is intentionally minimal: it owns module state and surfaces
//! enough query methods for hover / goto-definition. Incremental
//! invalidation (re-analyzing dependents when a dependency changes) is
//! orchestrated by the LSP layer.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pkl_syntax::parse;

use crate::loader::{ModuleLoader, ModuleUri};
use crate::{analyze, Analysis};

/// One module known to the graph.
pub struct ModuleEntry {
    pub uri: ModuleUri,
    pub source: String,
    pub analysis: Analysis,
    /// Resolved targets for each `import "..." [as alias]` in this module,
    /// keyed by the import's local name (alias if present, else the path's
    /// stem). Missing entries mean the loader couldn't resolve the path.
    pub import_targets: HashMap<String, ModuleUri>,
    /// Errors encountered while resolving this module's imports. Surfaced
    /// to the editor as warnings.
    pub import_errors: Vec<ImportError>,
    /// `true` if the document was supplied by an LSP client; `false` if it
    /// was loaded transitively from disk via the loader.
    pub is_open: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportError {
    pub local_name: String,
    pub raw_path: String,
    pub message: String,
}

pub struct ModuleGraph {
    loader: Arc<dyn ModuleLoader>,
    modules: HashMap<ModuleUri, ModuleEntry>,
}

impl ModuleGraph {
    pub fn new(loader: Arc<dyn ModuleLoader>) -> Self {
        Self {
            loader,
            modules: HashMap::new(),
        }
    }

    /// Replace the active loader. Existing analyses are kept; subsequent
    /// import resolutions use the new loader.
    pub fn set_loader(&mut self, loader: Arc<dyn ModuleLoader>) {
        self.loader = loader;
    }

    pub fn get(&self, uri: &str) -> Option<&ModuleEntry> {
        self.modules.get(uri)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ModuleEntry> {
        self.modules.values()
    }

    /// Insert or replace the entry for `uri` with the given `source`. The
    /// module is re-analyzed and its imports are pulled in transitively.
    /// Returns the canonical URI (which equals `uri` for now — namespace
    /// canonicalisation happens inside the loader).
    pub fn upsert(&mut self, uri: ModuleUri, source: String, is_open: bool) -> ModuleUri {
        let mut visited: HashSet<ModuleUri> = HashSet::new();
        self.upsert_inner(uri.clone(), source, is_open, &mut visited);
        uri
    }

    /// Remove a module from the graph. Dependents continue to reference its
    /// URI; the next analysis pass will surface the missing target.
    pub fn remove(&mut self, uri: &str) {
        self.modules.remove(uri);
    }

    fn upsert_inner(
        &mut self,
        uri: ModuleUri,
        source: String,
        is_open: bool,
        visited: &mut HashSet<ModuleUri>,
    ) {
        if !visited.insert(uri.clone()) {
            return; // already loading on this stack
        }

        // Parse + analyse the supplied source.
        let parsed = parse(&source);
        let analysis = analyze(&parsed.syntax(), parsed.diagnostics.clone());

        // Resolve imports.
        let mut import_targets: HashMap<String, ModuleUri> = HashMap::new();
        let mut import_errors: Vec<ImportError> = Vec::new();
        // Collect first so we don't borrow self while iterating.
        let pending_imports: Vec<(String, String, bool)> = analysis
            .resolution
            .imports
            .values()
            .map(|info| (info.local_name.clone(), info.raw_path.clone(), info.is_glob))
            .collect();

        // Store the entry *before* recursing so cycles see a partial entry.
        self.modules.insert(
            uri.clone(),
            ModuleEntry {
                uri: uri.clone(),
                source,
                analysis,
                import_targets: HashMap::new(),
                import_errors: Vec::new(),
                is_open,
            },
        );

        for (local_name, raw_path, is_glob) in pending_imports {
            if is_glob {
                match self.loader.resolve_glob(&raw_path, Some(&uri)) {
                    Ok(matches) => {
                        if matches.is_empty() {
                            import_errors.push(ImportError {
                                local_name: local_name.clone(),
                                raw_path: raw_path.clone(),
                                message: format!("glob `{}` matched no files", raw_path),
                            });
                            continue;
                        }
                        // For now we track only the first match in
                        // `import_targets` — the LSP hover/goto path
                        // doesn't yet model the `Mapping<String, Module>`
                        // surface Pkl uses for globs. Every matched file
                        // still gets pulled into the graph so it can
                        // serve cross-file lookups.
                        let mut first: Option<ModuleUri> = None;
                        for (target_uri, target_source) in matches {
                            let already_loaded = self
                                .modules
                                .get(&target_uri)
                                .map(|e| e.is_open)
                                .unwrap_or(false);
                            if !already_loaded {
                                self.upsert_inner(
                                    target_uri.clone(),
                                    target_source,
                                    false,
                                    visited,
                                );
                            }
                            if first.is_none() {
                                first = Some(target_uri);
                            }
                        }
                        if let Some(target) = first {
                            import_targets.insert(local_name, target);
                        }
                    }
                    Err(err) => {
                        import_errors.push(ImportError {
                            local_name,
                            raw_path,
                            message: err.to_string(),
                        });
                    }
                }
                continue;
            }
            match self.loader.resolve(&raw_path, Some(&uri)) {
                Ok((target_uri, target_source)) => {
                    // Only load if we haven't already (open documents are
                    // authoritative — never overwrite an open module from
                    // the filesystem).
                    let already_loaded = self
                        .modules
                        .get(&target_uri)
                        .map(|e| e.is_open)
                        .unwrap_or(false);
                    if !already_loaded {
                        self.upsert_inner(target_uri.clone(), target_source, false, visited);
                    }
                    import_targets.insert(local_name, target_uri);
                }
                Err(err) => {
                    import_errors.push(ImportError {
                        local_name,
                        raw_path,
                        message: err.to_string(),
                    });
                }
            }
        }

        if let Some(entry) = self.modules.get_mut(&uri) {
            entry.import_targets = import_targets;
            entry.import_errors = import_errors;
        }
    }

    // ------------------------------------------------------------------
    // Cross-module queries

    /// Resolve `local_name` (an import alias inside `from`) to the imported
    /// module URI, if known.
    pub fn imported_module(&self, from: &str, local_name: &str) -> Option<&ModuleEntry> {
        let entry = self.modules.get(from)?;
        let target_uri = entry.import_targets.get(local_name)?;
        self.modules.get(target_uri)
    }

    /// Every module currently in the graph that imports `uri`. Used by the
    /// LSP layer to recompute cross-file features when a dependency
    /// changes — references, workspace symbols, completion ranking.
    pub fn dependents_of<'a>(&'a self, uri: &str) -> Vec<&'a ModuleEntry> {
        self.modules
            .values()
            .filter(|m| m.import_targets.values().any(|target| target == uri))
            .collect()
    }

    /// Re-load `uri` from disk via the active loader, replacing its
    /// graph entry. The open flag is preserved. Returns the new source
    /// length on success so callers can detect no-op refreshes.
    pub fn refresh_from_loader(&mut self, uri: &str) -> Option<usize> {
        let was_open = self.modules.get(uri).map(|e| e.is_open).unwrap_or(false);
        let (resolved_uri, source) = self.loader.resolve(uri, None).ok()?;
        let len = source.len();
        self.upsert(resolved_uri, source, was_open);
        Some(len)
    }

    /// Look up a top-level declaration named `member_name` in `module`.
    /// Returns the symbol so callers can fetch its hover info and span.
    pub fn lookup_top_level<'a>(
        &'a self,
        module: &'a ModuleEntry,
        member_name: &str,
    ) -> Option<&'a crate::Symbol> {
        module
            .analysis
            .resolution
            .symbols
            .iter()
            .find(|s| !s.origin.is_stdlib() && s.container.is_none() && s.name == member_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::{FsLoader, FsLoaderConfig};
    use std::collections::HashMap as Map;
    use std::path::PathBuf;

    use tempfile::tempdir;

    fn loader_with_namespaces(map: Map<&str, PathBuf>) -> Arc<dyn ModuleLoader> {
        let mut ns: Map<String, PathBuf> = Map::new();
        for (k, v) in map {
            ns.insert(k.to_string(), v);
        }
        FsLoader::new(FsLoaderConfig {
            namespaces: ns,
            ..FsLoaderConfig::default()
        })
        .into_arc()
    }

    #[test]
    fn loads_relative_import() {
        let dir = tempdir().unwrap();
        let util_path = dir.path().join("util.pkl");
        std::fs::write(&util_path, "answer: Int = 42\n").unwrap();

        let main_path = dir.path().join("main.pkl");
        std::fs::write(&main_path, "import \"./util.pkl\" as util\nx = util\n").unwrap();

        let mut graph = ModuleGraph::new(loader_with_namespaces(Map::new()));
        let main_uri = format!("file://{}", main_path.display());
        let source = std::fs::read_to_string(&main_path).unwrap();
        graph.upsert(main_uri.clone(), source, true);

        let entry = graph.get(&main_uri).unwrap();
        assert_eq!(entry.import_errors, vec![] as Vec<ImportError>);
        let target = entry.import_targets.get("util").expect("util resolved");
        let target_entry = graph.get(target).expect("util module loaded");
        let answer = graph.lookup_top_level(target_entry, "answer").unwrap();
        assert_eq!(answer.name, "answer");
    }

    #[test]
    fn loads_namespace_import() {
        let dir = tempdir().unwrap();
        let module_path = dir.path().join("config.pkl");
        std::fs::write(&module_path, "port: Int = 8080\n").unwrap();

        let mut ns = Map::new();
        ns.insert("switchyard", dir.path().to_path_buf());
        let loader = loader_with_namespaces(ns);

        let mut graph = ModuleGraph::new(loader);
        // The user-edited file lives outside the namespace.
        let main_dir = tempdir().unwrap();
        let main_path = main_dir.path().join("main.pkl");
        std::fs::write(
            &main_path,
            "import \"switchyard:config.pkl\" as cfg\nx = cfg\n",
        )
        .unwrap();
        let main_uri = format!("file://{}", main_path.display());
        let source = std::fs::read_to_string(&main_path).unwrap();
        graph.upsert(main_uri.clone(), source, true);

        let entry = graph.get(&main_uri).unwrap();
        assert!(
            entry.import_errors.is_empty(),
            "expected no errors, got {:?}",
            entry.import_errors
        );
        let target_uri = entry
            .import_targets
            .get("cfg")
            .expect("cfg import resolved");
        let target = graph.get(target_uri).expect("namespace module loaded");
        let port = graph.lookup_top_level(target, "port").unwrap();
        assert_eq!(port.name, "port");
    }

    #[test]
    fn missing_namespace_records_import_error() {
        let dir = tempdir().unwrap();
        let main_path = dir.path().join("main.pkl");
        std::fs::write(
            &main_path,
            "import \"acme:missing.pkl\" as acme\nx = acme\n",
        )
        .unwrap();

        let mut graph = ModuleGraph::new(loader_with_namespaces(Map::new()));
        let main_uri = format!("file://{}", main_path.display());
        let source = std::fs::read_to_string(&main_path).unwrap();
        graph.upsert(main_uri.clone(), source, true);

        let entry = graph.get(&main_uri).unwrap();
        assert_eq!(entry.import_errors.len(), 1);
        assert_eq!(entry.import_errors[0].local_name, "acme");
        assert!(entry.import_errors[0].message.contains("acme"));
    }

    #[test]
    fn cyclic_import_does_not_loop_forever() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.pkl");
        let b = dir.path().join("b.pkl");
        std::fs::write(&a, "import \"./b.pkl\" as b\nx: Int = 1\n").unwrap();
        std::fs::write(&b, "import \"./a.pkl\" as a\ny: Int = 2\n").unwrap();

        let mut graph = ModuleGraph::new(loader_with_namespaces(Map::new()));
        let a_uri = format!("file://{}", a.display());
        let src = std::fs::read_to_string(&a).unwrap();
        graph.upsert(a_uri.clone(), src, true);

        assert!(graph.get(&a_uri).is_some());
        let b_uri = format!("file://{}", b.display());
        assert!(graph.get(&b_uri).is_some());
    }

    #[test]
    fn glob_import_loads_every_match() {
        let dir = tempdir().unwrap();
        let parts = dir.path().join("parts");
        std::fs::create_dir(&parts).unwrap();
        std::fs::write(parts.join("one.pkl"), "a: Int = 1\n").unwrap();
        std::fs::write(parts.join("two.pkl"), "b: Int = 2\n").unwrap();

        let main = dir.path().join("main.pkl");
        std::fs::write(&main, "import* \"./parts/*.pkl\" as parts\n").unwrap();

        let mut graph = ModuleGraph::new(loader_with_namespaces(Map::new()));
        let main_uri = format!("file://{}", main.display());
        let src = std::fs::read_to_string(&main).unwrap();
        graph.upsert(main_uri.clone(), src, true);

        let one_uri = format!(
            "file://{}",
            parts.join("one.pkl").canonicalize().unwrap().display()
        );
        let two_uri = format!(
            "file://{}",
            parts.join("two.pkl").canonicalize().unwrap().display()
        );
        assert!(
            graph.get(&one_uri).is_some(),
            "expected one.pkl in graph, have {:?}",
            graph.iter().map(|m| m.uri.clone()).collect::<Vec<_>>()
        );
        assert!(graph.get(&two_uri).is_some());

        let entry = graph.get(&main_uri).unwrap();
        assert!(
            entry.import_errors.is_empty(),
            "import errors: {:?}",
            entry.import_errors
        );
    }
}

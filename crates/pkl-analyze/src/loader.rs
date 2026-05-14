//! Module-loading abstraction for cross-file resolution.
//!
//! Pkl's `import "..."` statement understands several path forms:
//!
//! * Relative paths (`"./foo.pkl"`, `"foo.pkl"`) resolved against the
//!   importing file's directory.
//! * Absolute filesystem paths (`"/etc/foo.pkl"`).
//! * `file://` URIs.
//!
//! On top of those we support **custom namespaces**: the user configures a
//! map from a prefix (e.g. `"switchyard"`) to a filesystem root, and any
//! import whose path starts with `"switchyard:"` is rewritten to live under
//! that root.
//!
//! The official Pkl schemes `pkl:`, `package:`, `https:`, `http:`, and
//! `modulepath:` are *recognised* (so we don't try to read them as files)
//! but otherwise unsupported in this initial implementation — the loader
//! reports them as [`LoadError::UnsupportedScheme`].

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Canonical, absolute, normalised string identifying a module.
///
/// Filesystem-backed modules use `file:///abs/path.pkl`. Future remote
/// schemes will use their natural URI form.
pub type ModuleUri = String;

/// Errors produced when resolving an `import "..."` target.
#[derive(Debug)]
pub enum LoadError {
    /// We resolved the path but the file isn't on disk.
    NotFound(String),
    /// The path used a scheme we recognise but don't implement.
    UnsupportedScheme(String),
    /// The path references a namespace we don't have a mapping for.
    UnknownNamespace { namespace: String, raw: String },
    /// Something else went wrong on the filesystem.
    Io(io::Error),
    /// The path was malformed (e.g. empty, traversal escaping namespace).
    Malformed(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::NotFound(p) => write!(f, "module not found: {}", p),
            LoadError::UnsupportedScheme(s) => write!(f, "unsupported import scheme: {}", s),
            LoadError::UnknownNamespace { namespace, raw } => {
                write!(f, "unknown namespace `{}` in import `{}`", namespace, raw)
            }
            LoadError::Io(e) => write!(f, "I/O error reading module: {}", e),
            LoadError::Malformed(p) => write!(f, "malformed import path: {}", p),
        }
    }
}

impl std::error::Error for LoadError {}

/// Resolves an import string into a canonical URI plus its source bytes.
pub trait ModuleLoader: Send + Sync {
    /// `raw` is the literal contents of an `import "..."` string (without
    /// the surrounding quotes). `from` is the URI of the importing module,
    /// used to resolve relative paths. `None` means the request is not
    /// associated with a specific importer (e.g. the LSP did-open path
    /// for a top-level file).
    fn resolve(&self, raw: &str, from: Option<&str>) -> Result<(ModuleUri, String), LoadError>;

    /// Resolve a `import* "..."` glob to every file it matches. The
    /// default falls back to `resolve` so loaders that don't implement
    /// globs still work for trivial single-file patterns.
    fn resolve_glob(
        &self,
        raw: &str,
        from: Option<&str>,
    ) -> Result<Vec<(ModuleUri, String)>, LoadError> {
        self.resolve(raw, from).map(|out| vec![out])
    }
}

/// A loader that tries each child in order, returning the first
/// successful resolution. Errors from earlier children that aren't a
/// schemata mismatch propagate so the user sees the most actionable one.
pub struct ChainedLoader {
    children: Vec<Arc<dyn ModuleLoader>>,
}

impl ChainedLoader {
    pub fn new(children: Vec<Arc<dyn ModuleLoader>>) -> Self {
        Self { children }
    }

    pub fn into_arc(self) -> Arc<dyn ModuleLoader> {
        Arc::new(self)
    }
}

impl ModuleLoader for ChainedLoader {
    fn resolve(&self, raw: &str, from: Option<&str>) -> Result<(ModuleUri, String), LoadError> {
        let mut last_err: Option<LoadError> = None;
        for child in &self.children {
            match child.resolve(raw, from) {
                Ok(out) => return Ok(out),
                // `UnsupportedScheme` from one child often means another
                // child does support it. Don't surface that to the user.
                Err(LoadError::UnsupportedScheme(_)) => continue,
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            LoadError::UnsupportedScheme(format!("no loader matched `{}`", raw))
        }))
    }

    fn resolve_glob(
        &self,
        raw: &str,
        from: Option<&str>,
    ) -> Result<Vec<(ModuleUri, String)>, LoadError> {
        let mut last_err: Option<LoadError> = None;
        for child in &self.children {
            match child.resolve_glob(raw, from) {
                Ok(out) => return Ok(out),
                Err(LoadError::UnsupportedScheme(_)) => continue,
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            LoadError::UnsupportedScheme(format!("no loader matched glob `{}`", raw))
        }))
    }
}

/// Loader that resolves `pkl:` imports to the embedded vendored
/// stdlib sources.
pub struct StdlibLoader;

impl StdlibLoader {
    pub fn new() -> Self {
        Self
    }

    pub fn into_arc(self) -> Arc<dyn ModuleLoader> {
        Arc::new(self)
    }
}

impl Default for StdlibLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleLoader for StdlibLoader {
    fn resolve(&self, raw: &str, _from: Option<&str>) -> Result<(ModuleUri, String), LoadError> {
        let stripped = raw.trim();
        let name = match stripped.strip_prefix("pkl:") {
            Some(n) => n,
            None => return Err(LoadError::UnsupportedScheme(stripped.to_string())),
        };
        match pkl_stdlib::vendored::find(name) {
            Some(m) => Ok((pkl_stdlib::vendored::module_uri(name), m.source.to_string())),
            None => Err(LoadError::NotFound(format!("pkl:{}", name))),
        }
    }
}

/// Optional loader for `https:` / `http:` imports.
///
/// Off by default; enable the `remote` Cargo feature to compile it. Each
/// resolution issues a single blocking GET via `ureq`. Network failures
/// surface as `LoadError::Io`.
#[cfg(feature = "remote")]
pub struct RemoteLoader;

#[cfg(feature = "remote")]
impl RemoteLoader {
    pub fn new() -> Self {
        Self
    }

    pub fn into_arc(self) -> Arc<dyn ModuleLoader> {
        Arc::new(self)
    }
}

#[cfg(feature = "remote")]
impl Default for RemoteLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "remote")]
impl ModuleLoader for RemoteLoader {
    fn resolve(&self, raw: &str, _from: Option<&str>) -> Result<(ModuleUri, String), LoadError> {
        let stripped = raw.trim();
        if !(stripped.starts_with("https:") || stripped.starts_with("http:")) {
            return Err(LoadError::UnsupportedScheme(stripped.to_string()));
        }
        let resp = ureq::get(stripped)
            .call()
            .map_err(|e| LoadError::Io(io::Error::new(io::ErrorKind::Other, e.to_string())))?;
        let body = resp.into_string().map_err(LoadError::Io)?;
        Ok((stripped.to_string(), body))
    }
}

/// Configuration for the bundled [`FsLoader`].
#[derive(Clone, Debug, Default)]
pub struct FsLoaderConfig {
    /// Map from a namespace prefix (no trailing `:`) to a filesystem root.
    /// Imports of the form `"name:rest"` are rewritten to `root/rest`.
    pub namespaces: HashMap<String, PathBuf>,
    /// Ordered list of search roots for `modulepath:` imports. The
    /// loader tries each root in order and returns the first match.
    pub module_paths: Vec<PathBuf>,
    /// On-disk cache root for `package:` imports. Resolution looks for
    /// `<cache>/<host>/<package@version>/<path>` (the canonical
    /// pickle-cache layout). If unset, `package:` imports are
    /// unsupported.
    pub package_cache: Option<PathBuf>,
}

impl FsLoaderConfig {
    /// Parse a `name=path[,name=path...]` formatted string, performing
    /// `$HOME` and `$VAR` expansion against the current process
    /// environment.
    pub fn parse_env(raw: &str) -> Self {
        let mut namespaces = HashMap::new();
        for entry in raw.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            if let Some((name, path)) = entry.split_once('=') {
                let name = name.trim();
                if name.is_empty() {
                    continue;
                }
                let expanded = expand_env(path.trim());
                namespaces.insert(name.to_string(), PathBuf::from(expanded));
            }
        }
        FsLoaderConfig {
            namespaces,
            module_paths: Vec::new(),
            package_cache: None,
        }
    }

    /// Parse a PATH-like list of `modulepath:` roots. The separator is
    /// `:` on Unix and `;` on Windows.
    pub fn parse_module_paths(raw: &str) -> Vec<PathBuf> {
        let separator = if cfg!(windows) { ';' } else { ':' };
        raw.split(separator)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| PathBuf::from(expand_env(s)))
            .collect()
    }
}

/// Default filesystem loader.
pub struct FsLoader {
    config: FsLoaderConfig,
}

impl FsLoader {
    pub fn new(config: FsLoaderConfig) -> Self {
        Self { config }
    }

    pub fn into_arc(self) -> Arc<dyn ModuleLoader> {
        Arc::new(self)
    }

    /// Convenience: build a [`ChainedLoader`] containing a [`StdlibLoader`]
    /// followed by an [`FsLoader`] with the supplied config. The result
    /// resolves `pkl:` imports against the vendored stdlib and everything
    /// else against the filesystem.
    pub fn with_stdlib(config: FsLoaderConfig) -> Arc<dyn ModuleLoader> {
        ChainedLoader::new(vec![
            StdlibLoader::new().into_arc(),
            Self::new(config).into_arc(),
        ])
        .into_arc()
    }

    /// Resolve a `package:` import against the configured cache root.
    /// Accepts either `name@version/path` or `host/name@version/path`.
    /// Reports `UnsupportedScheme` when no cache is configured.
    fn resolve_package_cache(&self, rest: &str) -> Result<PathBuf, LoadError> {
        let cache = self
            .config
            .package_cache
            .as_ref()
            .ok_or_else(|| LoadError::UnsupportedScheme(format!("package:{}", rest)))?;
        // Trim leading slashes / `//` separators so we always join cleanly.
        let trimmed = rest.trim_start_matches('/');
        Ok(cache.join(trimmed))
    }

    /// Resolve a `modulepath:` import against the configured search roots
    /// in order. Returns the first existing match; falls back to the
    /// first root joined with the path when nothing exists yet (so
    /// hover/goto still has a stable URI).
    fn resolve_module_path(&self, rest: &str) -> Result<PathBuf, LoadError> {
        if self.config.module_paths.is_empty() {
            return Err(LoadError::UnsupportedScheme(format!("modulepath:{}", rest)));
        }
        let trimmed = rest.trim_start_matches('/');
        for root in &self.config.module_paths {
            let candidate = root.join(trimmed);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        // No hit on disk — return the first-root path as a stable
        // placeholder.
        Ok(self.config.module_paths[0].join(trimmed))
    }

    fn resolve_namespace_path(&self, prefix: &str, rest: &str) -> Result<PathBuf, LoadError> {
        let root =
            self.config
                .namespaces
                .get(prefix)
                .ok_or_else(|| LoadError::UnknownNamespace {
                    namespace: prefix.to_string(),
                    raw: format!("{}:{}", prefix, rest),
                })?;
        let rest = rest.trim_start_matches('/');
        let mut candidates = Vec::with_capacity(4);
        push_module_candidate(&mut candidates, root.join(rest));
        if root.join("PklProject.pkl").exists() {
            push_module_candidate(&mut candidates, root.join(prefix).join(rest));
        }
        candidates
            .iter()
            .find(|path| path.exists())
            .cloned()
            .or_else(|| candidates.into_iter().next())
            .ok_or_else(|| LoadError::Malformed(format!("empty namespace import `{}:`", prefix)))
    }

    /// Resolve `raw` to an absolute path without reading it.
    pub fn resolve_path(&self, raw: &str, from: Option<&str>) -> Result<PathBuf, LoadError> {
        let raw_trimmed = raw.trim();
        if raw_trimmed.is_empty() {
            return Err(LoadError::Malformed("empty import path".into()));
        }

        // 1. `pkl:` is owned by `StdlibLoader`; remote schemes need the
        //    optional `RemoteLoader`.
        for unsupported in ["pkl:", "https:", "http:"] {
            if raw_trimmed.starts_with(unsupported) {
                return Err(LoadError::UnsupportedScheme(raw_trimmed.to_string()));
            }
        }

        // 2. `package:` cache resolution.
        if let Some(rest) = raw_trimmed.strip_prefix("package:") {
            return self.resolve_package_cache(rest);
        }

        // 3. `modulepath:` — search each configured root in order.
        if let Some(rest) = raw_trimmed.strip_prefix("modulepath:") {
            return self.resolve_module_path(rest);
        }

        // 4. `file://` URI form.
        if let Some(rest) = raw_trimmed.strip_prefix("file://") {
            return Ok(PathBuf::from(rest));
        }

        // 3. Custom namespace: `name:path/to/file.pkl`.
        if let Some(idx) = raw_trimmed.find(':') {
            // Watch out for Windows drive letters like `C:\foo`. A single
            // alphabetic char before `:` is a drive letter, not a
            // namespace.
            let prefix = &raw_trimmed[..idx];
            let looks_like_drive_letter =
                prefix.len() == 1 && prefix.chars().next().unwrap().is_ascii_alphabetic();
            if !looks_like_drive_letter && is_simple_ident(prefix) {
                return self.resolve_namespace_path(prefix, &raw_trimmed[idx + 1..]);
            }
        }

        // 4. Absolute filesystem path.
        let path = Path::new(raw_trimmed);
        if path.is_absolute() {
            return Ok(path.to_path_buf());
        }

        // 5. Relative path — resolve against the importer's directory.
        match from {
            Some(from_uri) => {
                let base = uri_to_path(from_uri)
                    .ok_or_else(|| LoadError::Malformed(format!("importer uri: {}", from_uri)))?;
                let base_dir = base.parent().unwrap_or(Path::new(""));
                Ok(base_dir.join(raw_trimmed))
            }
            None => Err(LoadError::Malformed(format!(
                "cannot resolve relative import `{}` without an importer URI",
                raw_trimmed
            ))),
        }
    }
}

impl ModuleLoader for FsLoader {
    fn resolve(&self, raw: &str, from: Option<&str>) -> Result<(ModuleUri, String), LoadError> {
        let path = self.resolve_path(raw, from)?;
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Surface a NotFound error with the lexically-normalised
                // path so the LSP can still address the file by a stable
                // URI before the user saves it.
                return Err(LoadError::NotFound(
                    normalize_path(&path).display().to_string(),
                ));
            }
            Err(e) => return Err(LoadError::Io(e)),
        };
        let source = fs::read_to_string(&canonical).map_err(LoadError::Io)?;
        Ok((path_to_uri(&canonical), source))
    }

    fn resolve_glob(
        &self,
        raw: &str,
        from: Option<&str>,
    ) -> Result<Vec<(ModuleUri, String)>, LoadError> {
        let resolved = self.resolve_path(raw, from)?;
        let pattern = resolved.to_string_lossy().to_string();
        let matched = match glob::glob(&pattern) {
            Ok(iter) => iter,
            Err(e) => return Err(LoadError::Malformed(e.to_string())),
        };
        let mut out = Vec::new();
        for entry in matched {
            let path = match entry {
                Ok(p) => p,
                Err(e) => return Err(LoadError::Io(e.into_error())),
            };
            // Skip directories — globs typically target leaf files.
            if path.is_dir() {
                continue;
            }
            let canonical = path.canonicalize().map_err(|e| match e.kind() {
                io::ErrorKind::NotFound => LoadError::NotFound(path.display().to_string()),
                _ => LoadError::Io(e),
            })?;
            let source = fs::read_to_string(&canonical).map_err(LoadError::Io)?;
            out.push((path_to_uri(&canonical), source));
        }
        Ok(out)
    }
}

// ----------------------------------------------------------------------
// Helpers

fn is_simple_ident(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    s.chars().next().unwrap().is_ascii_alphabetic()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn push_module_candidate(out: &mut Vec<PathBuf>, path: PathBuf) {
    let has_extension = path.extension().is_some();
    out.push(path.clone());
    if !has_extension {
        out.push(path.with_extension("pkl"));
    }
}

fn expand_env(input: &str) -> String {
    // Tiny ${VAR}/$VAR expander good enough for `$HOME`-style paths in
    // namespace configs. Unknown vars become empty.
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('{') => {
                chars.next();
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c == '}' {
                        chars.next();
                        break;
                    }
                    name.push(c);
                    chars.next();
                }
                if let Ok(v) = std::env::var(&name) {
                    out.push_str(&v);
                }
            }
            Some(&c) if c.is_ascii_alphabetic() || c == '_' => {
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if let Ok(v) = std::env::var(&name) {
                    out.push_str(&v);
                }
            }
            _ => out.push('$'),
        }
    }
    out
}

/// Convert a `file:///absolute/path` URI back into a [`PathBuf`].
///
/// Handles the two flavours we emit:
///   * `file:///abs/path`         (Unix; leading triple-slash)
///   * `file:///C:/abs/path`      (Windows; leading triple-slash + drive)
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let stripped = uri.strip_prefix("file://")?;
    if cfg!(windows) {
        // Windows file URIs prefix `file:///C:/foo` — the leading slash
        // is just URI syntax; drop it so we end up with `C:/foo`.
        let trimmed = stripped.strip_prefix('/').unwrap_or(stripped);
        Some(PathBuf::from(trimmed.replace('/', "\\")))
    } else {
        Some(PathBuf::from(stripped))
    }
}

/// Lexically normalise a path: collapse `.` and `..` segments without
/// touching the filesystem. Used as a fallback when
/// `Path::canonicalize` fails on a target that doesn't exist yet
/// (e.g. an import pointing at a file the user hasn't created).
pub fn normalize_path(input: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in input.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Convert an absolute filesystem path to a `file://` URI string.
pub fn path_to_uri(path: &Path) -> ModuleUri {
    debug_assert!(path.is_absolute(), "path_to_uri: expected absolute path");
    let s = path.to_string_lossy();
    if cfg!(windows) {
        // file:///C:/foo
        format!("file:///{}", s.replace('\\', "/"))
    } else {
        format!("file://{}", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_namespaces() {
        std::env::set_var("PKL_LSP_TEST_HOME", "/tmp/test-home");
        let cfg = FsLoaderConfig::parse_env(
            "switchyard=$PKL_LSP_TEST_HOME/switchyard-pkl,empty=,acme=/etc/acme",
        );
        assert_eq!(
            cfg.namespaces.get("switchyard"),
            Some(&PathBuf::from("/tmp/test-home/switchyard-pkl"))
        );
        assert_eq!(
            cfg.namespaces.get("acme"),
            Some(&PathBuf::from("/etc/acme"))
        );
        // empty path is still recorded; loader will error when used.
        assert!(cfg.namespaces.contains_key("empty"));
    }

    #[test]
    fn resolves_namespace_paths() {
        let mut ns = HashMap::new();
        ns.insert("switchyard".into(), PathBuf::from("/tmp/switchyard"));
        let loader = FsLoader::new(FsLoaderConfig {
            namespaces: ns,
            ..FsLoaderConfig::default()
        });
        let p = loader
            .resolve_path("switchyard:config/main.pkl", None)
            .unwrap();
        assert_eq!(p, PathBuf::from("/tmp/switchyard/config/main.pkl"));
    }

    #[test]
    fn resolves_extensionless_namespace_paths() {
        let dir = tempfile::tempdir().unwrap();
        let module_path = dir.path().join("automations.pkl");
        std::fs::write(&module_path, "").unwrap();

        let mut ns = HashMap::new();
        ns.insert("switchyard".into(), dir.path().to_path_buf());
        let loader = FsLoader::new(FsLoaderConfig {
            namespaces: ns,
            ..FsLoaderConfig::default()
        });

        let p = loader.resolve_path("switchyard:automations", None).unwrap();
        assert_eq!(p, module_path);
    }

    #[test]
    fn resolves_project_root_namespace_module_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("PklProject.pkl"), r#"amends "pkl:Project""#).unwrap();
        let module_dir = dir.path().join("switchyard");
        std::fs::create_dir(&module_dir).unwrap();
        let module_path = module_dir.join("automations.pkl");
        std::fs::write(&module_path, "").unwrap();

        let mut ns = HashMap::new();
        ns.insert("switchyard".into(), dir.path().to_path_buf());
        let loader = FsLoader::new(FsLoaderConfig {
            namespaces: ns,
            ..FsLoaderConfig::default()
        });

        let p = loader.resolve_path("switchyard:automations", None).unwrap();
        assert_eq!(p, module_path);
    }

    #[test]
    fn unknown_namespace_errors() {
        let loader = FsLoader::new(FsLoaderConfig::default());
        let err = loader.resolve_path("switchyard:foo.pkl", None).unwrap_err();
        match err {
            LoadError::UnknownNamespace { namespace, .. } => {
                assert_eq!(namespace, "switchyard");
            }
            other => panic!("expected UnknownNamespace, got {:?}", other),
        }
    }

    #[test]
    fn unsupported_schemes_are_flagged() {
        let loader = FsLoader::new(FsLoaderConfig::default());
        for raw in [
            "pkl:json",
            "package://example.com/foo@1.0.0",
            "https://example.com/foo.pkl",
        ] {
            let err = loader.resolve_path(raw, None).unwrap_err();
            assert!(matches!(err, LoadError::UnsupportedScheme(_)), "{}", raw);
        }
    }

    // Unix-only: hardcodes a POSIX-style importer URI without a drive letter,
    // which `uri_to_path` doesn't handle on Windows. Windows path resolution
    // is exercised through the higher-level module_graph tests.
    #[cfg(not(windows))]
    #[test]
    fn relative_path_resolved_against_importer() {
        let importer = "file:///tmp/project/main.pkl";
        let loader = FsLoader::new(FsLoaderConfig::default());
        let p = loader.resolve_path("./util.pkl", Some(importer)).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/project/./util.pkl"));
    }

    #[test]
    fn relative_path_without_importer_errors() {
        let loader = FsLoader::new(FsLoaderConfig::default());
        let err = loader.resolve_path("./util.pkl", None).unwrap_err();
        assert!(matches!(err, LoadError::Malformed(_)));
    }

    #[test]
    fn file_uri_form() {
        let loader = FsLoader::new(FsLoaderConfig::default());
        let p = loader.resolve_path("file:///etc/foo.pkl", None).unwrap();
        assert_eq!(p, PathBuf::from("/etc/foo.pkl"));
    }

    #[test]
    fn stdlib_loader_resolves_pkl_scheme() {
        let loader = StdlibLoader::new();
        let (uri, src) = loader.resolve("pkl:json", None).unwrap();
        assert_eq!(uri, "pkl:json");
        assert!(src.contains("module pkl.json"));
    }

    #[test]
    fn stdlib_loader_rejects_non_pkl_scheme() {
        let loader = StdlibLoader::new();
        let err = loader.resolve("file:///etc/foo.pkl", None).unwrap_err();
        assert!(matches!(err, LoadError::UnsupportedScheme(_)));
    }

    #[test]
    fn path_to_uri_round_trips() {
        let input = if cfg!(windows) {
            PathBuf::from(r"C:\code\foo.pkl")
        } else {
            PathBuf::from("/code/foo.pkl")
        };
        let uri = path_to_uri(&input);
        if cfg!(windows) {
            assert_eq!(uri, "file:///C:/code/foo.pkl");
        } else {
            assert_eq!(uri, "file:///code/foo.pkl");
        }
        let round_trip = uri_to_path(&uri).unwrap();
        assert_eq!(round_trip, input);
    }

    #[test]
    fn normalize_path_collapses_dot_segments() {
        assert_eq!(
            normalize_path(Path::new("/a/./b/../c/d.pkl")),
            PathBuf::from("/a/c/d.pkl")
        );
        assert_eq!(
            normalize_path(Path::new("../outside")),
            PathBuf::from("../outside")
        );
    }

    #[test]
    fn modulepath_finds_first_matching_root() {
        use tempfile::tempdir;
        let root_a = tempdir().unwrap();
        let root_b = tempdir().unwrap();
        std::fs::write(root_b.path().join("foo.pkl"), "x: Int = 1\n").unwrap();
        let cfg = FsLoaderConfig {
            module_paths: vec![root_a.path().to_path_buf(), root_b.path().to_path_buf()],
            ..FsLoaderConfig::default()
        };
        let loader = FsLoader::new(cfg);
        let path = loader.resolve_path("modulepath:foo.pkl", None).unwrap();
        assert_eq!(path, root_b.path().join("foo.pkl"));
    }

    #[test]
    fn modulepath_without_config_is_unsupported() {
        let loader = FsLoader::new(FsLoaderConfig::default());
        let err = loader.resolve_path("modulepath:foo.pkl", None).unwrap_err();
        assert!(matches!(err, LoadError::UnsupportedScheme(_)));
    }

    #[test]
    fn package_resolves_against_cache() {
        use tempfile::tempdir;
        let cache = tempdir().unwrap();
        let cfg = FsLoaderConfig {
            package_cache: Some(cache.path().to_path_buf()),
            ..FsLoaderConfig::default()
        };
        let loader = FsLoader::new(cfg);
        let path = loader
            .resolve_path("package:example.com/pkg@1.0.0/main.pkl", None)
            .unwrap();
        assert_eq!(path, cache.path().join("example.com/pkg@1.0.0/main.pkl"));
    }

    #[test]
    fn package_without_cache_is_unsupported() {
        let loader = FsLoader::new(FsLoaderConfig::default());
        let err = loader
            .resolve_path("package:example.com/pkg@1.0.0/main.pkl", None)
            .unwrap_err();
        assert!(matches!(err, LoadError::UnsupportedScheme(_)));
    }

    #[test]
    fn parse_module_paths_handles_separator() {
        let sep = if cfg!(windows) { ';' } else { ':' };
        let raw = format!("/a/path{}/b/path", sep);
        let paths = FsLoaderConfig::parse_module_paths(&raw);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from("/a/path"));
    }

    #[test]
    fn chained_loader_tries_stdlib_first_then_fs() {
        // `pkl:json` should resolve via the stdlib loader.
        let chained = FsLoader::with_stdlib(FsLoaderConfig::default());
        let (uri, _) = chained.resolve("pkl:json", None).unwrap();
        assert_eq!(uri, "pkl:json");
        // A relative import without an importer URI surfaces the
        // filesystem loader's `Malformed` error rather than `UnsupportedScheme`.
        let err = chained.resolve("./foo.pkl", None).unwrap_err();
        assert!(matches!(err, LoadError::Malformed(_)));
    }
}

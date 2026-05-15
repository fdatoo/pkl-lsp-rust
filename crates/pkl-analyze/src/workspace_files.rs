//! Workspace `.pkl` file index, used by the LSP's import-path completion.
//!
//! The LSP scans the configured workspace roots once on `initialize` and
//! keeps the resulting [`WorkspaceIndex`] in memory. Subsequent
//! `didChangeWatchedFiles` / `didOpen` notifications mutate the index
//! incrementally so completion stays cheap.
//!
//! Resolution of an `import "..."` cursor context is then a two-step
//! filter over [`WorkspaceIndex::files`]:
//!
//! 1. Normalise every candidate into an extensionless path *relative to
//!    the importer's directory* (`../sibling`, `subdir/foo`, …).
//! 2. Keep only the candidates whose relative form starts with the
//!    prefix the user has typed inside the quotes, then rank them by
//!    directory affinity to the importer.
//!
//! The walk is bounded (depth 16) and skips well-known artefact roots so
//! it stays sub-second on real projects.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// Maximum recursion depth for the workspace scan. Pkl projects are
/// typically shallow; this cap exists to make pathological symlink loops
/// terminate. The depth-0 layer is the root itself.
const MAX_SCAN_DEPTH: usize = 16;

/// Directory names skipped during the workspace scan. These are common
/// language / VCS artefact roots that never contain user-authored Pkl.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".pkl-cache",
    "node_modules",
    "target",
    "build",
    "dist",
    ".idea",
    ".vscode",
];

/// Kind of import completion. The LSP layer renders both as `File`
/// completions today but the distinction is useful for future tooling
/// (e.g. surfacing modulepath candidates with a different icon).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportCompletionKind {
    /// A workspace file resolved through a relative path.
    WorkspaceFile,
    /// Reserved for future namespace / modulepath-based completions.
    #[allow(dead_code)]
    ModulePath,
}

/// One candidate to render in the editor's import-path completion list.
#[derive(Debug, Clone)]
pub struct ImportCompletion {
    /// User-facing label (typically the file's basename).
    pub display: String,
    /// String to insert between the import quotes.
    pub insert: String,
    /// Disposition for icons / filtering.
    pub kind: ImportCompletionKind,
    /// Lower is better. The LSP layer turns this into `sort_text`.
    pub score: i32,
}

/// In-memory cache of every `.pkl` file under the workspace roots.
#[derive(Debug, Default)]
pub struct WorkspaceIndex {
    roots: Vec<PathBuf>,
    /// Deduplicated, sorted list of `.pkl` files. Sorted on insertion so
    /// completion output is stable when scores tie.
    files: Vec<PathBuf>,
}

impl WorkspaceIndex {
    /// Empty index. Equivalent to `Self::default()`; spelled out so the
    /// LSP layer can construct one before `initialize` lands.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Scan every root once, depth-limited, skipping the well-known
    /// ignore set. Returns a populated index.
    pub fn scan(roots: Vec<PathBuf>) -> Self {
        let mut index = WorkspaceIndex {
            roots: roots.clone(),
            files: Vec::new(),
        };
        let mut seen: HashSet<PathBuf> = HashSet::new();
        // Track visited directories (via canonical path when available)
        // so symlink cycles terminate.
        let mut visited_dirs: HashSet<PathBuf> = HashSet::new();
        for root in roots {
            walk(&root, 0, &mut index.files, &mut seen, &mut visited_dirs);
        }
        index.files.sort();
        index
    }

    /// Roots passed to [`Self::scan`].
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Every `.pkl` file currently in the index.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// Add a new `.pkl` file to the index. Idempotent — if `path` is
    /// already known nothing changes. Non-`.pkl` paths are ignored.
    pub fn add(&mut self, path: PathBuf) {
        if !is_pkl_path(&path) {
            return;
        }
        let normalised = normalize(&path);
        match self.files.binary_search(&normalised) {
            Ok(_) => {}
            Err(idx) => self.files.insert(idx, normalised),
        }
    }

    /// Remove a previously-indexed path. No-op if it wasn't there.
    pub fn remove(&mut self, path: &Path) {
        let normalised = normalize(path);
        if let Ok(idx) = self.files.binary_search(&normalised) {
            self.files.remove(idx);
        }
    }

    /// Rank workspace files as candidates for an `import "<prefix>"`
    /// completion at `current_file`. Returns at most `MAX_RESULTS`
    /// items, sorted by score ascending.
    ///
    /// `current_file` is the path of the file the cursor sits in.
    /// `prefix` is the text the user has typed *between the quotes* so
    /// far — this is what `filter_text` needs to match against.
    pub fn completions_for(&self, current_file: &Path, prefix: &str) -> Vec<ImportCompletion> {
        const MAX_RESULTS: usize = 200;
        let current_dir = current_file.parent().unwrap_or(Path::new(""));
        let current_norm = normalize(current_file);
        let mut out: Vec<ImportCompletion> = Vec::new();
        for candidate in &self.files {
            if *candidate == current_norm {
                continue;
            }
            let Some(rel) = relative_path(current_dir, candidate) else {
                continue;
            };
            if !rel.starts_with(prefix) {
                continue;
            }
            let insert = strip_pkl_extension(&rel);
            let display = candidate
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&rel)
                .strip_suffix(".pkl")
                .unwrap_or_else(|| {
                    candidate
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&rel)
                })
                .to_string();
            let score = score_relative(&rel);
            out.push(ImportCompletion {
                display,
                insert,
                kind: ImportCompletionKind::WorkspaceFile,
                score,
            });
        }
        out.sort_by(|a, b| a.score.cmp(&b.score).then_with(|| a.insert.cmp(&b.insert)));
        out.truncate(MAX_RESULTS);
        out
    }
}

// ---------------------------------------------------------------------
// Walk helpers
// ---------------------------------------------------------------------

fn walk(
    dir: &Path,
    depth: usize,
    out: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    visited_dirs: &mut HashSet<PathBuf>,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let canonical = dir.canonicalize().ok();
    let key = canonical.clone().unwrap_or_else(|| dir.to_path_buf());
    if !visited_dirs.insert(key) {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            // Skip the IGNORED_DIRS set.
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if IGNORED_DIRS.contains(&name) {
                    continue;
                }
                // Skip dotfile directories that aren't explicitly listed —
                // they're almost never user-authored Pkl. Allow `.pkl-cache`
                // through the IGNORED_DIRS skip above just to keep behavior
                // predictable in case it ever holds editable files.
                if name.starts_with('.') {
                    continue;
                }
            }
            walk(&path, depth + 1, out, seen, visited_dirs);
        } else if file_type.is_file() && is_pkl_path(&path) {
            let normalised = normalize(&path);
            if seen.insert(normalised.clone()) {
                out.push(normalised);
            }
        }
        // Symlinks: file_type reports the symlink target, but we still
        // record cycle keys via `visited_dirs` so loops terminate even
        // when the symlink points at an ancestor.
    }
}

fn is_pkl_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("pkl")
}

/// Lexically normalise a path: collapse `.` and `..` segments without
/// touching the filesystem. Mirrors `loader::normalize_path` so the two
/// modules agree on a canonical form for symbolic comparison.
fn normalize(input: &Path) -> PathBuf {
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

/// Build a relative path from `from_dir` to `target`. Returns `None`
/// when the two paths share no common root (e.g. different drives on
/// Windows). The output always uses forward slashes regardless of the
/// host OS because Pkl import strings are OS-agnostic.
fn relative_path(from_dir: &Path, target: &Path) -> Option<String> {
    let from = normalize(from_dir);
    let target = normalize(target);

    let from_components: Vec<_> = from.components().collect();
    let target_components: Vec<_> = target.components().collect();

    // Drop any leading `RootDir` so we can compare drives explicitly on
    // Windows; on Unix we just bail if one side is absolute and the
    // other isn't.
    let from_prefix = from_components.first().copied();
    let target_prefix = target_components.first().copied();
    match (from_prefix, target_prefix) {
        (Some(Component::Prefix(a)), Some(Component::Prefix(b)))
            if a.as_os_str() != b.as_os_str() =>
        {
            return None;
        }
        _ => {}
    }

    let mut shared = 0usize;
    let limit = from_components.len().min(target_components.len());
    while shared < limit && from_components[shared] == target_components[shared] {
        shared += 1;
    }

    // If the importer has zero components (relative `""` parent) the
    // join still works — we just emit the target path unchanged.
    let parents_needed = from_components.len().saturating_sub(shared);
    let tail = &target_components[shared..];
    if parents_needed == 0 && tail.is_empty() {
        return Some(String::new());
    }

    let mut out = String::new();
    for _ in 0..parents_needed {
        out.push_str("../");
    }
    let mut first = parents_needed == 0;
    for comp in tail {
        let seg = comp.as_os_str().to_string_lossy();
        if first {
            first = false;
        } else if !out.ends_with('/') {
            out.push('/');
        }
        out.push_str(&seg);
    }
    // Drop a trailing slash if we ended up with `../` and nothing else.
    if out.is_empty() {
        return None;
    }
    // Strip an inadvertent leading `/` that can sneak in if the target
    // shared no prefix with `from_dir`.
    let cleaned = if out.starts_with('/') && !out.starts_with("//") {
        out.trim_start_matches('/').to_string()
    } else {
        out
    };
    Some(cleaned)
}

/// Lower is better. A path that stays in the same directory beats one
/// that descends, which beats one that climbs.
fn score_relative(rel: &str) -> i32 {
    let parent_jumps = rel.matches("../").count() as i32;
    let depth = rel.matches('/').count() as i32 - parent_jumps;
    // Heavy weight on `../` jumps; modest weight on descent depth.
    parent_jumps * 100 + depth * 10
}

fn strip_pkl_extension(path: &str) -> String {
    path.strip_suffix(".pkl").unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn scan_finds_pkl_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(&root.join("a.pkl"), "");
        write(&root.join("nested/b.pkl"), "");
        write(&root.join("nested/deeper/c.pkl"), "");
        write(&root.join("not-pkl.txt"), "");

        let index = WorkspaceIndex::scan(vec![root.to_path_buf()]);
        let files: Vec<_> = index
            .files()
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(files.contains(&"a.pkl".to_string()));
        assert!(files
            .iter()
            .any(|p| p.ends_with("nested/b.pkl") || p.ends_with("nested\\b.pkl")));
        assert!(files
            .iter()
            .any(|p| p.ends_with("nested/deeper/c.pkl") || p.ends_with("nested\\deeper\\c.pkl")));
        assert_eq!(files.len(), 3);
    }

    #[test]
    fn scan_skips_ignored_directories() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(&root.join("keep.pkl"), "");
        write(&root.join(".git/objects/blob.pkl"), "");
        write(&root.join("node_modules/dep/index.pkl"), "");
        write(&root.join("target/build.pkl"), "");

        let index = WorkspaceIndex::scan(vec![root.to_path_buf()]);
        assert_eq!(index.files().len(), 1);
        assert!(index.files()[0].ends_with("keep.pkl"));
    }

    #[test]
    fn add_and_remove_are_idempotent() {
        let mut index = WorkspaceIndex::empty();
        index.add(PathBuf::from("/tmp/proj/main.pkl"));
        index.add(PathBuf::from("/tmp/proj/main.pkl"));
        index.add(PathBuf::from("/tmp/proj/notes.txt"));
        assert_eq!(index.files().len(), 1);

        index.remove(Path::new("/tmp/proj/main.pkl"));
        assert!(index.files().is_empty());

        // Removing something we never added is a no-op.
        index.remove(Path::new("/tmp/proj/main.pkl"));
        assert!(index.files().is_empty());
    }

    #[test]
    fn completions_filter_by_prefix() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(&root.join("main.pkl"), "");
        write(&root.join("sibling.pkl"), "");
        write(&root.join("subdir/leaf.pkl"), "");

        let index = WorkspaceIndex::scan(vec![root.to_path_buf()]);
        let main = root.join("main.pkl");

        // Empty prefix returns everything except the current file.
        let all = index.completions_for(&main, "");
        assert_eq!(all.len(), 2);

        // Prefix `sib` filters to the sibling file.
        let sib = index.completions_for(&main, "sib");
        assert_eq!(sib.len(), 1);
        assert_eq!(sib[0].insert, "sibling");

        // Prefix `sub` filters to the descent.
        let sub = index.completions_for(&main, "sub");
        assert_eq!(sub.len(), 1);
        assert!(sub[0].insert.contains("leaf"));
    }

    #[test]
    fn completions_rank_same_directory_above_descent() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(&root.join("main.pkl"), "");
        write(&root.join("alpha.pkl"), "");
        write(&root.join("nested/beta.pkl"), "");
        write(&root.join("up/parent.pkl"), "");

        let index = WorkspaceIndex::scan(vec![root.to_path_buf()]);
        let main = root.join("main.pkl");
        let ranked = index.completions_for(&main, "");
        let inserts: Vec<_> = ranked.iter().map(|c| c.insert.clone()).collect();
        let idx_alpha = inserts.iter().position(|s| s == "alpha").unwrap();
        let idx_beta = inserts.iter().position(|s| s == "nested/beta").unwrap();
        assert!(
            idx_alpha < idx_beta,
            "expected alpha (same dir) ahead of nested/beta, got {:?}",
            inserts
        );
    }

    #[test]
    fn completions_from_subdir_produce_parent_relative_paths() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(&root.join("top.pkl"), "");
        write(&root.join("sub/main.pkl"), "");
        write(&root.join("sub/peer.pkl"), "");

        let index = WorkspaceIndex::scan(vec![root.to_path_buf()]);
        let main = root.join("sub/main.pkl");

        let comps = index.completions_for(&main, "");
        let inserts: Vec<_> = comps.iter().map(|c| c.insert.clone()).collect();
        assert!(inserts.contains(&"peer".to_string()));
        assert!(inserts.contains(&"../top".to_string()));

        // Prefix matching against `../`.
        let parents = index.completions_for(&main, "../");
        assert_eq!(parents.len(), 1);
        assert_eq!(parents[0].insert, "../top");
    }

    #[test]
    fn completions_empty_workspace_returns_nothing() {
        let dir = tempdir().unwrap();
        let index = WorkspaceIndex::scan(vec![dir.path().to_path_buf()]);
        assert!(index
            .completions_for(&dir.path().join("main.pkl"), "")
            .is_empty());
    }

    #[test]
    fn completions_nonexistent_workspace_does_not_error() {
        let index = WorkspaceIndex::scan(vec![PathBuf::from("/nonexistent/path/abc")]);
        assert!(index.files().is_empty());
        // Querying with no files in the index is well-defined.
        assert!(index
            .completions_for(Path::new("/nonexistent/path/abc/file.pkl"), "")
            .is_empty());
    }

    #[test]
    fn add_post_scan_appears_in_completions() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(&root.join("main.pkl"), "");
        let mut index = WorkspaceIndex::scan(vec![root.to_path_buf()]);
        assert!(index.completions_for(&root.join("main.pkl"), "").is_empty());

        index.add(root.join("late.pkl"));
        let comps = index.completions_for(&root.join("main.pkl"), "");
        assert_eq!(comps.len(), 1);
        assert_eq!(comps[0].insert, "late");
    }

    #[test]
    fn relative_path_round_trip() {
        // Same-directory siblings.
        let rel = relative_path(Path::new("/a/b"), Path::new("/a/b/c.pkl")).unwrap();
        assert_eq!(rel, "c.pkl");
        // Parent jump.
        let rel = relative_path(Path::new("/a/b/c"), Path::new("/a/b/peer.pkl")).unwrap();
        assert_eq!(rel, "../peer.pkl");
        // Sibling subtree.
        let rel = relative_path(Path::new("/a/b/c"), Path::new("/a/b/d/leaf.pkl")).unwrap();
        assert_eq!(rel, "../d/leaf.pkl");
    }
}

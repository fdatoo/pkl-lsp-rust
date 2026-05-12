//! Conversion between LSP `Url`s and analyzer `ModuleUri` strings.
//!
//! The analyzer keeps URIs as plain `String`s so it stays independent of
//! tower-lsp. The two representations are interchangeable for `file://`
//! URIs — we just transport the string.

use std::path::PathBuf;

use pkl_analyze::ModuleUri;
use tower_lsp::lsp_types::Url;

/// Convert an LSP `Url` into the [`ModuleUri`] form the analyzer uses.
/// Canonicalises the underlying path when it exists so symlinks and
/// loader-resolved paths agree on a single key.
pub fn url_to_module_uri(url: &Url) -> ModuleUri {
    if url.scheme() != "file" {
        return url.to_string();
    }
    let raw_path = match url.to_file_path() {
        Ok(p) => p,
        Err(_) => return url.to_string(),
    };
    let canonical = raw_path.canonicalize().unwrap_or(raw_path);
    pkl_analyze::loader::path_to_uri(&canonical)
}

/// Convert a [`ModuleUri`] back into an LSP `Url`. Returns `None` if the
/// URI is malformed.
pub fn module_uri_to_url(uri: &ModuleUri) -> Option<Url> {
    if let Some(path) = pkl_analyze::loader::uri_to_path(uri) {
        let absolute: PathBuf = if path.is_absolute() {
            path
        } else {
            std::env::current_dir().ok()?.join(path)
        };
        Url::from_file_path(absolute).ok()
    } else {
        Url::parse(uri).ok()
    }
}

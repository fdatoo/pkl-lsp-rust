use pkl_syntax::cst::{ident_text, token_span};
use tower_lsp::lsp_types::Url;

use crate::document::Document;

pub fn import_target_at(
    doc: &Document,
    offset: u32,
    target_lookup: impl Fn(&str) -> Option<Url>,
) -> Option<Url> {
    for import in doc.module().imports() {
        let path = import.path()?;
        let path_span = token_span(&path);
        if !path_span.touches(offset) {
            continue;
        }
        let local_name = import_local_name(&import);
        if let Some(url) = target_lookup(&local_name) {
            return Some(url);
        }
    }
    None
}

pub fn document_import_links(
    doc: &Document,
    target_lookup: impl Fn(&str) -> Option<Url>,
) -> Vec<tower_lsp::lsp_types::DocumentLink> {
    let mut links = Vec::new();
    for import in doc.module().imports() {
        let Some(path) = import.path() else {
            continue;
        };
        let local_name = import_local_name(&import);
        let Some(target) = target_lookup(&local_name) else {
            continue;
        };
        links.push(tower_lsp::lsp_types::DocumentLink {
            range: doc.span_to_range(token_span(&path)),
            target: Some(target),
            tooltip: Some("Open imported module".to_string()),
            data: None,
        });
    }
    links
}

fn import_local_name(import: &pkl_syntax::cst::ImportClause) -> String {
    if let Some(alias) = import.alias() {
        return ident_text(&alias);
    }
    import
        .path()
        .map(|path| derive_import_name(path.text()))
        .unwrap_or_default()
}

fn derive_import_name(raw_path: &str) -> String {
    let trimmed = strip_string_quotes(raw_path);
    let last = trimmed.rsplit('/').next().unwrap_or(&trimmed);
    let stem = match last.find('.') {
        Some(idx) => &last[..idx],
        None => last,
    };
    stem.to_string()
}

fn strip_string_quotes(raw: &str) -> String {
    let mut s = raw;
    let lead_hashes = s.bytes().take_while(|&b| b == b'#').count();
    s = &s[lead_hashes..];
    if s.starts_with("\"\"\"") && s.ends_with("\"\"\"") && s.len() >= 6 {
        s = &s[3..s.len() - 3];
    } else if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s = &s[1..s.len() - 1];
    }
    let trim_trail = s
        .bytes()
        .rev()
        .take_while(|&b| b == b'#')
        .count()
        .min(lead_hashes);
    s[..s.len() - trim_trail].to_string()
}

//! Server configuration: namespace mappings and other knobs delivered via
//! `initializationOptions` or environment variables.

use std::collections::HashMap;
use std::path::PathBuf;

use pkl_analyze::FsLoaderConfig;
use serde::Deserialize;
use tower_lsp::lsp_types::{Documentation, MarkupContent, MarkupKind};

const NAMESPACES_ENV: &str = "PKL_LSP_NAMESPACES";
const MODULE_PATHS_ENV: &str = "PKL_LSP_MODULE_PATHS";
const PACKAGE_CACHE_ENV: &str = "PKL_LSP_PACKAGE_CACHE";

#[derive(Debug, Default, Deserialize)]
pub struct InitOptions {
    /// User-defined namespace prefixes that map to filesystem roots. A
    /// later `import "name:foo.pkl"` is rewritten to `root/foo.pkl`.
    #[serde(default)]
    pub namespaces: HashMap<String, String>,
    /// Ordered list of search roots for `modulepath:` imports.
    #[serde(default, rename = "modulePaths")]
    pub module_paths: Vec<String>,
    /// Filesystem cache root for `package:` imports.
    #[serde(default, rename = "packageCache")]
    pub package_cache: Option<String>,
    /// Optional evaluator command for on-save validation. Use `{file}` as
    /// the placeholder for the saved document path.
    #[serde(default, rename = "evalCommand")]
    pub eval_command: Vec<String>,
    /// Field-keyed literal value completions. For example, values under
    /// `entity` are offered inside `entity = "..."`.
    #[serde(default)]
    pub completion: CompletionConfig,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct CompletionConfig {
    #[serde(default)]
    pub values: HashMap<String, Vec<ValueCompletion>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ValueCompletion {
    Label(String),
    Item {
        label: String,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        documentation: Option<String>,
    },
}

impl ValueCompletion {
    pub fn label(&self) -> &str {
        match self {
            ValueCompletion::Label(label) => label,
            ValueCompletion::Item { label, .. } => label,
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            ValueCompletion::Label(_) => None,
            ValueCompletion::Item { detail, .. } => detail.as_deref(),
        }
    }

    pub fn documentation(&self) -> Option<Documentation> {
        match self {
            ValueCompletion::Label(_) => None,
            ValueCompletion::Item { documentation, .. } => documentation.as_ref().map(|value| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: value.clone(),
                })
            }),
        }
    }
}

impl InitOptions {
    pub fn parse(raw: Option<serde_json::Value>) -> Self {
        raw.and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    /// Merge in any config defined via environment variables.
    /// Init-options entries take precedence.
    pub fn into_loader_config(self) -> FsLoaderConfig {
        let mut cfg = FsLoaderConfig::default();
        if let Ok(raw) = std::env::var(NAMESPACES_ENV) {
            cfg = FsLoaderConfig::parse_env(&raw);
        }
        if let Ok(raw) = std::env::var(MODULE_PATHS_ENV) {
            cfg.module_paths = FsLoaderConfig::parse_module_paths(&raw);
        }
        if let Ok(raw) = std::env::var(PACKAGE_CACHE_ENV) {
            cfg.package_cache = Some(PathBuf::from(expand_env(raw.trim())));
        }
        for (name, path) in self.namespaces {
            cfg.namespaces
                .insert(name, PathBuf::from(expand_env(&path)));
        }
        if !self.module_paths.is_empty() {
            cfg.module_paths = self
                .module_paths
                .into_iter()
                .map(|p| PathBuf::from(expand_env(&p)))
                .collect();
        }
        if let Some(p) = self.package_cache {
            cfg.package_cache = Some(PathBuf::from(expand_env(&p)));
        }
        cfg
    }
}

fn expand_env(input: &str) -> String {
    // Mirror loader::expand_env without duplicating its body publicly. A
    // shellexpand-style crate would do the same but isn't worth a new
    // dependency for the small surface we need.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_namespaces_from_init_options() {
        let value = Some(json!({"namespaces": {"switchyard": "/srv/sy"}}));
        let opts = InitOptions::parse(value);
        assert_eq!(opts.namespaces["switchyard"], "/srv/sy");
    }

    #[test]
    fn parses_field_value_completions_from_init_options() {
        let value = Some(json!({
            "completion": {
                "values": {
                    "entity": [
                        "light.kitchen",
                        {
                            "label": "light.cabinets",
                            "detail": "Cabinets Left",
                            "documentation": "A configured Home Assistant entity."
                        }
                    ]
                }
            }
        }));
        let opts = InitOptions::parse(value);
        let values = opts.completion.values.get("entity").unwrap();
        assert_eq!(values[0].label(), "light.kitchen");
        assert_eq!(values[1].label(), "light.cabinets");
        assert_eq!(values[1].detail(), Some("Cabinets Left"));
    }

    #[test]
    fn missing_init_options_yields_empty() {
        let opts = InitOptions::parse(None);
        assert!(opts.namespaces.is_empty());
    }

    #[test]
    fn env_supplements_init_options() {
        std::env::set_var("PKL_LSP_NAMESPACES", "envns=/tmp/env");
        std::env::set_var("PKL_LSP_TEST_ROOT", "/srv/test-root");
        let opts = InitOptions {
            namespaces: [("user".to_string(), "$PKL_LSP_TEST_ROOT".to_string())]
                .into_iter()
                .collect(),
            module_paths: Vec::new(),
            package_cache: None,
            eval_command: Vec::new(),
            completion: CompletionConfig::default(),
        };
        let cfg = opts.into_loader_config();
        assert_eq!(cfg.namespaces["envns"], PathBuf::from("/tmp/env"));
        assert_eq!(cfg.namespaces["user"], PathBuf::from("/srv/test-root"));
        std::env::remove_var("PKL_LSP_NAMESPACES");
    }
}

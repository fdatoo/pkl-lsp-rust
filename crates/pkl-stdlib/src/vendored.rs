//! Embedded copies of Pkl's standard-library modules.
//!
//! The files in `vendor/` are bundled into the binary via `include_str!`,
//! so the language server can resolve `import "pkl:..."` references
//! without any network or filesystem dependency.
//!
//! See `vendor/UPSTREAM.md` for the snapshot commit.

/// One stdlib module. `name` is the `pkl:` short name (e.g. `"json"`),
/// `module` is the fully qualified Pkl module name (e.g. `"pkl.json"`),
/// `filename` is the bundled `.pkl` file, and `source` is its text.
pub struct VendoredModule {
    pub name: &'static str,
    pub module: &'static str,
    pub filename: &'static str,
    pub source: &'static str,
}

macro_rules! v {
    ($name:literal, $module:literal, $file:literal) => {
        VendoredModule {
            name: $name,
            module: $module,
            filename: $file,
            source: include_str!(concat!("../vendor/", $file)),
        }
    };
}

/// Every Pkl standard-library module we ship.
pub static MODULES: &[&VendoredModule] = &[
    &v!("base", "pkl.base", "base.pkl"),
    &v!("math", "pkl.math", "math.pkl"),
    &v!("json", "pkl.json", "json.pkl"),
    &v!("yaml", "pkl.yaml", "yaml.pkl"),
    &v!("xml", "pkl.xml", "xml.pkl"),
    &v!("protobuf", "pkl.protobuf", "protobuf.pkl"),
    &v!("jsonnet", "pkl.jsonnet", "jsonnet.pkl"),
    &v!("pklbinary", "pkl.pklbinary", "pklbinary.pkl"),
    &v!("reflect", "pkl.reflect", "reflect.pkl"),
    &v!("test", "pkl.test", "test.pkl"),
    &v!("semver", "pkl.semver", "semver.pkl"),
    &v!("platform", "pkl.platform", "platform.pkl"),
    &v!("shell", "pkl.shell", "shell.pkl"),
    &v!("analyze", "pkl.analyze", "analyze.pkl"),
    &v!("settings", "pkl.settings", "settings.pkl"),
    &v!("release", "pkl.release", "release.pkl"),
    &v!("DocPackageInfo", "pkl.DocPackageInfo", "DocPackageInfo.pkl"),
    &v!("DocsiteInfo", "pkl.DocsiteInfo", "DocsiteInfo.pkl"),
    &v!("Project", "pkl.Project", "Project.pkl"),
    &v!(
        "EvaluatorSettings",
        "pkl.EvaluatorSettings",
        "EvaluatorSettings.pkl"
    ),
    &v!("Benchmark", "pkl.Benchmark", "Benchmark.pkl"),
    &v!("Command", "pkl.Command", "Command.pkl"),
];

/// Look up a module by its `pkl:` short name (e.g. `"json"` for
/// `import "pkl:json"`).
pub fn find(short_name: &str) -> Option<&'static VendoredModule> {
    MODULES.iter().copied().find(|m| m.name == short_name)
}

/// Canonical URI used to identify a vendored module. The format is
/// `pkl:<name>` so it doubles as the module key in the analyzer graph.
pub fn module_uri(short_name: &str) -> String {
    format!("pkl:{}", short_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_modules_load() {
        for m in MODULES {
            assert!(!m.source.is_empty(), "{} is empty", m.filename);
            assert!(
                m.source.contains("module pkl."),
                "{} doesn't look like a Pkl module",
                m.filename
            );
        }
    }

    #[test]
    fn lookup_by_short_name() {
        assert!(find("json").is_some());
        assert!(find("yaml").is_some());
        assert!(find("nonexistent").is_none());
    }
}

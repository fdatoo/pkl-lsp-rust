//! A structured catalogue of Pkl's standard library, used by the analyzer
//! for name resolution and hover content.
//!
//! Members are intentionally hand-curated rather than auto-generated from
//! the upstream Pkl source. That keeps the runtime cost zero (everything is
//! `&'static`), gives us editorial control over the doc-comment phrasing, and
//! avoids a Pkl interpreter dependency. Coverage is the most-used surface of
//! `pkl.base`; types from other stdlib modules (`pkl.math`, `pkl.yaml`, etc.)
//! will be modeled as needed.

pub mod base;
pub mod scrape;
pub mod top_level;
pub mod vendored;

/// Kind of declaration the [`StdlibType`] represents.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StdlibKind {
    /// `class Foo`
    Class,
    /// `open class Foo`
    OpenClass,
    /// `abstract class Foo`
    AbstractClass,
    /// `typealias`
    TypeAlias,
}

/// A built-in type known to the analyzer.
#[derive(Debug)]
pub struct StdlibType {
    pub name: &'static str,
    /// Stdlib module that declares the type, e.g. `pkl.base`.
    pub module: &'static str,
    pub kind: StdlibKind,
    /// Type-parameter names, e.g. `["T"]` for `List<T>`.
    pub generics: &'static [&'static str],
    /// Parent type name (for `class X extends Y`). Cross-module refs are
    /// allowed; the analyzer follows the link when needed.
    pub extends: Option<&'static str>,
    pub doc: &'static str,
    pub members: &'static [StdlibMember],
}

#[derive(Debug)]
pub struct StdlibMember {
    pub name: &'static str,
    pub kind: MemberKind,
    /// Full signature as it would appear in a hover card, e.g.
    /// `length: Int` for a property or
    /// `contains(other: String): Boolean` for a method.
    pub signature: &'static str,
    pub doc: &'static str,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Property,
    Method,
}

/// A top-level function visible in every Pkl module (e.g. `List`, `Set`,
/// `Map`, `read`). Distinct from a type member because it has no receiver.
#[derive(Debug)]
pub struct StdlibFunction {
    pub name: &'static str,
    pub module: &'static str,
    pub signature: &'static str,
    pub doc: &'static str,
}

/// Catalogue of every stdlib type the analyzer knows about. The
/// vendored `pkl.base` source is authoritative; hand-curated entries
/// from [`base::ALL`] supply only the types the upstream source doesn't
/// declare (e.g. compatibility shims, or test helpers).
pub fn types() -> Vec<&'static StdlibType> {
    let mut out: Vec<&'static StdlibType> = scrape::parsed_base().to_vec();
    let seen: std::collections::HashSet<&'static str> = out.iter().map(|t| t.name).collect();
    for t in base::ALL {
        if !seen.contains(t.name) {
            out.push(*t);
        }
    }
    out
}

/// Catalogue of every top-level function the analyzer knows about.
pub fn functions() -> &'static [&'static StdlibFunction] {
    top_level::ALL
}

/// Look up a stdlib type by its bare name (e.g. `"String"`). Scraped
/// (canonical) entries win; the hand-curated catalogue is a fallback.
pub fn find_type(name: &str) -> Option<&'static StdlibType> {
    scrape::parsed_base()
        .iter()
        .copied()
        .find(|t| t.name == name)
        .or_else(|| base::ALL.iter().copied().find(|t| t.name == name))
}

/// Find a member by name on the named class. Consults the canonical
/// scraped catalogue first, then the hand-curated fallback.
pub fn find_member(class_name: &str, member_name: &str) -> Option<&'static StdlibMember> {
    if let Some(t) = scrape::parsed_base()
        .iter()
        .copied()
        .find(|t| t.name == class_name)
    {
        if let Some(m) = t.members.iter().find(|m| m.name == member_name) {
            return Some(m);
        }
    }
    base::ALL
        .iter()
        .copied()
        .find(|t| t.name == class_name)
        .and_then(|t| t.members.iter().find(|m| m.name == member_name))
}

/// Look up a top-level function by name (e.g. `"List"`).
pub fn find_function(name: &str) -> Option<&'static StdlibFunction> {
    top_level::ALL.iter().copied().find(|f| f.name == name)
}

/// Format a type's display name including its generics (e.g. `List<T>`).
pub fn render_type_name(t: &StdlibType) -> String {
    if t.generics.is_empty() {
        t.name.to_string()
    } else {
        format!("{}<{}>", t.name, t.generics.join(", "))
    }
}

/// Render a Pkl-style signature for the type itself, used as the hover
/// signature when the user hovers a bare type reference.
pub fn render_type_signature(t: &StdlibType) -> String {
    let prefix = match t.kind {
        StdlibKind::Class => "class",
        StdlibKind::OpenClass => "open class",
        StdlibKind::AbstractClass => "abstract class",
        StdlibKind::TypeAlias => "typealias",
    };
    let head = format!("{} {}", prefix, render_type_name(t));
    match (t.kind, t.extends) {
        (StdlibKind::TypeAlias, Some(target)) => format!("{} = {}", head, target),
        (_, Some(target)) => format!("{} extends {}", head, target),
        _ => head,
    }
}

/// Names of standard-library modules shipped with Pkl. Used by completion
/// of import paths once that lands.
pub const STDLIB_MODULES: &[&str] = &[
    "pkl.base",
    "pkl.math",
    "pkl.platform",
    "pkl.protobuf",
    "pkl.reflect",
    "pkl.semver",
    "pkl.shell",
    "pkl.test",
    "pkl.xml",
    "pkl.yaml",
    "pkl.json",
];

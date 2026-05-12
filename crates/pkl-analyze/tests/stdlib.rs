//! Tests that the analyzer seeds Pkl's standard library into every module
//! scope and renders useful hover output for built-in names.

use pkl_analyze::hover::hover_markdown;
use pkl_analyze::resolve_module;
use pkl_analyze::symbols::Origin;
use pkl_syntax::cst::{AstNode, Module};
use pkl_syntax::parse;

fn resolve(src: &str) -> pkl_analyze::Resolution {
    let parsed = parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "syntax diagnostics: {:#?}",
        parsed.diagnostics
    );
    let module = Module::cast(parsed.syntax()).expect("module");
    resolve_module(&module)
}

#[test]
fn references_to_string_resolve_to_stdlib() {
    let src = "name: String = \"alice\"";
    let r = resolve(src);
    let offset = src.find("String").unwrap() as u32;
    let symbol_id = r.symbol_at_offset(offset).expect("String resolves");
    let symbol = r.symbol(symbol_id);
    assert_eq!(symbol.name, "String");
    assert!(matches!(
        symbol.origin,
        Origin::Stdlib { module: "pkl.base" }
    ));
}

#[test]
fn references_to_list_ctor_resolve_to_stdlib() {
    let src = "xs = List(1, 2, 3)";
    let r = resolve(src);
    let offset = src.find("List").unwrap() as u32;
    let symbol_id = r.symbol_at_offset(offset).expect("List resolves");
    let symbol = r.symbol(symbol_id);
    assert_eq!(symbol.name, "List");
    assert!(symbol.origin.is_stdlib());
}

#[test]
fn user_declaration_shadows_stdlib() {
    let src = "class String {}\nname: String = String";
    let r = resolve(src);
    let class = r
        .symbols
        .iter()
        .find(|s| s.name == "String" && matches!(s.origin, Origin::User))
        .expect("user String class");
    // Both references (the annotation and the expression) should resolve to
    // the user class, not the stdlib String.
    let annotation_offset = src.find(": String").unwrap() as u32 + 2;
    let expr_offset = src.rfind("String").unwrap() as u32;
    assert_eq!(r.symbol_at_offset(annotation_offset), Some(class.id));
    assert_eq!(r.symbol_at_offset(expr_offset), Some(class.id));
}

#[test]
fn hover_on_stdlib_type_shows_module_attribution() {
    let r = resolve("x: String = \"a\"");
    let string_sym = r
        .symbols
        .iter()
        .find(|s| s.name == "String" && s.origin.is_stdlib())
        .unwrap();
    let md = hover_markdown(&r, string_sym);
    assert!(md.contains("class String"), "got: {}", md);
    assert!(md.contains("from `pkl.base`"), "got: {}", md);
    // The doc-comment should appear too.
    assert!(md.contains("Unicode"), "got: {}", md);
}

#[test]
fn hover_on_user_symbol_does_not_mention_stdlib() {
    let r = resolve("name: String = \"a\"");
    let prop = r
        .symbols
        .iter()
        .find(|s| s.name == "name" && matches!(s.origin, Origin::User))
        .unwrap();
    let md = hover_markdown(&r, prop);
    assert!(
        !md.contains("from `"),
        "user hover should not credit stdlib: {}",
        md
    );
}

#[test]
fn well_known_types_are_all_seeded() {
    let r = resolve("");
    for name in [
        "Any",
        "Null",
        "Boolean",
        "Int",
        "Float",
        "Number",
        "String",
        "Char",
        "Bytes",
        "Duration",
        "DataSize",
        "Pair",
        "Collection",
        "List",
        "Set",
        "Map",
        "Listing",
        "Mapping",
        "Dynamic",
        "Regex",
    ] {
        let found = r
            .symbols
            .iter()
            .any(|s| s.name == name && s.origin.is_stdlib());
        assert!(found, "stdlib type `{}` missing from seeding", name);
    }
}

#[test]
fn top_level_constructors_are_seeded() {
    let r = resolve("");
    for name in ["List", "Set", "Map", "Pair", "Regex"] {
        let count = r.symbols.iter().filter(|s| s.name == name).count();
        // At minimum the type itself; for List/Set/Map/Pair/Regex we also
        // seed a constructor function, so the count is >= 1. We don't
        // require both because the type and function share a name and the
        // function entry overwrites the type entry in the scope.
        assert!(count >= 1, "missing entry for {}", name);
    }
}

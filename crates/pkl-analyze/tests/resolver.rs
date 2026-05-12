//! Integration tests for the Pkl resolver.

use pkl_analyze::hover::hover_markdown;
use pkl_analyze::resolve_module;
use pkl_analyze::SymbolKind;
use pkl_syntax::cst::{AstNode, Module};
use pkl_syntax::parse;

/// Helper that runs the parser + resolver and asserts there were no syntax
/// diagnostics, then returns the resolution.
fn resolve(src: &str) -> pkl_analyze::Resolution {
    let parsed = parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "syntax diagnostics: {:#?}\nsource:\n{}",
        parsed.diagnostics,
        src
    );
    let module = Module::cast(parsed.syntax()).expect("module root");
    resolve_module(&module)
}

/// Returns the byte offset of the first occurrence of `needle` in `src`.
fn offset_of(src: &str, needle: &str) -> u32 {
    src.find(needle).expect("needle not found") as u32
}

#[test]
fn registers_module_level_class_and_property() {
    let src = "class Foo {}\nname: String = \"alice\"";
    let r = resolve(src);
    let names: Vec<_> = r.symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"Foo"));
    assert!(names.contains(&"name"));
}

#[test]
fn class_members_are_registered_with_container() {
    let src = "class Person { name: String\n  function greet(): String = \"hi\" }";
    let r = resolve(src);
    let class = r.symbols.iter().find(|s| s.name == "Person").unwrap();
    let prop = r.symbols.iter().find(|s| s.name == "name").unwrap();
    let method = r.symbols.iter().find(|s| s.name == "greet").unwrap();
    assert_eq!(prop.container, Some(class.id));
    assert_eq!(method.container, Some(class.id));
}

#[test]
fn references_inside_method_body_resolve_to_parameter() {
    let src = "function greet(name: String): String = name";
    let r = resolve(src);
    let param = r.symbols.iter().find(|s| s.name == "name").unwrap();
    // The body's `name` reference is past the equals sign.
    let body_ref_offset = src.rfind("name").unwrap() as u32;
    let resolved = r.symbol_at_offset(body_ref_offset).unwrap();
    assert_eq!(resolved, param.id);
}

#[test]
fn let_binding_is_visible_in_body_not_value() {
    let src = "x = let (y = 1) y + 2";
    let r = resolve(src);
    let y_let = r
        .symbols
        .iter()
        .find(|s| s.name == "y" && matches!(s.kind, SymbolKind::LetBinding))
        .expect("let binding registered");
    // The reference to `y` after `=` resolves to the let binding.
    let body_ref_offset = src.rfind("y").unwrap() as u32;
    let resolved = r.symbol_at_offset(body_ref_offset).unwrap();
    assert_eq!(resolved, y_let.id);
}

#[test]
fn lambda_parameter_shadows_outer() {
    let src = "outerName: String = \"a\"\nf = (outerName: Int) -> outerName";
    let r = resolve(src);
    let module_prop = r
        .symbols
        .iter()
        .find(|s| s.name == "outerName" && matches!(s.kind, SymbolKind::Property))
        .unwrap();
    let lambda_param = r
        .symbols
        .iter()
        .find(|s| s.name == "outerName" && matches!(s.kind, SymbolKind::Parameter))
        .unwrap();
    let body_ref_offset = src.rfind("outerName").unwrap() as u32;
    let resolved = r.symbol_at_offset(body_ref_offset).unwrap();
    assert_eq!(resolved, lambda_param.id);
    assert_ne!(resolved, module_prop.id);
}

#[test]
fn import_aliases_resolve() {
    let src = "import \"other.pkl\" as other\nx = other";
    let r = resolve(src);
    let import = r
        .symbols
        .iter()
        .find(|s| matches!(s.kind, SymbolKind::Import { .. }))
        .unwrap();
    assert_eq!(import.name, "other");
    let ref_offset = src.rfind("other").unwrap() as u32;
    let resolved = r.symbol_at_offset(ref_offset).unwrap();
    assert_eq!(resolved, import.id);
}

#[test]
fn type_parameter_resolves_inside_method() {
    let src = "function id<T>(x: T): T = x";
    let r = resolve(src);
    let t = r
        .symbols
        .iter()
        .find(|s| s.name == "T" && matches!(s.kind, SymbolKind::TypeParameter))
        .unwrap();
    // The return type annotation references `T`.
    let return_t_offset = offset_of(src, "): T") + 3;
    let resolved = r.symbol_at_offset(return_t_offset).unwrap();
    assert_eq!(resolved, t.id);
}

#[test]
fn for_binding_resolves_in_body() {
    let src = "servers {\n  for (i in List(1, 2, 3)) {\n    new { name = i }\n  }\n}";
    let r = resolve(src);
    let binding = r
        .symbols
        .iter()
        .find(|s| s.name == "i" && matches!(s.kind, SymbolKind::ForBinding))
        .unwrap();
    let body_ref_offset = src.rfind("i }").unwrap() as u32;
    let resolved = r.symbol_at_offset(body_ref_offset).unwrap();
    assert_eq!(resolved, binding.id);
}

#[test]
fn hover_includes_signature_and_doc() {
    let src = "/// The user's name.\nname: String = \"alice\"";
    let r = resolve(src);
    let prop = r.symbols.iter().find(|s| s.name == "name").unwrap();
    let md = hover_markdown(&r, prop);
    assert!(md.contains("```pkl"));
    assert!(md.contains("name: String"));
    assert!(md.contains("The user's name."));
}

#[test]
fn hover_for_class_member_shows_container() {
    let src = "class Person { name: String }";
    let r = resolve(src);
    let prop = r
        .symbols
        .iter()
        .find(|s| s.name == "name" && matches!(s.kind, SymbolKind::Property))
        .unwrap();
    let md = hover_markdown(&r, prop);
    assert!(md.contains("in class `Person`"), "got: {}", md);
}

#[test]
fn unresolved_identifier_is_not_recorded() {
    // `undefined_thing` doesn't exist; we shouldn't crash and shouldn't
    // associate any symbol with that offset.
    let src = "x = undefined_thing";
    let r = resolve(src);
    let offset = offset_of(src, "undefined_thing");
    assert!(r.symbol_at_offset(offset).is_none());
}

#[test]
fn ident_inside_string_interpolation_resolves_to_property() {
    // The `name` identifier inside `"hi \(name)"` should resolve to the
    // property declaration on the line above.
    let src = "name: String = \"alice\"\ngreeting = \"hi \\(name)!\"\n";
    let r = resolve(src);
    let name_sym = r
        .symbols
        .iter()
        .find(|s| s.name == "name" && matches!(s.kind, SymbolKind::Property))
        .expect("name property registered");
    // Offset of the `name` token *inside* the interpolation hole, not the
    // declaration. `rfind` lands on the hole occurrence.
    let interp_offset = src.rfind("name").unwrap() as u32;
    let resolved = r
        .symbol_at_offset(interp_offset)
        .expect("hole identifier should resolve");
    assert_eq!(resolved, name_sym.id);
}

#[test]
fn let_binding_visible_inside_interpolation_hole() {
    // The `y` inside `"\(y)"` should resolve to the let-binding.
    let src = "x = let (y = 1) \"y is \\(y)\"\n";
    let r = resolve(src);
    let y_let = r
        .symbols
        .iter()
        .find(|s| s.name == "y" && matches!(s.kind, SymbolKind::LetBinding))
        .expect("let binding registered");
    let interp_offset = src.rfind('y').unwrap() as u32;
    let resolved = r
        .symbol_at_offset(interp_offset)
        .expect("hole identifier resolves");
    assert_eq!(resolved, y_let.id);
}

#[test]
fn ident_inside_multiline_interpolation_resolves() {
    // A triple-quoted multiline string with an interpolation should
    // resolve the inner identifier just like the single-line case.
    let src = "name: String = \"alice\"\nx = \"\"\"\nhi \\(name)\n\"\"\"\n";
    let r = resolve(src);
    let name_sym = r
        .symbols
        .iter()
        .find(|s| s.name == "name" && matches!(s.kind, SymbolKind::Property))
        .expect("name property registered");
    let interp_offset = src.rfind("name").unwrap() as u32;
    let resolved = r
        .symbol_at_offset(interp_offset)
        .expect("hole identifier resolves");
    assert_eq!(resolved, name_sym.id);
}

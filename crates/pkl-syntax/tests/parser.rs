//! Smoke tests for the Pkl parser. These don't aim for full grammar
//! coverage — they pin the shapes the LSP foundation relies on.

use pkl_syntax::ast::*;
use pkl_syntax::parser::parse;

fn assert_clean(src: &str) {
    let r = parse(src);
    assert!(
        r.diagnostics.is_empty(),
        "expected no diagnostics, got: {:#?}\nsource:\n{}",
        r.diagnostics,
        src
    );
}

#[test]
fn parses_empty_module() {
    let r = parse("");
    assert!(r.module.header.is_none());
    assert!(r.module.imports.is_empty());
    assert!(r.module.items.is_empty());
    assert!(r.diagnostics.is_empty());
}

#[test]
fn parses_module_header_with_name() {
    let r = parse("module acme.config");
    let header = r.module.header.expect("header present");
    let name = header.name.expect("name present");
    let segs: Vec<_> = name.segments.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(segs, vec!["acme", "config"]);
    assert!(r.diagnostics.is_empty());
}

#[test]
fn parses_amends_clause() {
    let r = parse(r#"amends "base.pkl""#);
    let header = r.module.header.expect("header");
    match header.clause {
        Some(ExtendsAmendsClause::Amends { target, .. }) => {
            assert_eq!(target.raw, "\"base.pkl\"");
            assert_eq!(target.value.as_deref(), Some("base.pkl"));
        }
        _ => panic!("expected amends clause"),
    }
    assert!(r.diagnostics.is_empty());
}

#[test]
fn parses_import_with_alias() {
    let r = parse(r#"import "other.pkl" as other"#);
    assert_eq!(r.module.imports.len(), 1);
    let i = &r.module.imports[0];
    assert!(!i.is_glob);
    assert_eq!(i.alias.as_ref().unwrap().name, "other");
}

#[test]
fn parses_glob_import() {
    let r = parse(r#"import* "modules/*.pkl""#);
    assert!(r.module.imports[0].is_glob);
}

#[test]
fn parses_property_declaration() {
    let r = parse("name: String = \"alice\"");
    assert_clean("name: String = \"alice\"");
    let p = match &r.module.items[0] {
        Item::Property(p) => p,
        _ => panic!("expected property"),
    };
    assert_eq!(p.name.name, "name");
    assert!(p.ty.is_some());
    assert!(p.value.is_some());
}

#[test]
fn parses_property_with_object_body() {
    let r = parse("server { host = \"localhost\"; port = 8080 }");
    assert!(
        r.diagnostics.is_empty(),
        "diagnostics: {:#?}",
        r.diagnostics
    );
    let p = match &r.module.items[0] {
        Item::Property(p) => p,
        _ => panic!("expected property"),
    };
    assert!(matches!(p.value, Some(PropertyValue::ObjectBody(_))));
}

#[test]
fn parses_class_with_members() {
    let r = parse(
        r#"
class Person {
  name: String
  age: Int = 0
  function greet(other: String): String = "hi"
}
"#,
    );
    assert!(
        r.diagnostics.is_empty(),
        "diagnostics: {:#?}",
        r.diagnostics
    );
    let c = match &r.module.items[0] {
        Item::Class(c) => c,
        _ => panic!("expected class"),
    };
    assert_eq!(c.name.name, "Person");
    let body = c.body.as_ref().expect("body");
    assert_eq!(body.members.len(), 3);
}

#[test]
fn parses_typealias() {
    let r = parse("typealias Strings = List<String>");
    assert!(
        r.diagnostics.is_empty(),
        "diagnostics: {:#?}",
        r.diagnostics
    );
    let t = match &r.module.items[0] {
        Item::TypeAlias(t) => t,
        _ => panic!("expected typealias"),
    };
    assert_eq!(t.name.name, "Strings");
}

#[test]
fn parses_arithmetic_expression() {
    let r = parse("x = 1 + 2 * 3");
    assert!(r.diagnostics.is_empty());
    let p = match &r.module.items[0] {
        Item::Property(p) => p,
        _ => panic!(),
    };
    let value = match &p.value {
        Some(PropertyValue::Expr(e)) => e,
        _ => panic!(),
    };
    // The top-level should be `+`, not `*`.
    match value {
        Expr::Binary {
            op: BinaryOp::Add,
            lhs,
            rhs,
            ..
        } => {
            assert!(matches!(**lhs, Expr::Literal(Literal::Int { .. })));
            assert!(matches!(
                **rhs,
                Expr::Binary {
                    op: BinaryOp::Mul,
                    ..
                }
            ));
        }
        other => panic!("expected add at top, got {:?}", other),
    }
}

#[test]
fn parses_logical_and_or() {
    let r = parse("x = a || b && c");
    assert!(
        r.diagnostics.is_empty(),
        "diagnostics: {:#?}",
        r.diagnostics
    );
    let p = match &r.module.items[0] {
        Item::Property(p) => p,
        _ => panic!(),
    };
    let value = match &p.value {
        Some(PropertyValue::Expr(e)) => e,
        _ => panic!(),
    };
    match value {
        Expr::Binary {
            op: BinaryOp::Or,
            rhs,
            ..
        } => {
            assert!(matches!(
                **rhs,
                Expr::Binary {
                    op: BinaryOp::And,
                    ..
                }
            ));
        }
        other => panic!("expected ||, got {:?}", other),
    }
}

#[test]
fn parses_if_let_new() {
    let r = parse(
        r#"
x = if (a > 0) "pos" else "non"
y = let (z = 1) z + 2
z = new Foo { bar = 1 }
"#,
    );
    assert!(
        r.diagnostics.is_empty(),
        "diagnostics: {:#?}",
        r.diagnostics
    );
    assert_eq!(r.module.items.len(), 3);
}

#[test]
fn parses_lambda_and_call() {
    let r = parse("f = (x: Int, y: Int) -> x + y\nresult = f(1, 2)");
    assert!(
        r.diagnostics.is_empty(),
        "diagnostics: {:#?}",
        r.diagnostics
    );
}

#[test]
fn parses_when_and_for_in_object() {
    let r = parse(
        r#"
servers {
  for (i in List(1, 2, 3)) {
    new { name = i }
  }
  when (debug) {
    extra = true
  }
}
"#,
    );
    assert!(
        r.diagnostics.is_empty(),
        "diagnostics: {:#?}",
        r.diagnostics
    );
}

#[test]
fn parses_nullable_and_union_types() {
    let r = parse("x: String?\ny: String | Int | Null");
    assert!(
        r.diagnostics.is_empty(),
        "diagnostics: {:#?}",
        r.diagnostics
    );
}

#[test]
fn parses_doc_comment_attached_to_property() {
    let r = parse(
        r#"
/// The user's name.
/// Multi-line.
name: String = "alice"
"#,
    );
    assert!(
        r.diagnostics.is_empty(),
        "diagnostics: {:#?}",
        r.diagnostics
    );
    let p = match &r.module.items[0] {
        Item::Property(p) => p,
        _ => panic!(),
    };
    let doc = p.doc_comment.as_deref().unwrap();
    assert!(doc.contains("The user's name."));
    assert!(doc.contains("Multi-line."));
}

#[test]
fn recovers_from_garbage_token() {
    let r = parse("@ class Foo { name: String }\n??? unexpected\nclass Bar {}");
    // We should still see both classes despite the garbage in the middle.
    let names: Vec<_> = r.module.items.iter().filter_map(|i| i.name()).collect();
    assert!(names.contains(&"Foo"), "names = {:?}", names);
    assert!(names.contains(&"Bar"), "names = {:?}", names);
    assert!(!r.diagnostics.is_empty(), "expected diagnostics");
}

#[test]
fn annotations_attach_to_declarations() {
    let r = parse(
        r#"
@Deprecated
@Since { version = "1.0" }
function f(): Int = 1
"#,
    );
    assert!(
        r.diagnostics.is_empty(),
        "diagnostics: {:#?}",
        r.diagnostics
    );
    let m = match &r.module.items[0] {
        Item::Method(m) => m,
        _ => panic!(),
    };
    assert_eq!(m.annotations.len(), 2);
}

#[test]
fn tuple_type_yields_helpful_diagnostic() {
    let r = parse("x: (Int, String) = 1");
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.message.contains("no tuple types") && d.message.contains("Pair<A, B>")),
        "diagnostics: {:?}",
        r.diagnostics
    );
}

#[test]
fn modifiers_chain() {
    let r = parse("abstract open class Foo {}");
    assert!(
        r.diagnostics.is_empty(),
        "diagnostics: {:#?}",
        r.diagnostics
    );
    let c = match &r.module.items[0] {
        Item::Class(c) => c,
        _ => panic!(),
    };
    assert_eq!(c.modifiers.len(), 2);
}

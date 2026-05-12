//! Smoke tests for the Pkl parser. These don't aim for full grammar
//! coverage — they pin the shapes the LSP foundation relies on.

use pkl_syntax::cst::{
    AstNode, BinaryOp, ClauseKind, Expr, Item, LiteralKind, Module, PropertyValue,
};
use pkl_syntax::parser::parse;
use pkl_syntax::SyntaxKind;

fn parse_module(src: &str) -> (Module, Vec<pkl_syntax::SyntaxDiagnostic>) {
    let r = parse(src);
    let module = Module::cast(r.syntax()).expect("Module root");
    (module, r.diagnostics)
}

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
    let (m, diags) = parse_module("");
    assert!(m.header().is_none());
    assert!(m.imports().next().is_none());
    assert!(m.items().next().is_none());
    assert!(diags.is_empty());
}

#[test]
fn parses_module_header_with_name() {
    let (m, diags) = parse_module("module acme.config");
    let header = m.header().expect("header present");
    let name = header.name().expect("name present");
    let segs: Vec<String> = name
        .segments()
        .map(|t| pkl_syntax::cst::ident_text(&t))
        .collect();
    assert_eq!(segs, vec!["acme", "config"]);
    assert!(diags.is_empty());
}

#[test]
fn parses_amends_clause() {
    let (m, diags) = parse_module(r#"amends "base.pkl""#);
    let header = m.header().expect("header");
    let clause = header.clause().expect("clause");
    assert_eq!(clause.kind(), ClauseKind::Amends);
    let target = clause.target().expect("target string");
    assert_eq!(target.text(), "\"base.pkl\"");
    assert!(diags.is_empty());
}

#[test]
fn parses_import_with_alias() {
    let (m, _) = parse_module(r#"import "other.pkl" as other"#);
    let imports: Vec<_> = m.imports().collect();
    assert_eq!(imports.len(), 1);
    let i = &imports[0];
    assert!(!i.is_glob());
    let alias = i.alias().expect("alias");
    assert_eq!(pkl_syntax::cst::ident_text(&alias), "other");
}

#[test]
fn parses_glob_import() {
    let (m, _) = parse_module(r#"import* "modules/*.pkl""#);
    let imports: Vec<_> = m.imports().collect();
    assert!(imports[0].is_glob());
}

#[test]
fn parses_property_declaration() {
    let src = "name: String = \"alice\"";
    let (m, _) = parse_module(src);
    assert_clean(src);
    let p = match m.items().next().unwrap() {
        Item::Property(p) => p,
        _ => panic!("expected property"),
    };
    let name_tok = p.name().expect("name");
    assert_eq!(pkl_syntax::cst::ident_text(&name_tok), "name");
    assert!(p.ty().is_some());
    assert!(p.value().is_some());
}

#[test]
fn parses_property_with_object_body() {
    let src = "server { host = \"localhost\"; port = 8080 }";
    let (m, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);
    let p = match m.items().next().unwrap() {
        Item::Property(p) => p,
        _ => panic!("expected property"),
    };
    assert!(matches!(p.value(), Some(PropertyValue::ObjectBody(_))));
}

#[test]
fn parses_class_with_members() {
    let src = r#"
class Person {
  name: String
  age: Int = 0
  function greet(other: String): String = "hi"
}
"#;
    let (m, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);
    let c = match m.items().next().unwrap() {
        Item::Class(c) => c,
        _ => panic!("expected class"),
    };
    let name = c.name().expect("name");
    assert_eq!(pkl_syntax::cst::ident_text(&name), "Person");
    let body = c.body().expect("body");
    assert_eq!(body.members().count(), 3);
}

#[test]
fn parses_typealias() {
    let src = "typealias Strings = List<String>";
    let (m, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);
    let t = match m.items().next().unwrap() {
        Item::TypeAlias(t) => t,
        _ => panic!("expected typealias"),
    };
    let name = t.name().expect("name");
    assert_eq!(pkl_syntax::cst::ident_text(&name), "Strings");
}

#[test]
fn parses_arithmetic_expression() {
    let src = "x = 1 + 2 * 3";
    let (m, diags) = parse_module(src);
    assert!(diags.is_empty());
    let p = match m.items().next().unwrap() {
        Item::Property(p) => p,
        _ => panic!(),
    };
    let value = match p.value() {
        Some(PropertyValue::Expr(e)) => e,
        _ => panic!(),
    };
    // The top-level should be `+`, not `*`.
    let Expr::Binary(b) = &value else {
        panic!("expected binary at top, got {:?}", value);
    };
    assert_eq!(b.op(), Some(BinaryOp::Add));
    let lhs = b.lhs().expect("lhs");
    assert!(
        matches!(&lhs, Expr::Literal(l) if l.kind() == Some(LiteralKind::Int)),
        "expected int literal lhs"
    );
    let rhs = b.rhs().expect("rhs");
    let Expr::Binary(inner) = rhs else {
        panic!("expected inner binary");
    };
    assert_eq!(inner.op(), Some(BinaryOp::Mul));
}

#[test]
fn parses_logical_and_or() {
    let src = "x = a || b && c";
    let (m, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);
    let p = match m.items().next().unwrap() {
        Item::Property(p) => p,
        _ => panic!(),
    };
    let value = match p.value() {
        Some(PropertyValue::Expr(e)) => e,
        _ => panic!(),
    };
    let Expr::Binary(b) = &value else {
        panic!("expected ||, got {:?}", value);
    };
    assert_eq!(b.op(), Some(BinaryOp::Or));
    let rhs = b.rhs().expect("rhs");
    let Expr::Binary(inner) = rhs else {
        panic!("expected inner &&");
    };
    assert_eq!(inner.op(), Some(BinaryOp::And));
}

#[test]
fn parses_if_let_new() {
    let src = r#"
x = if (a > 0) "pos" else "non"
y = let (z = 1) z + 2
z = new Foo { bar = 1 }
"#;
    let (m, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);
    assert_eq!(m.items().count(), 3);
}

#[test]
fn parses_lambda_and_call() {
    let src = "f = (x: Int, y: Int) -> x + y\nresult = f(1, 2)";
    let (_, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);
}

#[test]
fn parses_when_and_for_in_object() {
    let src = r#"
servers {
  for (i in List(1, 2, 3)) {
    new { name = i }
  }
  when (debug) {
    extra = true
  }
}
"#;
    let (_, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);
}

#[test]
fn parses_nullable_and_union_types() {
    let src = "x: String?\ny: String | Int | Null";
    let (_, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);
}

#[test]
fn parses_doc_comment_attached_to_property() {
    let src = r#"
/// The user's name.
/// Multi-line.
name: String = "alice"
"#;
    let (m, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);
    let p = match m.items().next().unwrap() {
        Item::Property(p) => p,
        _ => panic!(),
    };
    let doc = p.doc_comment().expect("doc comment");
    assert!(doc.contains("The user's name."));
    assert!(doc.contains("Multi-line."));
}

#[test]
fn recovers_from_garbage_token() {
    let src = "@ class Foo { name: String }\n??? unexpected\nclass Bar {}";
    let r = parse(src);
    let module = Module::cast(r.syntax()).expect("module");
    // We should still see both classes despite the garbage in the middle.
    let names: Vec<String> = module.items().filter_map(|i| i.name()).collect();
    assert!(names.contains(&"Foo".to_string()), "names = {:?}", names);
    assert!(names.contains(&"Bar".to_string()), "names = {:?}", names);
    assert!(!r.diagnostics.is_empty(), "expected diagnostics");
}

#[test]
fn annotations_attach_to_declarations() {
    let src = r#"
@Deprecated
@Since { version = "1.0" }
function f(): Int = 1
"#;
    let (m, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);
    let method = match m.items().next().unwrap() {
        Item::Method(m) => m,
        _ => panic!(),
    };
    assert_eq!(method.annotations().count(), 2);
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
    let src = "abstract open class Foo {}";
    let (m, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);
    let c = match m.items().next().unwrap() {
        Item::Class(c) => c,
        _ => panic!(),
    };
    assert_eq!(c.modifiers().count(), 2);
}

#[test]
fn module_root_has_module_kind() {
    let r = parse("x = 1");
    assert_eq!(r.syntax().kind(), SyntaxKind::Module);
}

#[test]
fn collapses_cascading_eof_diagnostics() {
    // `if (` previously cascaded five "found end of file" diagnostics
    // (missing condition, `)`, then-branch, `else`, else-branch). Mid-
    // typing input only needs the first one.
    let r = parse("x = if (");
    assert_eq!(
        r.diagnostics.len(),
        1,
        "expected single diagnostic, got {:?}",
        r.diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
    );
    // The round-trip invariant still holds for malformed input.
    assert_eq!(r.syntax().text().to_string(), "x = if (");
}

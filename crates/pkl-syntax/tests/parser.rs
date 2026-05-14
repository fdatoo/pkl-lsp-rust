//! Smoke tests for the Pkl parser. These don't aim for full grammar
//! coverage — they pin the shapes the LSP foundation relies on.

use pkl_syntax::cst::{
    AstNode, BinaryOp, ClauseKind, Expr, Item, LiteralKind, Module, PropertyValue, Type,
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
fn parses_constrained_and_default_types() {
    let src = r#"
class Fibonacci {
  function fib(n: Int(this >= 0)): Int(this >= 0) = n
}
local typealias PklJobs = Mapping<String, *Workflow.Job>
"#;
    let (m, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);

    let class = match m.items().next().unwrap() {
        Item::Class(c) => c,
        _ => panic!("expected class"),
    };
    let method = class
        .body()
        .unwrap()
        .members()
        .find_map(|m| match m {
            pkl_syntax::cst::ClassMember::Method(m) => Some(m),
            _ => None,
        })
        .unwrap();
    let param = method.parameters().unwrap().parameters().next().unwrap();
    assert!(matches!(param.ty(), Some(Type::Constrained(_))));

    let alias = match m.items().nth(1).unwrap() {
        Item::TypeAlias(t) => t,
        _ => panic!("expected typealias"),
    };
    let text = alias.syntax().text().to_string();
    assert!(text.contains("*Workflow.Job"));
}

#[test]
fn parses_import_glob_expression() {
    let src = r#"fruit = import*("@fruities/catalog/*.pkl")"#;
    let (m, diags) = parse_module(src);
    assert!(diags.is_empty(), "diagnostics: {:#?}", diags);
    let prop = match m.items().next().unwrap() {
        Item::Property(p) => p,
        _ => panic!("expected property"),
    };
    let Some(PropertyValue::Expr(Expr::Import(import))) = prop.value() else {
        panic!("expected import expression");
    };
    assert!(import.argument().is_some());
}

#[test]
fn parses_computed_object_entries() {
    assert_clean(
        r#"
jobs = new {
  ["gradle-check"] = gradleCheck
  ["java-executables"] = (buildJavaExecutableJob) { isRelease = false }
  [[true]] { nightlyMacOS = false }
}
"#,
    );
}

#[test]
fn parses_object_amends_chains_and_truncating_division() {
    assert_clean(
        r#"
foo {
  bar { "Hello" }
} {
  bar { "World" }
}

examples {
  ["truncating division"] {
    5.kb ~/ 3
  }
}
"#,
    );
}

#[test]
fn parses_object_amend_lambda_parameters() {
    assert_clean(
        r#"
example {
  local f = (x, y, z) -> new Dynamic { prop3 = z }
  result1 = (f) { a, b: Number(this > 3) -> prop = a + b }.apply(1, 2)
  result = (f) { a: Foo, b: Bar, c: Baz -> prop1 = a; prop2 = b }.apply(new Foo {}, new Bar {}, new Baz {})
}
"#,
    );
}

#[test]
fn parses_trailing_commas_in_lambda_and_function_types() {
    assert_clean(
        r#"
local lA = (a, b, c,) -> true
local lB = (
  a: Int,
  b: Int,
) -> true
local lC: (Dynamic,) -> Dynamic = new Mixin { a, -> x = true }
local lD: (Dynamic,) -> Dynamic = new Mixin { a: Dynamic, -> x = true }
"#,
    );
}

#[test]
fn parses_union_types_in_casts_and_nested_indexes() {
    assert_clean(
        r#"
examples {
  ["union type"] {
    42 as Int|String
    List(1, 2, 3,) as List<String>|List<Int>
  }
}
result = mapping2[mapping1["x"]]
res12 {1.2;.3;.4;.5}
"#,
    );
}

#[test]
fn parses_class_body_semicolon_separators() {
    assert_clean(r#"local class Person { name: String = "Default"; age: Int }"#);
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

// ----------------------------------------------------------------------
// Error-recovery: mid-typing inputs that the LSP sees every keystroke.
//
// The invariant for every case: the round-trip holds, the expected
// outer node kind is produced, and we emit a short diagnostic so the
// LSP problems panel stays readable. Completion / hover / signature-
// help walk the CST through the typed accessors, all of which return
// `Option`s, so missing-child slots are already handled safely.

fn find_node(root: &pkl_syntax::SyntaxNode, kind: SyntaxKind) -> Option<pkl_syntax::SyntaxNode> {
    root.descendants().find(|n| n.kind() == kind)
}

fn assert_round_trip(src: &str, r: &pkl_syntax::parser::ParseResult) {
    let text = r.syntax().text().to_string();
    assert_eq!(text, src, "round-trip mismatch");
}

#[test]
fn recovery_trailing_dot_member_access() {
    let src = "x = foo.";
    let r = parse(src);
    assert_round_trip(src, &r);

    let member = find_node(&r.syntax(), SyntaxKind::MemberExpr).expect("MemberExpr present");
    // The MemberExpr should still wrap the receiver and the dot.
    assert!(member.text().to_string().ends_with('.'));

    let messages: Vec<&str> = r.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(messages.len(), 1, "diagnostics: {:?}", messages);
    assert!(
        messages[0].contains("expected member name"),
        "got: {:?}",
        messages
    );
}

#[test]
fn recovery_trailing_open_bracket_index() {
    let src = "x = foo[";
    let r = parse(src);
    assert_round_trip(src, &r);

    let index = find_node(&r.syntax(), SyntaxKind::IndexExpr).expect("IndexExpr present");
    // The error placeholder should be a direct child so analyzer
    // visitors keying on the expected child kind see a slot.
    assert!(
        find_node(&index, SyntaxKind::ErrorNode).is_some(),
        "ErrorNode missing from {}",
        index.text()
    );

    let messages: Vec<&str> = r.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(messages.len(), 1, "diagnostics: {:?}", messages);
    assert!(
        messages[0].contains("index expression"),
        "got: {:?}",
        messages
    );
}

#[test]
fn recovery_trailing_question_dot_member_access() {
    let src = "x = foo?.";
    let r = parse(src);
    assert_round_trip(src, &r);

    let member = find_node(&r.syntax(), SyntaxKind::MemberExpr).expect("MemberExpr present");
    assert!(member.text().to_string().contains("?."));
    assert_eq!(r.diagnostics.len(), 1);
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

#[test]
fn recovery_trailing_open_paren_call() {
    let src = "x = foo(";
    let r = parse(src);
    assert_round_trip(src, &r);

    let call = find_node(&r.syntax(), SyntaxKind::CallExpr).expect("CallExpr present");
    // The ArgList must be present so signature-help has a hook.
    assert!(
        find_node(&call, SyntaxKind::ArgList).is_some(),
        "ArgList missing from {}",
        call.text()
    );

    let messages: Vec<&str> = r.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(messages.len(), 1, "diagnostics: {:?}", messages);
    assert!(messages[0].contains("closing `)`"), "got: {:?}", messages);
}

#[test]
fn recovery_partial_call_with_arg_comma() {
    let src = "x = foo(1, 2,";
    let r = parse(src);
    assert_round_trip(src, &r);

    let call = find_node(&r.syntax(), SyntaxKind::CallExpr).expect("CallExpr present");
    let arg_list = find_node(&call, SyntaxKind::ArgList).expect("ArgList present");
    // Both numeric literal args should still be in the arg list so
    // signature-help knows which slot the cursor is on.
    let literals: Vec<_> = arg_list
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::LiteralExpr)
        .collect();
    assert_eq!(literals.len(), 2, "{}", arg_list.text());
    assert_eq!(r.diagnostics.len(), 1);
}

#[test]
fn recovery_partial_let_binding_no_body() {
    let src = "x = let (a = 1) ";
    let r = parse(src);
    assert_round_trip(src, &r);

    let let_expr = find_node(&r.syntax(), SyntaxKind::LetExpr).expect("LetExpr present");
    // The binding should still be there so the body sees `a` in scope.
    assert!(
        find_node(&let_expr, SyntaxKind::Parameter).is_some(),
        "Parameter missing from {}",
        let_expr.text()
    );
    assert_eq!(r.diagnostics.len(), 1);
}

#[test]
fn recovery_partial_new_with_open_brace() {
    let src = "x = new T {";
    let r = parse(src);
    assert_round_trip(src, &r);

    let new_expr = find_node(&r.syntax(), SyntaxKind::NewExpr).expect("NewExpr present");
    // The ObjectBody must be a child so object-member completion fires.
    assert!(
        find_node(&new_expr, SyntaxKind::ObjectBody).is_some(),
        "ObjectBody missing from {}",
        new_expr.text()
    );
    assert_eq!(r.diagnostics.len(), 1);
}

#[test]
fn recovery_partial_amends_with_open_brace() {
    let src = "x = foo {";
    let r = parse(src);
    assert_round_trip(src, &r);

    let amends = find_node(&r.syntax(), SyntaxKind::AmendsExpr).expect("AmendsExpr present");
    assert!(
        find_node(&amends, SyntaxKind::ObjectBody).is_some(),
        "ObjectBody missing from {}",
        amends.text()
    );
    assert_eq!(r.diagnostics.len(), 1);
}

#[test]
fn recovery_property_with_trailing_eq() {
    let src = "name: Int = ";
    let r = parse(src);
    assert_round_trip(src, &r);

    // The property must still parse as a PropertyDecl so completion
    // sees the property name in scope and signature-help knows the
    // expected type.
    let prop = match r
        .syntax()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::PropertyDecl)
    {
        Some(n) => n,
        None => panic!("PropertyDecl missing"),
    };
    let typed = pkl_syntax::cst::PropertyDecl::cast(prop).unwrap();
    assert!(typed.name().is_some());
    assert!(typed.ty().is_some());
    assert_eq!(r.diagnostics.len(), 1);
}

#[test]
fn recovery_round_trips_for_every_partial_input() {
    for src in [
        "x = foo.",
        "x = foo?.",
        "x = foo(",
        "x = foo(1,",
        "x = foo[",
        "x = let (a = 1) ",
        "x = if (",
        "x = new T {",
        "x = foo {",
        "name: Int = ",
        "y = foo.bar(",
        "z = bar.baz.",
    ] {
        let r = parse(src);
        assert_round_trip(src, &r);
    }
}

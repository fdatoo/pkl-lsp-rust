//! Tests for the type inferrer and member-access resolution.

use pkl_analyze::analyze;
use pkl_analyze::Ty;
use pkl_syntax::parse;

fn analyze_clean(src: &str) -> pkl_analyze::Analysis {
    let parsed = parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "syntax diagnostics: {:#?}\nsource:\n{}",
        parsed.diagnostics,
        src
    );
    analyze(&parsed.module, parsed.diagnostics)
}

/// Byte offset of the first occurrence of `needle` in `src`.
fn offset_of(src: &str, needle: &str) -> u32 {
    src.find(needle).expect("needle missing") as u32
}

#[test]
fn infers_literal_types() {
    let src = r#"
i = 1
f = 1.0
s = "hi"
b = true
n = null
"#;
    let a = analyze_clean(src);
    let inf = &a.inference;
    assert_eq!(inf.type_of(offset_of(src, "1\n")), Some(&Ty::Int));
    assert_eq!(inf.type_of(offset_of(src, "1.0")), Some(&Ty::Float));
    assert_eq!(inf.type_of(offset_of(src, "\"hi\"")), Some(&Ty::Str));
    assert_eq!(inf.type_of(offset_of(src, "true")), Some(&Ty::Boolean));
    assert_eq!(inf.type_of(offset_of(src, "null")), Some(&Ty::Null));
}

#[test]
fn infers_arithmetic() {
    let src = "x = 1 + 2";
    let a = analyze_clean(src);
    let ty = a.inference.type_of(offset_of(src, "1 + 2")).unwrap();
    assert_eq!(ty, &Ty::Int);
}

#[test]
fn string_concat_yields_string() {
    let src = r#"x = "a" + 1"#;
    let a = analyze_clean(src);
    let ty = a.inference.type_of(offset_of(src, "\"a\" + 1")).unwrap();
    assert_eq!(ty, &Ty::Str);
}

#[test]
fn comparison_yields_boolean() {
    let src = "x = 1 < 2";
    let a = analyze_clean(src);
    let ty = a.inference.type_of(offset_of(src, "1 < 2")).unwrap();
    assert_eq!(ty, &Ty::Boolean);
}

#[test]
fn if_branches_join() {
    let src = "x = if (true) 1 else 1.0";
    let a = analyze_clean(src);
    let ty = a.inference.type_of(offset_of(src, "if")).unwrap();
    // Int + Float should promote to Number (their LUB).
    assert_eq!(ty, &Ty::Number, "got {:?}", ty);
}

#[test]
fn property_type_seeds_identifier() {
    let src = r#"
name: String = "alice"
greeting = name
"#;
    let a = analyze_clean(src);
    let ty = a
        .inference
        .type_of(
            offset_of(src, "name\n"), /* identifier on second line */
        )
        .or_else(|| {
            // `name` may be parsed differently; find it after `greeting = `.
            a.inference
                .type_of(offset_of(src, "greeting = name") + "greeting = ".len() as u32)
        })
        .unwrap();
    assert_eq!(ty, &Ty::Str);
}

#[test]
fn member_on_string_resolves_to_length() {
    let src = "x = \"hello\".length";
    let a = analyze_clean(src);
    let offset = offset_of(src, "length");
    let member = a.inference.member_ref_touching(offset).expect("member ref");
    assert_eq!(member.member_name, "length");
    let stdlib_member = member.stdlib_member.expect("known stdlib member");
    assert_eq!(stdlib_member.signature, "length: Int");
    // The expression's overall type should be Int.
    let expr_ty = a
        .inference
        .type_of(offset_of(src, "\"hello\".length"))
        .unwrap();
    assert_eq!(expr_ty, &Ty::Int);
}

#[test]
fn method_call_propagates_return_type() {
    let src = r#"x = "hi".contains("h")"#;
    let a = analyze_clean(src);
    let member_offset = offset_of(src, "contains");
    let member = a
        .inference
        .member_ref_touching(member_offset)
        .expect("member ref");
    let sm = member.stdlib_member.unwrap();
    assert!(sm.signature.starts_with("contains"));
    // Call's overall result is Boolean.
    let call_offset = offset_of(src, "\"hi\".contains(\"h\")");
    let call_ty = a.inference.type_of(call_offset).unwrap();
    assert_eq!(call_ty, &Ty::Boolean);
}

#[test]
fn duration_unit_suffix_resolves() {
    let src = "x = 5.s";
    let a = analyze_clean(src);
    let member = a
        .inference
        .member_ref_touching(offset_of(src, ".s") + 1)
        .expect("found `.s` member");
    let sm = member.stdlib_member.unwrap();
    assert_eq!(sm.signature, "s: Duration");
    let ty = a.inference.type_of(offset_of(src, "5.s")).unwrap();
    assert_eq!(ty, &Ty::Duration);
}

#[test]
fn member_lookup_walks_parent_class() {
    // `length` is declared directly on `List` in the upstream
    // `pkl.base` source. The lookup should resolve there; falling
    // back to Collection is the chain we test elsewhere.
    let src = "xs = List(1, 2, 3).length";
    let a = analyze_clean(src);
    let member_offset = offset_of(src, "length");
    let member = a
        .inference
        .member_ref_touching(member_offset)
        .expect("member ref");
    assert_eq!(member.member_name, "length");
    let sm = member.stdlib_member.expect("scraped member");
    assert_eq!(sm.signature, "length: Int");
    let declaring = member.stdlib_type.unwrap().name;
    assert!(
        matches!(declaring, "List" | "Collection"),
        "expected List or Collection, got {}",
        declaring
    );
}

#[test]
fn unknown_receiver_records_member_but_no_resolution() {
    // Unresolved identifier — we don't know its type.
    let src = "x = unknownThing.foo";
    let parsed = parse(src);
    let a = analyze(&parsed.module, parsed.diagnostics);
    let member = a
        .inference
        .member_ref_touching(offset_of(src, "foo"))
        .expect("member ref still recorded");
    assert_eq!(member.member_name, "foo");
    assert!(member.stdlib_member.is_none());
    assert!(member.stdlib_type.is_none());
}

#[test]
fn null_coalesce_strips_nullable() {
    let src = "x: Int? = null\ny = x ?? 0";
    let a = analyze_clean(src);
    let ty = a.inference.type_of(offset_of(src, "x ?? 0")).unwrap();
    assert_eq!(ty, &Ty::Int);
}

#[test]
fn object_body_member_resolves_against_class() {
    let src = r#"
class Person {
  name: String
  age: Int
}

alice: Person = new {
  name = "Alice"
  age = 30
}
"#;
    let a = analyze_clean(src);
    // `name = "Alice"` should resolve to Person.name.
    let body_name = src.rfind("name =").unwrap() as u32;
    let member = a
        .inference
        .member_ref_touching(body_name)
        .expect("object body name resolves");
    assert!(member.user_member.is_some());
    let sym = a.resolution.symbol(member.user_member.unwrap());
    assert_eq!(sym.name, "name");
    assert_eq!(sym.declared_ty, Ty::Str);
}

#[test]
fn new_expression_seeds_object_body_type() {
    // `new Person { name = ... }` — explicit type at the `new`.
    let src = r#"
class Person { name: String }
x = new Person { name = "Bob" }
"#;
    let a = analyze_clean(src);
    let body_name = src.rfind("name =").unwrap() as u32;
    let member = a
        .inference
        .member_ref_touching(body_name)
        .expect("new Person body name resolves");
    assert!(member.user_member.is_some());
}

#[test]
fn qualified_type_name_records_member_ref() {
    let src = r#"import "other.pkl" as other
x: other.Thing = unknownThing"#;
    let parsed = pkl_syntax::parse(src);
    let a = pkl_analyze::analyze(&parsed.module, parsed.diagnostics);
    // The `Thing` segment in the type annotation — find via `.Thing`.
    let thing_offset = (src.find(".Thing").unwrap() + 1) as u32;
    let member = a
        .inference
        .member_ref_touching(thing_offset)
        .expect("qualified type records a member ref");
    assert_eq!(member.member_name, "Thing");
    // Receiver should be Module-shaped.
    assert_eq!(member.receiver_ty, pkl_analyze::Ty::Module);
}

#[test]
fn for_binding_narrowed_from_list_iterable() {
    let src = r#"
xs = List(1, 2, 3)
result = new Listing<Int> {
  for (x in xs) {
    x
  }
}
"#;
    let a = analyze_clean(src);
    // The `x` inside the body should be typed as Int.
    let body_x = src.rfind("x\n").unwrap() as u32;
    let ty = a.inference.type_of(body_x).expect("`x` body has a type");
    assert_eq!(ty, &pkl_analyze::Ty::Int);
}

#[test]
fn user_subtype_widens_through_extends_chain() {
    use pkl_analyze::subtyping::is_subtype;
    let src = r#"
class Animal {}
class Dog extends Animal {}
class Puppy extends Dog {}
"#;
    let parsed = pkl_syntax::parse(src);
    let a = pkl_analyze::analyze(&parsed.module, parsed.diagnostics);
    let dog = pkl_analyze::Ty::Named {
        name: "Dog".into(),
        args: vec![],
    };
    let animal = pkl_analyze::Ty::Named {
        name: "Animal".into(),
        args: vec![],
    };
    let puppy = pkl_analyze::Ty::Named {
        name: "Puppy".into(),
        args: vec![],
    };
    assert!(is_subtype(&dog, &animal, &a.resolution));
    assert!(is_subtype(&puppy, &animal, &a.resolution));
    assert!(!is_subtype(&animal, &dog, &a.resolution));
}

#[test]
fn method_body_return_type_diagnostic() {
    let src = "function f(): Int = \"oops\"";
    let parsed = pkl_syntax::parse(src);
    let a = pkl_analyze::analyze(&parsed.module, parsed.diagnostics);
    let msgs: Vec<&str> = a.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(
        msgs.iter().any(|m| m.contains("type mismatch")),
        "expected type mismatch diagnostic, got: {:?}",
        msgs
    );
}

#[test]
fn lambda_param_inferred_from_call_context() {
    // `xs.map((x) -> x.length)` — `x` should be narrowed to String so the
    // inferrer resolves `x.length` against String, not Unknown.
    let src = "lengths = List(\"a\", \"bc\", \"def\").map((x) -> x.length)";
    let a = analyze_clean(src);
    let length_offset = src.rfind("length").unwrap() as u32;
    let member = a
        .inference
        .member_ref_touching(length_offset)
        .expect("inferred lambda parameter resolves member");
    assert_eq!(member.member_name, "length");
    assert!(member.stdlib_member.is_some());
    assert_eq!(member.stdlib_type.unwrap().name, "String");
}

#[test]
fn type_check_narrows_then_branch() {
    let src = r#"function f(x: Any): Int = if (x is String) x.length else 0"#;
    let a = analyze_clean(src);
    // The `x.length` expression should now type as Int because `x` was
    // narrowed to String inside the then-branch.
    let dot_offset = src.rfind("length").unwrap() as u32;
    let member = a
        .inference
        .member_ref_touching(dot_offset)
        .expect("narrowed member resolves");
    assert_eq!(member.member_name, "length");
    let sm = member.stdlib_member.expect("stdlib member resolved");
    assert_eq!(sm.signature, "length: Int");
}

#[test]
fn inherited_member_resolves_through_extends_chain() {
    let src = r#"
class Animal {
  name: String
}
class Dog extends Animal {
  bark: String
}
function greet(d: Dog): String = d.name
"#;
    let a = analyze_clean(src);
    let name_offset = src.rfind("name").unwrap() as u32;
    let member = a
        .inference
        .member_ref_touching(name_offset)
        .expect("inherited member resolves");
    assert!(
        member.user_member.is_some(),
        "expected resolved user member, got {:?}",
        member
    );
    let sym = a.resolution.symbol(member.user_member.unwrap());
    assert_eq!(sym.name, "name");
}

#[test]
fn super_inside_method_picks_up_parent_class() {
    let src = r#"
class Animal {
  name: String
}
class Dog extends Animal {
  greeting: String = super.name
}
"#;
    let a = analyze_clean(src);
    let name_offset = src.rfind("name").unwrap() as u32;
    let member = a
        .inference
        .member_ref_touching(name_offset)
        .expect("super.name resolves");
    assert!(member.user_member.is_some());
}

#[test]
fn user_class_member_resolves() {
    let src = r#"
class Person {
  name: String
  age: Int
}
function whoIs(p: Person): String = p.name
"#;
    let a = analyze_clean(src);
    let name_offset = src.rfind("name").unwrap() as u32;
    let member = a
        .inference
        .member_ref_touching(name_offset)
        .expect("member ref");
    assert!(
        member.user_member.is_some(),
        "expected a user member, got {:?}",
        member
    );
    let sym = a.resolution.symbol(member.user_member.unwrap());
    assert_eq!(sym.name, "name");
    // Member type should match the declared `name: String`.
    assert!(member.stdlib_member.is_none());
}

#[test]
fn list_ctor_propagates_element_type() {
    let src = "xs = List(1, 2, 3)";
    let a = analyze_clean(src);
    let ty = a.inference.type_of(offset_of(src, "List(")).unwrap();
    match ty {
        Ty::List(inner) => assert_eq!(**inner, Ty::Int),
        other => panic!("expected List<Int>, got {:?}", other),
    }
}

#[test]
fn list_first_yields_element_type() {
    let src = "x = List(\"a\", \"b\").first";
    let a = analyze_clean(src);
    let ty = a
        .inference
        .type_of(offset_of(src, "List(\"a\", \"b\").first"))
        .unwrap();
    assert_eq!(ty, &Ty::Str);
}

#[test]
fn map_substitutes_lambda_return_type() {
    // `List(...).map(lambda)` should infer the lambda's return type and
    // bind R, so the call result is `List<that-return-type>`.
    let src = "xs = List(1, 2).map((x: Int) -> \"hi\")";
    let a = analyze_clean(src);
    let ty = a
        .inference
        .type_of(offset_of(src, "List(1, 2).map((x: Int) -> \"hi\")"))
        .unwrap();
    match ty {
        Ty::List(inner) => assert_eq!(**inner, Ty::Str),
        other => panic!("expected List<String>, got {:?}", other),
    }
}

#[test]
fn diagnostic_for_type_mismatch_in_property() {
    let src = "name: String = 42";
    let parsed = pkl_syntax::parse(src);
    let a = pkl_analyze::analyze(&parsed.module, parsed.diagnostics);
    let msgs: Vec<&str> = a.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(
        msgs.iter().any(|m| m.contains("type mismatch")),
        "expected type mismatch diagnostic, got {:?}",
        msgs
    );
}

#[test]
fn no_diagnostic_when_inferrer_is_unsure() {
    // `unknownThing` has no type — be permissive.
    let src = "name: String = unknownThing";
    let parsed = pkl_syntax::parse(src);
    let a = pkl_analyze::analyze(&parsed.module, parsed.diagnostics);
    assert!(
        !a.diagnostics
            .iter()
            .any(|d| d.message.contains("type mismatch")),
        "got diagnostics: {:?}",
        a.diagnostics
    );
}

#[test]
fn nullable_accepts_concrete_value() {
    let src = "name: String? = \"a\"";
    let parsed = pkl_syntax::parse(src);
    let a = pkl_analyze::analyze(&parsed.module, parsed.diagnostics);
    let msgs: Vec<&str> = a.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(msgs.is_empty(), "got diagnostics: {:?}", msgs);
}

#[test]
fn lambda_has_function_type() {
    let src = "f = (x: Int) -> x";
    let a = analyze_clean(src);
    let ty = a.inference.type_of(offset_of(src, "(x: Int)")).unwrap();
    match ty {
        Ty::Function { params, ret } => {
            assert_eq!(params, &vec![Ty::Int]);
            assert!(matches!(**ret, Ty::Int));
        }
        other => panic!("expected function, got {:?}", other),
    }
}

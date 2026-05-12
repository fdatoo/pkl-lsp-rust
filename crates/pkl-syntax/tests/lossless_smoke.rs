//! Round-trip + smoke fuzz coverage for the lossless syntax tree.
//!
//! These tests exercise [`pkl_syntax::parse_green`] and the typed
//! wrappers in [`pkl_syntax::cst`] over a wide range of inputs:
//!
//! * Curated Pkl samples covering the full grammar.
//! * A handful of mutated variants of short seeds (byte drops and
//!   targeted byte inserts), to make sure error-recovery still
//!   reconstructs the source verbatim.
//! * A deterministically seeded byte fuzzer producing random ASCII
//!   strings to stress the parser without ever asking it to panic.
//!
//! Every input must satisfy:
//!
//! 1. `parse_green` returns without panicking.
//! 2. The tree's `text().to_string()` reproduces the input verbatim
//!    (the round-trip property).
//! 3. The root node has kind `Module`.

use pkl_syntax::cst::{AstNode, Item, Module, PropertyValue};
use pkl_syntax::{parse_green, SyntaxKind};

const SEEDS: &[&str] = &[
    "",
    " ",
    "\n\n\n",
    "// just a comment\n",
    "/* block */",
    "/// doc only\n",
    "module foo",
    "module foo.bar.baz\n",
    "amends \"base.pkl\"\n",
    "extends \"base.pkl\"\n",
    "import \"a.pkl\"\nimport* \"b/*.pkl\" as bs\n",
    "x = 1\n",
    "x: Int = 1\n",
    "x = 1 + 2 * 3 - 4 / 5 % 6 ** 7\n",
    "x = a || b && c == d != e < f <= g > h >= i\n",
    "x = if (cond) then else otherwise\n",
    "x = let (y = 1) y + 2\n",
    "x = foo.bar(1, 2, 3)\n",
    "x = list[0]!!\n",
    "x = (a, b) -> a + b\n",
    "x = new Foo { y = 1 }\n",
    "x = base { y = override }\n",
    "x = read(\"path\")\n",
    "x = read?(\"path\")\n",
    "x = read*(\"glob/*.txt\")\n",
    "x = throw(\"boom\")\n",
    "x = trace(\"hi\")\n",
    "x: \"literal\" = \"literal\"\n",
    "x: Foo | Bar | Baz = a\n",
    "x: (Int, String) -> Bool = (a, b) -> true\n",
    "x: List<Map<String, Int>> = List()\n",
    "x: Int? = null\n",
    "class Foo {}\n",
    "class Foo extends Bar { x: Int = 1 }\n",
    "open class Foo<out T> { value: T }\n",
    "abstract class A {}\n",
    "typealias StringMap<V> = Map<String, V>\n",
    "function f(x: Int): Int = x + 1\n",
    "function id<T>(x: T): T = x\n",
    "@Deprecated\nx: Int = 1\n",
    "@Since { version = \"1\" }\n@Deprecated\nfunction f(): Int = 1\n",
    "config {\n  when (cond) { x = 1 } else { x = 2 }\n  for (i in xs) { [i] = i * 2 }\n}\n",
    "obj { ...spread; bare; [\"key\"] = value; foo = bar }\n",
    "x = \"hello\\nworld\"\n",
    "x = \"\"\"\n  multiline\n  \"\"\"\n",
    "x = `class`\n",
    "/// outer doc\n/// continues\nclass Foo {\n  /// inner doc\n  prop: Int\n}\n",
    "x = 1.s + 2.ms + 3.gib\n",
    "module foo\nimport \"a\"\nclass X {}\nx = 1\nfunction f() = 1\ntypealias Y = X\n",
];

fn check_round_trip(src: &str) {
    let parsed = parse_green(src);
    let reconstructed = parsed.syntax.text().to_string();
    assert_eq!(
        src, reconstructed,
        "round-trip mismatch\n--- input ---\n{src}\n--- output ---\n{reconstructed}\n"
    );
    assert_eq!(parsed.syntax.kind(), SyntaxKind::Module);
    // Module wrapper must cast cleanly.
    assert!(Module::cast(parsed.syntax).is_some());
}

#[test]
fn seeds_round_trip() {
    for seed in SEEDS {
        check_round_trip(seed);
    }
}

/// Targeted mutations of a single seed. Produces a bounded number of
/// variants (single-byte drop and single-byte insert) so the whole
/// matrix completes in well under a second.
fn mutate(seed: &str) -> Vec<String> {
    let mut out = Vec::new();
    out.push(seed.to_string());
    for i in 0..seed.len() {
        if !seed.is_char_boundary(i) {
            continue;
        }
        let mut next_boundary = i + 1;
        while next_boundary < seed.len() && !seed.is_char_boundary(next_boundary) {
            next_boundary += 1;
        }
        if next_boundary <= seed.len() {
            let mut s = seed.to_string();
            s.replace_range(i..next_boundary, "");
            out.push(s);
        }
        for &c in &['{', '}', '(', ')', '"', '@', '/'] {
            let mut s = seed.to_string();
            s.insert(i, c);
            out.push(s);
        }
    }
    out
}

#[test]
fn mutated_short_seeds_round_trip() {
    // Restrict to short seeds — the harness already exercises long
    // seeds verbatim in `seeds_round_trip`.
    for seed in SEEDS.iter().filter(|s| s.len() <= 40) {
        for input in mutate(seed) {
            let parsed = parse_green(&input);
            let reconstructed = parsed.syntax.text().to_string();
            assert_eq!(
                input, reconstructed,
                "round-trip mismatch on mutated input\n--- input ---\n{input}\n--- output ---\n{reconstructed}\n"
            );
            assert_eq!(parsed.syntax.kind(), SyntaxKind::Module);
        }
    }
}

#[test]
fn lossless_handles_deeply_nested_input() {
    let mut s = String::new();
    for _ in 0..200 {
        s.push_str("new { ");
    }
    for _ in 0..200 {
        s.push_str(" }");
    }
    let parsed = parse_green(&s);
    assert_eq!(parsed.syntax.text().to_string(), s);
}

/// xorshift32: a deterministic, allocation-free PRNG suitable for fuzz
/// seeding without pulling in a `rand` dependency.
struct XorShift(u32);
impl XorShift {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

#[test]
fn random_ascii_inputs_round_trip() {
    // Restricted alphabet that yields realistic-ish Pkl shapes: brackets,
    // operators, identifiers, digits, whitespace. ASCII-only so we
    // never have to worry about codepoint splits.
    const ALPHABET: &[u8] =
        b" \n\t{}()[]<>:;,.=+-*/%!?@\"\\|&abcdefghijklmnopqrstuvwxyz_0123456789";
    let mut rng = XorShift(0x9E37_79B9);
    for trial in 0..50 {
        let len = (rng.next() % 80) as usize;
        let mut bytes = Vec::with_capacity(len);
        for _ in 0..len {
            let idx = (rng.next() as usize) % ALPHABET.len();
            bytes.push(ALPHABET[idx]);
        }
        let input = String::from_utf8(bytes).expect("ASCII-only");
        let parsed = parse_green(&input);
        let reconstructed = parsed.syntax.text().to_string();
        assert_eq!(
            input, reconstructed,
            "random fuzz trial {trial} broke round-trip\n--- input ---\n{input}\n--- output ---\n{reconstructed}\n"
        );
        assert_eq!(parsed.syntax.kind(), SyntaxKind::Module);
    }
}

#[test]
fn trivia_is_preserved_inside_declarations() {
    let src = "// before\n\
               /// doc\n\
               class Foo {\n\
              \x20 // inside\n\
              \x20 x: Int = 1 // trailing\n\
               }\n";
    let parsed = parse_green(src);
    assert_eq!(parsed.syntax.text().to_string(), src);
    let module = Module::cast(parsed.syntax).unwrap();
    let cls = module
        .items()
        .find_map(|i| match i {
            Item::Class(c) => Some(c),
            _ => None,
        })
        .expect("class present");
    assert_eq!(cls.doc_comment().as_deref(), Some("doc"));
}

#[test]
fn typed_view_walks_full_sample() {
    let src = "module sample\n\
               import \"base.pkl\" as base\n\
               /// the answer\n\
               theAnswer: Int = 42\n\
               class Box<T> {\n\
              \x20 value: T\n\
              \x20 function unwrap(): T = value\n\
               }\n";
    let parsed = parse_green(src);
    assert_eq!(parsed.syntax.text().to_string(), src);

    let m = Module::cast(parsed.syntax).expect("module");
    let header = m.header().expect("header");
    assert_eq!(header.name().expect("name").text_joined(), "sample");
    assert_eq!(m.imports().count(), 1);
    let items: Vec<_> = m.items().collect();
    assert_eq!(items.len(), 2);
    match &items[0] {
        Item::Property(p) => {
            assert_eq!(p.doc_comment().as_deref(), Some("the answer"));
            assert!(matches!(p.value(), Some(PropertyValue::Expr(_))));
        }
        _ => panic!("expected property"),
    }
    match &items[1] {
        Item::Class(c) => {
            let body = c.body().expect("class body");
            assert_eq!(body.members().count(), 2);
        }
        _ => panic!("expected class"),
    }
}

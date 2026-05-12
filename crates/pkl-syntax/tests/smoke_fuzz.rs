//! Lightweight "smoke fuzz": runs the parser over many deterministically
//! generated inputs to catch panics without needing a nightly toolchain.
//! The real `cargo fuzz` targets live under `/fuzz`.

use pkl_syntax::parse;

const SEEDS: &[&str] = &[
    "",
    " ",
    "module foo",
    "class",
    "class A {",
    "x = ",
    "\"unterminated",
    "/* unterminated",
    "import \"x\"",
    "function f(): Int = 1 + 2",
    "x: List<Map<String, Int>> = List()",
    "new { ... new { ... } }",
    "let (x = 1) let (y = 2) x + y",
    "if (a) if (b) c else d else e",
    "a.b.c.d.e",
    "/* nested /* still */ comment */",
    "module foo\namends \"x\"\nx: Int = 1",
    "@Deprecated\n@Since { version = \"1\" }\nfunction f(): Int = 1",
    "for (x in y) for (z in w) new { a = z }",
    "x = 1.s + 2.ms + 3.gib",
];

/// Mutate `seed` into many short variants by inserting, deleting, and
/// swapping bytes at a few positions. We don't aim for clever coverage —
/// the goal is to crash on parser panics.
fn mutate(seed: &str) -> Vec<String> {
    let mut out = Vec::new();
    out.push(seed.to_string());
    for i in 0..seed.len() {
        // Drop one byte.
        let mut s = seed.to_string();
        s.remove(i);
        out.push(s);
        // Insert a brace, paren, quote, or backslash.
        for &c in &['{', '}', '(', ')', '"', '\\', ';'] {
            let mut s = seed.to_string();
            s.insert(i, c);
            out.push(s);
        }
    }
    // Truncations.
    for take in 0..seed.len() {
        out.push(seed[..take].to_string());
    }
    out
}

#[test]
fn parser_does_not_panic_on_mutated_seeds() {
    for seed in SEEDS {
        for input in mutate(seed) {
            // Just don't panic. Diagnostics are fine.
            let _ = parse(&input);
        }
    }
}

#[test]
fn parser_handles_extremely_nested_input() {
    let mut s = String::new();
    for _ in 0..500 {
        s.push_str("new { ");
    }
    for _ in 0..500 {
        s.push_str(" }");
    }
    let _ = parse(&s);
}

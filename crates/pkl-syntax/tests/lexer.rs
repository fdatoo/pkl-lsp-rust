//! End-to-end tests for the Pkl lexer.

use pkl_syntax::kind::SyntaxKind;
use pkl_syntax::lexer::tokenize;

fn kinds(src: &str) -> Vec<SyntaxKind> {
    tokenize(src)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .map(|t| t.kind)
        .collect()
}

fn all_kinds(src: &str) -> Vec<SyntaxKind> {
    tokenize(src).into_iter().map(|t| t.kind).collect()
}

#[test]
fn lexes_identifiers_and_keywords() {
    assert_eq!(
        kinds("class Foo extends Bar"),
        vec![
            SyntaxKind::ClassKw,
            SyntaxKind::Ident,
            SyntaxKind::ExtendsKw,
            SyntaxKind::Ident,
            SyntaxKind::Eof,
        ]
    );
}

#[test]
fn lexes_backtick_identifier() {
    let toks = tokenize("`class` = 1");
    let kinds: Vec<_> = toks
        .iter()
        .filter(|t| !t.is_trivia())
        .map(|t| t.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            SyntaxKind::QuotedIdent,
            SyntaxKind::Eq,
            SyntaxKind::IntNumber,
            SyntaxKind::Eof,
        ]
    );
}

#[test]
fn lexes_numbers() {
    assert_eq!(
        kinds("0 12 1_000 3.14 1e5 1.0e-3 0xFF 0b1010 0o755"),
        vec![
            SyntaxKind::IntNumber,
            SyntaxKind::IntNumber,
            SyntaxKind::IntNumber,
            SyntaxKind::FloatNumber,
            SyntaxKind::FloatNumber,
            SyntaxKind::FloatNumber,
            SyntaxKind::HexNumber,
            SyntaxKind::BinNumber,
            SyntaxKind::OctNumber,
            SyntaxKind::Eof,
        ]
    );
}

#[test]
fn does_not_confuse_member_access_with_float() {
    // `foo.bar` should not be tokenized as a float.
    assert_eq!(
        kinds("foo.bar"),
        vec![
            SyntaxKind::Ident,
            SyntaxKind::Dot,
            SyntaxKind::Ident,
            SyntaxKind::Eof
        ]
    );
}

#[test]
fn lexes_strings() {
    assert_eq!(
        kinds("\"hello\" \"a\\nb\""),
        vec![SyntaxKind::String, SyntaxKind::String, SyntaxKind::Eof]
    );
}

#[test]
fn lexes_multiline_string() {
    let src = "\"\"\"\n  hello\n  world\n  \"\"\"";
    assert_eq!(
        kinds(src),
        vec![SyntaxKind::MultilineString, SyntaxKind::Eof]
    );
}

#[test]
fn lexes_custom_delim_string() {
    let src = r##"#"raw "quotes" inside"#"##;
    assert_eq!(kinds(src), vec![SyntaxKind::String, SyntaxKind::Eof]);
}

#[test]
fn lexes_operators_and_punctuation() {
    assert_eq!(
        kinds("== != <= >= -> => ?. ?? || && ... |> **"),
        vec![
            SyntaxKind::EqEq,
            SyntaxKind::BangEq,
            SyntaxKind::LtEq,
            SyntaxKind::GtEq,
            SyntaxKind::Arrow,
            SyntaxKind::FatArrow,
            SyntaxKind::QuestionDot,
            SyntaxKind::QuestionQuestion,
            // `||` is `Pipe Pipe` at lexer level.
            SyntaxKind::Pipe,
            SyntaxKind::Pipe,
            // `&&` is `Amp Amp` at lexer level.
            SyntaxKind::Amp,
            SyntaxKind::Amp,
            SyntaxKind::Ellipsis,
            SyntaxKind::PipeGt,
            SyntaxKind::StarStar,
            SyntaxKind::Eof,
        ]
    );
}

#[test]
fn double_slash_is_a_comment_not_integer_division() {
    // Pkl uses `//` for line comments; integer division is `Int.div(other)`.
    let toks = tokenize("a // comment\nb");
    let kinds: Vec<_> = toks
        .iter()
        .filter(|t| !t.is_trivia())
        .map(|t| t.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![SyntaxKind::Ident, SyntaxKind::Ident, SyntaxKind::Eof]
    );
}

#[test]
fn lexes_compound_keywords() {
    assert_eq!(
        kinds("import* read? read*"),
        vec![
            SyntaxKind::ImportGlobKw,
            SyntaxKind::ReadOrNullKw,
            SyntaxKind::ReadGlobKw,
            SyntaxKind::Eof,
        ]
    );
}

#[test]
fn preserves_trivia() {
    let all = all_kinds("// hi\nx = 1");
    assert!(all.contains(&SyntaxKind::LineComment));
    assert!(all.contains(&SyntaxKind::Newline));
    assert!(all.contains(&SyntaxKind::Whitespace));
}

#[test]
fn lexes_nested_block_comment() {
    let toks = tokenize("/* outer /* inner */ still */ x");
    let comments: Vec<_> = toks
        .iter()
        .filter(|t| t.kind == SyntaxKind::BlockComment)
        .collect();
    assert_eq!(
        comments.len(),
        1,
        "nested block comments collapse to one token"
    );
    let last_significant = toks
        .iter()
        .rfind(|t| !t.is_trivia() && t.kind != SyntaxKind::Eof)
        .unwrap();
    assert_eq!(last_significant.kind, SyntaxKind::Ident);
    assert_eq!(last_significant.text, "x");
}

#[test]
fn lexes_doc_comment_vs_line_comment() {
    let toks = tokenize("/// doc\n// not doc\nx");
    let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
    assert!(kinds.contains(&SyntaxKind::DocComment));
    assert!(kinds.contains(&SyntaxKind::LineComment));
}

#[test]
fn unterminated_string_is_error() {
    let toks = tokenize("\"oops\n");
    let first = toks.iter().find(|t| !t.is_trivia()).unwrap();
    assert_eq!(first.kind, SyntaxKind::Error);
}

#[test]
fn handles_unicode_identifier() {
    let toks = tokenize("café = 1");
    let kinds: Vec<_> = toks
        .iter()
        .filter(|t| !t.is_trivia())
        .map(|t| t.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            SyntaxKind::Ident,
            SyntaxKind::Eq,
            SyntaxKind::IntNumber,
            SyntaxKind::Eof,
        ]
    );
}

#[test]
fn spans_are_contiguous_and_cover_source() {
    let src = "x = 1 + 2";
    let toks = tokenize(src);
    let mut cursor = 0u32;
    for t in &toks {
        if t.kind == SyntaxKind::Eof {
            assert_eq!(t.span.start as usize, src.len());
            break;
        }
        assert_eq!(t.span.start, cursor, "gap before {:?}", t);
        cursor = t.span.end;
    }
}

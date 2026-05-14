//! `SyntaxKind` enumerates every token (and, eventually, every node) the
//! Pkl grammar can produce. Keeping this in a single enum lets the parser and
//! later passes pattern-match exhaustively.

use std::fmt;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u16)]
pub enum SyntaxKind {
    // Trivia ----------------------------------------------------------------
    Whitespace,
    Newline,
    LineComment,
    BlockComment,
    DocComment,

    // Literals --------------------------------------------------------------
    IntNumber,
    FloatNumber,
    HexNumber,
    BinNumber,
    OctNumber,
    /// A single-line string literal, including its quotes. Used only when the
    /// string contains no interpolation; an interpolated string is decomposed
    /// into [`StringQuoteOpen`], [`StringPart`], [`Interpolation`] nodes, and
    /// [`StringQuoteClose`] tokens under an [`InterpolatedString`] node.
    String,
    /// A multi-line `"""..."""` string literal. As with [`String`], this is
    /// emitted only when there is no interpolation; otherwise the string is
    /// decomposed into [`InterpolatedMultilineString`] components.
    MultilineString,
    /// Opening quote token of an interpolated string (a single `"`, `"""`,
    /// or one of the `#"...`-style custom-delimited variants). The exact
    /// text identifies the flavour and hash count of the string.
    StringQuoteOpen,
    /// Closing quote token of an interpolated string, matching the opening
    /// flavour.
    StringQuoteClose,
    /// The literal-text run between escapes/interpolations in an
    /// interpolated single-line string.
    StringPart,
    /// The literal-text run between escapes/interpolations in an
    /// interpolated multi-line string.
    MultilineStringPart,
    /// Opening marker of an interpolation hole: `\(`, or `#\(`/`##\(`/...
    /// for custom-delimited strings.
    InterpolationStart,
    /// Closing marker of an interpolation hole: `)`, or `)#`/`)##`/... for
    /// custom-delimited strings.
    InterpolationEnd,

    // Identifier and keywords ----------------------------------------------
    Ident,
    /// A backtick-quoted identifier, used when a Pkl identifier collides with
    /// a keyword (e.g. ``` `class` = "Foo" ```).
    QuotedIdent,

    // Keywords -- keep in sync with `keyword_from_str` below.
    AbstractKw,
    AmendsKw,
    AsKw,
    ClassKw,
    ElseKw,
    ExtendsKw,
    ExternalKw,
    FalseKw,
    FixedKw,
    ForKw,
    FunctionKw,
    HiddenKw,
    IfKw,
    ImportKw,
    ImportGlobKw,
    InKw,
    IsKw,
    LetKw,
    LocalKw,
    ModuleKw,
    NewKw,
    NothingKw,
    NullKw,
    OpenKw,
    OutKw,
    OuterKw,
    ReadKw,
    ReadOrNullKw,
    ReadGlobKw,
    SuperKw,
    ThisKw,
    ThrowKw,
    TraceKw,
    TrueKw,
    TypeAliasKw,
    UnknownKw,
    WhenKw,

    // Punctuation -----------------------------------------------------------
    LBrace,         // {
    RBrace,         // }
    LParen,         // (
    RParen,         // )
    LBracket,       // [
    RBracket,       // ]
    LDoubleBracket, // [[
    RDoubleBracket, // ]]
    Comma,
    Dot,
    DotDot,
    Ellipsis, // ...
    Semicolon,
    Colon,
    ColonColon,
    Arrow,            // ->
    FatArrow,         // =>
    At,               // @
    Pipe,             // |
    PipeGt,           // |>
    Amp,              // &
    Question,         // ?
    QuestionDot,      // ?.
    QuestionQuestion, // ??
    Eq,               // =
    EqEq,             // ==
    Bang,             // !
    BangEq,           // !=
    Lt,               // <
    LtEq,             // <=
    Gt,               // >
    GtEq,             // >=
    Plus,             // +
    Minus,            // -
    Star,             // *
    Slash,            // /
    TildeSlash,       // ~/
    Percent,          // %
    StarStar,         // **

    // Markers ---------------------------------------------------------------
    /// Lexer error token; the rest of the field carries the bad text.
    Error,
    /// Sentinel returned after the last token in a stream.
    Eof,

    // ---- Node kinds (produced by the parser, not the lexer) --------------
    Module,
    ModuleHeader,
    ImportClause,
    ExtendsAmendsClause,
    AnnotationList,
    Annotation,
    ModifierList,
    Modifier,
    /// A dotted name in type or annotation position (e.g. `acme.config.Foo`).
    QualifiedName,

    ClassDecl,
    ClassBody,
    ClassPropertyDecl,
    ClassMethodDecl,
    TypeAliasDecl,
    PropertyDecl,
    MethodDecl,
    ParameterList,
    Parameter,
    TypeParameterList,
    TypeParameter,
    TypeArgumentList,

    ObjectBody,
    ObjectEntry,
    ObjectElement,
    ObjectProperty,
    ObjectMethod,
    ObjectEntryComputed,
    ObjectSpread,
    WhenGenerator,
    ForGenerator,

    // Types
    TypeRef,
    TypeNullable,
    TypeUnion,
    TypeFunction,
    TypeParenthesized,
    TypeStringLiteral,
    TypeConstrained,
    TypeDefault,
    TypeModule,
    TypeUnknown,
    TypeNothing,

    // Expressions
    LiteralExpr,
    IdentExpr,
    QualifiedExpr,
    ParenExpr,
    UnaryExpr,
    BinaryExpr,
    TypeCheckExpr, // expr is Type
    TypeCastExpr,  // expr as Type
    IfExpr,
    LetExpr,
    NewExpr,
    AmendsExpr,
    CallExpr,
    IndexExpr,
    MemberExpr,
    PropertyAccessExpr,
    LambdaExpr,
    NonNullExpr,      // expr!!
    NullCoalesceExpr, // expr ?? other
    ReadExpr,
    ImportExpr,
    ThrowExpr,
    TraceExpr,
    SuperAccessExpr,
    SuperSubscriptExpr,
    /// Node wrapping a single-line interpolated string. Children are a
    /// `StringQuoteOpen`, alternating `StringPart`/`Interpolation` children,
    /// then a `StringQuoteClose`. Used for both bare `"..."` and
    /// custom-delimited `#"..."#` flavours; the opening token's text
    /// identifies which.
    InterpolatedString,
    /// Node wrapping a multi-line interpolated string (triple-quoted).
    /// Layout mirrors [`InterpolatedString`] but with `MultilineStringPart`
    /// runs in place of `StringPart`.
    InterpolatedMultilineString,
    /// Node wrapping a single `\(expr)` interpolation hole inside an
    /// interpolated string. Children are `InterpolationStart`, an `Expr`,
    /// and `InterpolationEnd` (the closing marker may be omitted on
    /// recovery).
    Interpolation,

    ArgList,

    /// Catch-all node emitted by the parser when it could not make sense of
    /// some span. Carries an error message.
    ErrorNode,

    // Sentinel: must remain the last variant. Used by the rowan
    // `Language` impl to validate raw kinds. Adding a new variant?
    // Insert it above this line.
    #[doc(hidden)]
    __Last,
}

impl SyntaxKind {
    /// Safely convert a raw `u16` (as carried by `rowan::SyntaxKind`)
    /// into a `SyntaxKind`. Returns `None` when the raw value falls
    /// outside the enum range — useful for guarded conversions.
    pub fn from_raw(raw: u16) -> Option<Self> {
        if raw >= SyntaxKind::__Last as u16 {
            None
        } else {
            // SAFETY: `SyntaxKind` is `#[repr(u16)]` and `raw` is bounded
            // above by the sentinel `__Last`, so every value in range is
            // a valid discriminant.
            Some(unsafe { std::mem::transmute::<u16, SyntaxKind>(raw) })
        }
    }
}

impl SyntaxKind {
    /// True for whitespace, newlines, and comments — tokens the parser skips
    /// but which are preserved on each node for tooling (formatter, hover).
    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            SyntaxKind::Whitespace
                | SyntaxKind::Newline
                | SyntaxKind::LineComment
                | SyntaxKind::BlockComment
                | SyntaxKind::DocComment
        )
    }

    #[inline]
    pub fn is_keyword(self) -> bool {
        matches!(
            self,
            SyntaxKind::AbstractKw
                | SyntaxKind::AmendsKw
                | SyntaxKind::AsKw
                | SyntaxKind::ClassKw
                | SyntaxKind::ElseKw
                | SyntaxKind::ExtendsKw
                | SyntaxKind::ExternalKw
                | SyntaxKind::FalseKw
                | SyntaxKind::FixedKw
                | SyntaxKind::ForKw
                | SyntaxKind::FunctionKw
                | SyntaxKind::HiddenKw
                | SyntaxKind::IfKw
                | SyntaxKind::ImportKw
                | SyntaxKind::ImportGlobKw
                | SyntaxKind::InKw
                | SyntaxKind::IsKw
                | SyntaxKind::LetKw
                | SyntaxKind::LocalKw
                | SyntaxKind::ModuleKw
                | SyntaxKind::NewKw
                | SyntaxKind::NothingKw
                | SyntaxKind::NullKw
                | SyntaxKind::OpenKw
                | SyntaxKind::OutKw
                | SyntaxKind::OuterKw
                | SyntaxKind::ReadKw
                | SyntaxKind::ReadOrNullKw
                | SyntaxKind::ReadGlobKw
                | SyntaxKind::SuperKw
                | SyntaxKind::ThisKw
                | SyntaxKind::ThrowKw
                | SyntaxKind::TraceKw
                | SyntaxKind::TrueKw
                | SyntaxKind::TypeAliasKw
                | SyntaxKind::UnknownKw
                | SyntaxKind::WhenKw
        )
    }

    /// True if a keyword can also appear as a "soft" modifier in front of a
    /// declaration. Pkl allows e.g. `abstract`, `open`, `local`, `hidden`,
    /// `fixed`, `external` in modifier position.
    #[inline]
    pub fn is_modifier_kw(self) -> bool {
        matches!(
            self,
            SyntaxKind::AbstractKw
                | SyntaxKind::OpenKw
                | SyntaxKind::LocalKw
                | SyntaxKind::HiddenKw
                | SyntaxKind::FixedKw
                | SyntaxKind::ExternalKw
        )
    }
}

impl fmt::Display for SyntaxKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            // The handful of tokens whose human-readable spelling differs
            // from the variant name.
            SyntaxKind::AbstractKw => "abstract",
            SyntaxKind::AmendsKw => "amends",
            SyntaxKind::AsKw => "as",
            SyntaxKind::ClassKw => "class",
            SyntaxKind::ElseKw => "else",
            SyntaxKind::ExtendsKw => "extends",
            SyntaxKind::ExternalKw => "external",
            SyntaxKind::FalseKw => "false",
            SyntaxKind::FixedKw => "fixed",
            SyntaxKind::ForKw => "for",
            SyntaxKind::FunctionKw => "function",
            SyntaxKind::HiddenKw => "hidden",
            SyntaxKind::IfKw => "if",
            SyntaxKind::ImportKw => "import",
            SyntaxKind::ImportGlobKw => "import*",
            SyntaxKind::InKw => "in",
            SyntaxKind::IsKw => "is",
            SyntaxKind::LetKw => "let",
            SyntaxKind::LocalKw => "local",
            SyntaxKind::ModuleKw => "module",
            SyntaxKind::NewKw => "new",
            SyntaxKind::NothingKw => "nothing",
            SyntaxKind::NullKw => "null",
            SyntaxKind::OpenKw => "open",
            SyntaxKind::OutKw => "out",
            SyntaxKind::OuterKw => "outer",
            SyntaxKind::ReadKw => "read",
            SyntaxKind::ReadOrNullKw => "read?",
            SyntaxKind::ReadGlobKw => "read*",
            SyntaxKind::SuperKw => "super",
            SyntaxKind::ThisKw => "this",
            SyntaxKind::ThrowKw => "throw",
            SyntaxKind::TraceKw => "trace",
            SyntaxKind::TrueKw => "true",
            SyntaxKind::TypeAliasKw => "typealias",
            SyntaxKind::UnknownKw => "unknown",
            SyntaxKind::WhenKw => "when",
            other => return write!(f, "{:?}", other),
        };
        f.write_str(s)
    }
}

/// Map an identifier-looking string to a keyword, if it matches one.
///
/// Note: `import*`, `read?`, and `read*` are produced by the lexer's
/// punctuation handler rather than this table because they contain non-ident
/// characters.
pub(crate) fn keyword_from_ident(s: &str) -> Option<SyntaxKind> {
    Some(match s {
        "abstract" => SyntaxKind::AbstractKw,
        "amends" => SyntaxKind::AmendsKw,
        "as" => SyntaxKind::AsKw,
        "class" => SyntaxKind::ClassKw,
        "else" => SyntaxKind::ElseKw,
        "extends" => SyntaxKind::ExtendsKw,
        "external" => SyntaxKind::ExternalKw,
        "false" => SyntaxKind::FalseKw,
        "fixed" => SyntaxKind::FixedKw,
        "for" => SyntaxKind::ForKw,
        "function" => SyntaxKind::FunctionKw,
        "hidden" => SyntaxKind::HiddenKw,
        "if" => SyntaxKind::IfKw,
        "import" => SyntaxKind::ImportKw,
        "in" => SyntaxKind::InKw,
        "is" => SyntaxKind::IsKw,
        "let" => SyntaxKind::LetKw,
        "local" => SyntaxKind::LocalKw,
        "module" => SyntaxKind::ModuleKw,
        "new" => SyntaxKind::NewKw,
        "nothing" => SyntaxKind::NothingKw,
        "null" => SyntaxKind::NullKw,
        "open" => SyntaxKind::OpenKw,
        "out" => SyntaxKind::OutKw,
        "outer" => SyntaxKind::OuterKw,
        "read" => SyntaxKind::ReadKw,
        "super" => SyntaxKind::SuperKw,
        "this" => SyntaxKind::ThisKw,
        "throw" => SyntaxKind::ThrowKw,
        "trace" => SyntaxKind::TraceKw,
        "true" => SyntaxKind::TrueKw,
        "typealias" => SyntaxKind::TypeAliasKw,
        "unknown" => SyntaxKind::UnknownKw,
        "when" => SyntaxKind::WhenKw,
        _ => return None,
    })
}

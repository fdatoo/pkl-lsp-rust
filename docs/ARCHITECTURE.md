# Architecture

The Pkl language server is a single binary (`pkl-lsp`) backed by a four-crate
Cargo workspace. Each crate owns one slice of the analysis pipeline and
depends only on the crates to its left in this diagram:

```text
┌──────────────┐      ┌──────────────┐
│ pkl-syntax   │◄─────│ pkl-stdlib   │
│ (lexer +     │      │ (vendored    │
│  parsers +   │      │  pkl.base    │
│  AST + CST + │      │  + bundled   │
│  formatter)  │      │  modules)    │
└──────┬───────┘      └──────┬───────┘
       │                     │
       ▼                     ▼
       ┌──────────────────────┐
       │     pkl-analyze      │
       │  resolver, inferrer, │
       │  subtyping, loader,  │
       │     module graph     │
       └──────────┬───────────┘
                  │
                  ▼
            ┌──────────┐
            │ pkl-lsp  │
            │ tower-lsp│
            │ backend  │
            └──────────┘
```

## Layered responsibilities

### `pkl-syntax`

Pure lexer + parser + AST + formatter. No analysis state, no I/O.

The lossless rowan tree is the single source of truth; the typed AST
is materialized off it on every parse.

* `lexer.rs` is a hand-rolled byte-streaming lexer with `unicode-ident`
  for XID classification. It produces a flat `Vec<Token>` and preserves
  trivia (whitespace, comments) so the layer above can decide what to
  ignore. Strings carrying `\(expr)` interpolation drive a small
  mode-stack (`Normal` / `String { hashes, multiline }` /
  `Interpolation { paren_depth }`) so the lexer can re-enter normal Pkl
  tokenisation inside each hole; strings without interpolation stay on
  the single-token path. Each interpolated literal is emitted as
  `StringQuoteOpen` / alternating `StringPart` (or
  `MultilineStringPart`) + `InterpolationStart` ... `InterpolationEnd`
  pairs / `StringQuoteClose`.
* `green.rs` is the parser. It drives a `rowan::GreenNodeBuilder` over
  the lexer's token stream and produces a red-green syntax tree in
  which every byte of the original source (trivia included) is
  represented. `parse_green(src).syntax.text() == src` for any input.
* `syntax.rs` ties the `SyntaxKind` enum to `rowan::Language` and
  exports `SyntaxNode` / `SyntaxToken` / `SyntaxElement` aliases.
* `cst.rs` is the typed view over the lossless tree: zero-cost newtype
  wrappers around `SyntaxNode` (e.g. `Module`, `ClassDecl`, `Expr`)
  with accessor methods that walk the tree on demand. Includes
  `doc_comment_for(node)` which recovers `///` comments out of the
  declaration's leading trivia (descending through wrapper nodes like
  `ModifierList` so doc comments still attach when modifiers are
  present). The `InterpolatedString` wrapper exposes `parts()` and
  `interpolations()` iterators so analyzer/formatter passes can walk
  the literal-text runs and the `\(...)` holes uniformly across
  single-line, multi-line, and custom-delimited (`#"..."#`) flavours.
* `parser.rs` exposes the public `parse(src) -> ParseResult` entry
  point. `ParseResult` carries the immutable `GreenNode` (so the
  result can live in `Send + Sync` shared state) plus any diagnostics.
  Call `parsed.syntax()` to obtain a fresh thread-local red-tree root
  for walking; `cst::Module::cast(parsed.syntax())` then yields the
  typed module view. There is no separate owned AST — every consumer
  walks the lossless tree directly.
* `format.rs` re-emits canonical Pkl text by walking the typed CST
  view. Ordinary `//` and `/* ... */` comments — not just `///` doc
  comments — are preserved verbatim at every declaration boundary
  (module items, class members, object members).

### `pkl-stdlib`

A static catalogue of Pkl's standard library. Two flavours:

* `base.rs` / `top_level.rs` — hand-curated `StdlibType` / `StdlibMember`
  / `StdlibFunction` data used by the analyzer for hover, completion, and
  member resolution. Signatures are stored as `&'static str` and parsed
  lazily by `pkl-analyze::types::parse_signature` when needed for generic
  substitution.
* `vendored.rs` — the official `apple/pkl` stdlib `.pkl` sources baked
  into the binary via `include_str!`. The `pkl:` URI scheme resolves
  here so `import "pkl:json"` works without a network round-trip.

### `pkl-analyze`

Where most of the work happens. Composed of independent passes that talk
through small typed handoffs rather than a shared `Db`:

* `loader.rs` — `ModuleLoader` trait + `FsLoader` (filesystem + namespace
  resolution) + `StdlibLoader` (`pkl:` scheme) + `ChainedLoader`.
* `module_graph.rs` — owns one `ModuleEntry` per known module
  (open or transitively imported), with import targets and a dependents
  index so editing one file can refresh consumers.
* `resolver.rs` — single-file pass that walks the CST and builds:
  - `SymbolTable` of every declaration (`Class`, `Property`, `Method`,
    `Parameter`, `LetBinding`, `ForBinding`, `ObjectParameter`,
    `Import`, `TypeParameter`, `TypeAlias`).
  - `Vec<Reference>` of every identifier site that resolves to a symbol.
  - `Resolution.imports` map for the graph to consume.
  - Seeds the stdlib catalogue into the module scope so `String` etc.
    resolve out of the box.
* `infer.rs` — second pass that assigns a `Ty` to every expression,
  propagates types through arithmetic / `if` / `let` / lambdas / calls,
  resolves member access against stdlib + user-defined classes (walking
  inheritance), substitutes generics at call sites, and threads an
  "expected type" through object bodies so `new T { x = … }` knows what
  to look up against.
* `subtyping.rs` — conservative `is_subtype` predicate driving
  type-mismatch diagnostics. Permissive by design: unknown means
  "no diagnostic".
* `hover.rs` / `pretty.rs` / `types.rs` — formatting helpers used by
  the LSP and the inferrer.

### `pkl-lsp`

The thin tower-lsp glue:

* `backend.rs` holds the server state: `DashMap<Url, Document>` for
  ropes + per-file analysis, plus a `tokio::sync::RwLock<ModuleGraph>`
  for cross-module data.
* `config.rs` parses `initializationOptions` and `$PKL_LSP_NAMESPACES`.
* Per-feature modules (`hover.rs`, `goto.rs`, `completion.rs`,
  `references.rs`, `rename.rs`, `highlights.rs`, `folding.rs`,
  `selection_range.rs`, `semantic_tokens.rs`, `inlay_hints.rs`,
  `signature_help.rs`, `code_actions.rs`, `workspace_symbols.rs`,
  `formatting.rs`, `symbols.rs`) are stateless adapters from analyzer
  data to LSP types.

## Request lifecycle

```text
client     ┌──── didOpen / didChange ────►  Backend
                                            │
                                            ▼
                                       Document (rope + parse + analyze)
                                            │
                                            └──► ModuleGraph.upsert
                                                  ├─ parse + analyze
                                                  └─ loader.resolve(import) ...

client     ◄──── publishDiagnostics ────  Backend

client     ┌──── hover (pos) ─────────────►  Backend
                                            │
                                            ▼
                                       Document.position_to_offset
                                            │
                                            ▼
                                       Inference.member_ref_touching
                                       (or Resolution.symbol_at_offset)
                                            │
                                            ▼
                                       hover_markdown / member_hover_markdown
                                            │
client     ◄──── Hover ─────────────────  Backend
```

The graph is consulted for cross-module lookups (hover on an imported
member, goto-def into another file, workspace symbols). For single-file
queries the LSP shortcuts through the document's cached analysis.

## Key data structures

* `Span { start: u32, end: u32 }` — byte offsets. Every AST node and
  symbol carries one. Conversion to LSP `Position` (UTF-16) happens
  only in the LSP layer using the document's `ropey::Rope`.
* `Ty` — internal type enum: primitives, `Nullable`, `Union`,
  `List<T>`/`Map<K,V>`/`Pair<A,B>`/etc., `Function`, and a catch-all
  `Named { name, args }` used both for user classes and for type
  variables (`T`, `R`, `K`, …) during substitution.
* `Symbol` — every declaration the analyzer knows about. Carries
  `origin: Origin::User | Origin::Stdlib { module }`, a `declared_ty`,
  and an optional `container: Option<SymbolId>` (e.g. a property's
  enclosing class).
* `MemberRef` — recorded for every `expr.name` access, keyed by the
  name span. Holds either a `stdlib_member` + `stdlib_type` or a
  `user_class` + `user_member`. Both empty means "we saw an access
  but couldn't resolve it" — useful for hover/completion to still
  show a sensible "on `Type`" line.
* `ModuleEntry` — graph entry for one module: source, `Analysis`,
  `import_targets`, `import_errors`, `is_open`.

## Lossless tree architecture

The lossless `rowan`-backed syntax tree is the single source of truth
for parsing **and** consumption. There is no separate owned AST — the
analyzer, LSP feature handlers, the formatter, and the stdlib scraper
all walk the lossless tree through the typed wrappers in `cst`.

Span identities are preserved across the whole stack by
`cst::significant_span(node)`, which excludes leading and trailing
trivia from a node's range. Callers that key into the analyzer's
offset maps (`Inference::expr_types`, `Resolution::by_span_start`)
use that helper so the hand-rolled parser's old "spans bracket
significant tokens" invariant still holds.

Key pieces:

* Lossless parser (`green::parse_green`), red-green tree plumbing
  (`syntax`), typed wrappers (`cst`), and the round-trip +
  smoke-fuzz test suite.
* Comment-preserving formatter (`format::format_module`) walking the
  CST view.
* Doc-comment recovery via `cst::doc_comment_for(node)`.
* `pkl-analyze::resolve_module` and `infer_module` accept a
  `cst::Module`. The public `analyze(syntax, diagnostics)` entry
  point takes the rowan `SyntaxNode` directly and constructs the
  typed module on the way in.

## Non-goals (for now)

* A full Hindley-Milner type checker. The inferrer is single-pass and
  greedy. Generic substitution is best-effort; subtyping is
  conservative so we never lie to the user with a false positive.
* Cross-file references / rename. The graph tracks dependents but the
  LSP layer only emits intra-file edits today.

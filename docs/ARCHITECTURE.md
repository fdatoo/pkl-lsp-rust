# Architecture

The Pkl language server is a single binary (`pkl-lsp`) backed by a four-crate
Cargo workspace. Each crate owns one slice of the analysis pipeline and
depends only on the crates to its left in this diagram:

```text
┌──────────────┐      ┌──────────────┐
│ pkl-syntax   │◄─────│ pkl-stdlib   │
│ (lexer +     │      │ (vendored    │
│  parser +    │      │  pkl.base    │
│  AST +       │      │  + bundled   │
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

* `lexer.rs` is a hand-rolled byte-streaming lexer with `unicode-ident` for
  XID classification. It produces a flat `Vec<Token>` and preserves trivia
  (whitespace, comments) so the layer above can decide what to ignore.
* `parser.rs` is a recursive-descent parser that produces a typed `ast`
  tree with byte-offset `Span`s on every node and recoverable error nodes
  on bad input.
* `format.rs` re-emits the AST as canonical Pkl text for the LSP's
  formatting handler.

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
* `resolver.rs` — single-file pass that walks the AST and builds:
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

## Non-goals (for now)

* A lossless syntax tree. The typed AST drops ordinary `// ...` and
  `/* ... */` comments. The formatter and some refactorings would
  benefit from a `rowan`-style tree but that's a substantial rewrite
  and isn't on the critical path for editor UX.
* A full Hindley-Milner type checker. The inferrer is single-pass and
  greedy. Generic substitution is best-effort; subtyping is
  conservative so we never lie to the user with a false positive.
* Cross-file references / rename. The graph tracks dependents but the
  LSP layer only emits intra-file edits today.

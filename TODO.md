# TODO

Outstanding work and known gaps in the Pkl language server, organised by
component. Items are written as checkboxes so they can be ticked off; each
gap notes the smallest reproducer or implication where it isn't obvious.

Legend:

* `[x]` shipped
* `[-]` deferred, with a one-line reason
* `[ ]` open, ready to pick up

## Lexer / Parser (`pkl-syntax`)

- [-] **String interpolation tokens.** `"\(expr)"` is still emitted as one
  `String` token; the parser doesn't see the embedded expressions, so
  hover/goto inside an interpolation can't work. Deferred: requires a
  stateful re-entrant lexer and a new AST shape.
- [-] **Custom-delimited string interpolation.** Same as above for
  `#"...#\(expr)..."#`.
- [x] **Doc-comment recovery for leading trivia.** `cst::doc_comment_for`
  walks the leading-trivia children of a `SyntaxNode` so consumers can
  recover the doc comment for any declaration on demand.
- [-] **`new { ... }` element syntax.** Object bodies accept bare
  expressions as `ObjectMember::Element` today; no specific edge case
  remains open. Tracked as deferred polish if real-world examples
  surface.
- [x] **Type-level tuples.** Parser emits a friendly diagnostic
  ("use `Pair<A, B>` or a function type `(A, B) -> R`") instead of
  failing silently.
- [x] **Lossless syntax tree.** `pkl_syntax::parse` drives a
  `rowan`-based lossless parser and returns a `ParseResult` carrying
  the immutable green tree plus syntax diagnostics. `pkl_syntax::cst`
  provides typed newtype wrappers (`Module`, `ClassDecl`, `Expr`, …)
  with accessor methods that walk the tree on demand. Round-trip and
  smoke-fuzz tests assert `parse_green(src).syntax.text() == src` for
  both curated and mutated/random inputs. The formatter, analyzer,
  LSP feature handlers, and stdlib scraper all consume the lossless
  tree — there is no longer a separate owned AST.

## Resolver / Symbol table (`pkl-analyze::resolver`)

- [x] **Qualified type names.** `acme.Foo` records a cross-module
  member ref on the tail segment so hover and goto route through the
  module graph.
- [x] **`super` and inheritance scopes.** Class symbols carry the
  parent-class name; member lookup walks the `extends` chain; `super`
  inside a method body types as the parent class.
- [x] **Object body members.** Members declared inside `new T { ... }`
  / `prop: T = { ... }` resolve against `T`, including hover and
  goto-def.
- [x] **`when` / `for` body shadowing rules.** `for (x in xs)`
  narrows `x` from the iterable's element type (List/Set/Listing,
  Map/Mapping); `when (cond)` reuses the same `is`-based narrowing as
  `if`.
- [x] **Annotation argument resolution.** Annotation type heads route
  through the qualified-name path.

## Type inference (`pkl-analyze::infer`, `pkl-analyze::types`)

- [x] **Generic substitution.** `List(1,2,3)` infers `List<Int>`,
  `.map((x) -> "hi")` propagates `R` through to `List<String>`, etc.
- [x] **User-class member access.** `Person.name` resolves through the
  resolver's symbol table.
- [x] **Flow-sensitive narrowing.** `if (x is String) x.length`
  narrows `x` to `String` inside the then-branch.
- [x] **Union / intersection joins.** Branch joins compute an LUB over
  the primitive lattice (`Int|Float -> Number`) and the class graph
  (`Dog|Cat -> Animal` when both extend `Animal`); falls back to
  `Union` only when no common ancestor exists.
- [x] **Subtype checking.** Primitive lattice + nullable + collections
  + unions + named reflexivity + user-class extends-chain widening.
- [x] **Type-error diagnostics.** Property *and* method-body return-
  type mismatches surface on the LSP.
- [x] **Lambda parameter inference.** Untyped lambda parameters in
  member-call arguments inherit the callee's expected parameter type
  with receiver-side substitution applied.

## Standard library (`pkl-stdlib`)

- [x] **Module coverage.** All of `pkl.base`, `pkl.math`, `pkl.semver`,
  `pkl.platform`, `pkl.shell`, `pkl.reflect`, `pkl.test`, `pkl.json`,
  `pkl.yaml`, `pkl.xml`, `pkl.protobuf`, `pkl.jsonnet`,
  `pkl.pklbinary`, `pkl.analyze`, `pkl.settings`, `pkl.release`, plus
  the registry/project schemas are vendored verbatim from upstream and
  resolvable via the `pkl:` URI scheme.
- [x] **Build-time scrape vs hand curation.** `pkl-stdlib::scrape`
  parses the vendored `base.pkl` once at first access and produces
  `&'static StdlibType` entries with upstream signatures + doc
  comments. The scraped catalogue is authoritative; the hand-curated
  list is a fallback.
- [x] **Hand-curated catalogue completeness.** Obsoleted by the
  scrape — every member upstream declares is now reachable.
- [-] **Type parameters on stdlib symbols.** Deferred: no user-facing
  hover path drives a real need yet.

## Module graph & loader (`pkl-analyze::module_graph`, `loader`)

- [x] **`pkl:` scheme.** Resolves to the embedded vendored sources via
  `StdlibLoader`.
- [x] **`package:` scheme.** `FsLoader` resolves against the
  configured on-disk cache directory (init option `packageCache` or
  `$PKL_LSP_PACKAGE_CACHE`). Online package fetching is out of scope.
- [x] **`https:` / `http:` scheme.** Optional `RemoteLoader` behind
  the `remote` Cargo feature on `pkl-analyze`, using a blocking
  `ureq` GET. Off by default; opt-in for editor builds.
- [x] **`modulepath:` scheme.** Configurable search roots (init
  option `modulePaths` or `$PKL_LSP_MODULE_PATHS`).
- [-] **Transitive invalidation.** Not needed today because cross-
  module data is fetched lazily at LSP request time.
- [x] **Watch the filesystem.** `workspace/didChangeWatchedFiles`
  refreshes graph entries on CREATED/CHANGED, removes on DELETED.
- [x] **Canonicalisation requires existing files.** `FsLoader` falls
  back to a lexically-normalised path (via `normalize_path`) when
  `Path::canonicalize` fails on a missing file.
- [x] **Glob imports (`import* "..."`).** `FsLoader::resolve_glob`
  shells out to the `glob` crate; the graph pulls every matched file
  in. Pkl's `Mapping<String, Module>` surface for the alias remains
  a follow-up — only the first match becomes the `import_targets`
  entry today.
- [x] **Module-graph diagnostics for unresolved members.** LSP emits
  a warning on `imported.foo` when the imported module exists but
  doesn't expose `foo`.
- [x] **Bidirectional URI canonicalisation.** `uri_to_path` /
  `path_to_uri` round-trip both Unix and Windows shapes; CI now runs
  the full test matrix on `windows-latest`.

## LSP features (`pkl-lsp`)

- [x] **Completion provider.** Top-level identifiers, member completion
  off the receiver type, import-path completion of `pkl:` modules.
- [x] **References (`textDocument/references`).**
- [x] **Workspace-wide find-references.** `textDocument/references` now
  follows imports: for a top-level user symbol, every dependent module's
  `MemberRef` set is walked through the graph and matching `alias.<name>`
  sites become `Location`s. Stdlib symbols and import aliases stay
  local-only.
- [x] **Rename (`textDocument/rename` + `prepareRename`).**
- [x] **Workspace-wide rename.** `textDocument/rename` builds a multi-
  file `WorkspaceEdit` via the new `ModuleGraph::references_to` helper,
  emitting edits in every dependent module that accesses the symbol
  through an import alias. Import aliases stay local, stdlib symbols
  remain refused. Open dependents go through their `Document` rope;
  unopened dependents fall back to the graph's cached source string for
  UTF-16-correct range computation.
- [x] **Workspace symbols.** Aggregated from every module in the graph.
- [x] **Signature help.** Active parameter from comma-counting.
- [x] **Code actions.** "Annotate `name: Type`" quick-fix for
  unannotated properties whose type the inferrer worked out. More
  actions can land in the same module.
- [x] **Document formatting.** AST-driven; comment-preserving variant
  over the lossless tree is open work (see
  "Lossless tree consumer migration" below).
- [x] **Semantic tokens.** 14-type / 3-modifier legend driven by the
  token stream + resolver.
- [x] **Inlay hints.** Type hints after unannotated property
  declarations.
- [x] **Folding ranges.** Class bodies, object bodies, when/for/else
  bodies, multi-line strings, block comments.
- [x] **Selection ranges.** AST-walk-driven widening chain.
- [x] **Document highlights.** Definition-as-WRITE, references-as-READ.
- [x] **`window/workDoneProgress`.** `didOpen` wraps the graph load in
  a progress region when the client advertises the capability.

## Build / tooling

- [x] **`unicode-ident`** powers identifier classification.
- [x] **Benchmarks.** `pkl-syntax` and `pkl-analyze` ship criterion
  benches over a small module, a 16x/32x clone, and `pkl.base`.
- [x] **Fuzz target.** `/fuzz` defines `cargo fuzz` targets for
  `parse` and `analyze`; `pkl-syntax` also runs a deterministic smoke
  fuzz as part of the regular test suite.
- [x] **CI.** `.github/workflows/ci.yml` runs `cargo fmt --check`,
  `clippy -D warnings`, build + test on Linux + macOS + Windows, and
  an MSRV `cargo check` on 1.80.
- [x] **Release artifacts.** `.github/workflows/release.yml`
  cross-compiles `pkl-lsp` for x86_64 / aarch64 Linux + macOS and
  x86_64 Windows on every `v*` tag and publishes a GitHub release.
- [-] **`pkl:` stdlib bundling.** Resolved — vendored `.pkl` files
  shipped via `include_str!` (see `crates/pkl-stdlib/vendor/`).

## Documentation

- [x] **Architecture overview.** `docs/ARCHITECTURE.md`.
- [x] **Editor integration guides.** `docs/EDITORS.md` covers VS Code,
  Neovim, Helix, Zed, and Emacs.
- [x] **README namespace example** wired to a `switchyard` mapping.
- [x] **Contribution guide.** `CONTRIBUTING.md`.

## Lossless tree consumer migration

The lossless syntax tree is now the single source of truth across the
whole stack. The owned AST has been retired entirely; every consumer
walks `cst::*` directly.

- [x] **Comment-preserving formatter.** `pkl-syntax::format` walks
  `cst::Module` over the lossless tree and emits ordinary `//` and
  `/* ... */` comments verbatim at every declaration boundary.
- [x] **Retire the hand-rolled parser.** The 1958-line
  recursive-descent parser is gone; `pkl-syntax::parser::parse` is a
  thin wrapper around `green::parse_green`.
- [x] **Analyzer port.** `pkl-analyze::resolver` and
  `pkl-analyze::infer` walk `cst::Module`. `analyze(syntax,
  diagnostics)` takes a rowan `SyntaxNode`. Span identities are
  preserved by `cst::significant_span`, which excludes trivia from a
  node's range so the analyzer's offset-keyed lookups still match.
- [x] **LSP feature handlers.** `code_actions`, `folding`,
  `inlay_hints`, `selection_range`, `signature_help`, and `symbols`
  all walk `cst::*`. `Document::module()` exposes a typed view onto
  the cached parse result.
- [x] **Delete the owned AST.** `pkl-syntax::ast` and
  `pkl-syntax::ast_build` are gone. `cst::*` is the only typed entry
  point.

---

Every actionable item above the migration list has been worked
through. The remaining `[-]` entries are deferred by design (string
interpolation, stdlib type-parameter symbols, transitive graph
invalidation) with short rationales attached. Strike or add items as
they land or emerge.

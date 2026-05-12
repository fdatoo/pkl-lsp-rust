# Contributing

The Pkl language server is split across four crates plus an LSP binary,
all under one Cargo workspace. The README explains the layout; the
[architecture doc](docs/ARCHITECTURE.md) is the deeper read.

## Local development loop

```sh
# Build everything
cargo build

# Lint everything (CI runs the same)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Run every test (unit + integration + doc)
cargo test --workspace

# Benchmarks (release-mode; bring your own samples size)
cargo bench -p pkl-syntax
cargo bench -p pkl-analyze

# Fuzz the parser end-to-end (requires `cargo install cargo-fuzz`
# and a nightly toolchain).
cd fuzz && cargo +nightly fuzz run parse
```

CI runs rustfmt, clippy, build, and tests on Linux + macOS and an MSRV
`cargo check` on Rust 1.86.

## Coding conventions

* `RUSTFLAGS="-D warnings"` is enforced in CI. Treat warnings as errors
  locally too — they almost always indicate a real bug or dead code.
* Run `cargo fmt --all` before pushing. The committed files match what
  rustfmt produces with the default config.
* Prefer adding a focused integration test under
  `crates/*/tests/` over a unit test buried in `src/`. The integration
  tests double as documentation.
* New stdlib entries belong in
  `crates/pkl-stdlib/src/base.rs` (or a new module under `pkl_stdlib`)
  with signatures in the **same shape** as existing entries:
  - property: `"name: Type"`
  - method: `"name(arg: T): Result"` (use `<R>` for method-local
    generics, `T...` for variadics).
  The structural signature parser in `pkl-analyze::types` relies on
  that shape.
* Doc comments on stdlib members and on Symbol decls show up verbatim
  in editor hover. Write them like you would a Pkl-side docstring:
  one-line summary, optional follow-up paragraphs.
* Don't introduce `unwrap()` / `expect()` on user input paths. The LSP
  must never panic on malformed Pkl — that's what the smoke-fuzz test
  guards.

## Test layout

* `crates/pkl-syntax/tests/lexer.rs`, `parser.rs`, `smoke_fuzz.rs` —
  the lexer/parser must accept these and never panic.
* `crates/pkl-analyze/tests/` — resolver + inferrer + stdlib + module
  graph integration tests. New analyzer features land here.
* `crates/pkl-lsp/tests/server.rs` — end-to-end LSP tests driving the
  tower-lsp server over an in-memory duplex. Add a new test per
  user-visible LSP capability you ship.

## Adding a new LSP feature

Most feature work follows the same shape:

1. If the feature needs new analyzer data, add it under `pkl-analyze`
   first (with its own integration test).
2. Add a small handler module under `crates/pkl-lsp/src/` that takes a
   `&Document` (and `&ModuleGraph` if it needs cross-file data) and
   returns the LSP type. Keep these stateless adapters — no IO.
3. Add a capability bit in `capabilities.rs`.
4. Add an `async fn` to the `LanguageServer` impl in `backend.rs` that
   forwards to your handler.
5. Add a `crates/pkl-lsp/tests/server.rs` case that drives the
   round-trip over the duplex and asserts on the JSON payload.

## Refreshing the vendored stdlib

`crates/pkl-stdlib/vendor/UPSTREAM.md` documents the exact commands. The
short version: `git clone --depth 1 --filter=blob:none --sparse
https://github.com/apple/pkl`, `git sparse-checkout set stdlib`, copy
`stdlib/*.pkl` and `LICENSE.txt`, update the commit hash in
`UPSTREAM.md`.

## TODO / roadmap

The deferred work and known gaps are tracked in [`TODO.md`](TODO.md).
Strike items as they ship and add new ones when you discover gaps.

## Commit messages

* Imperative present tense ("Add foo", not "Added foo"). 70-char
  subject line. Wrap the body at 72.
* Reference the analyzer / lexer / LSP component in the first word.
* If you touch the grammar or stdlib data shape, mention "breaking
  change" in the body so reviewers can audit downstream tests.

## Releases

There's no release process yet — `pkl-lsp` is consumed from source.
When that changes the `.github/workflows/` directory will gain a
release workflow that tags + publishes signed binaries.

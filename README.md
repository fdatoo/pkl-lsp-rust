# pkl-lsp-rust

A language server for [Pkl](https://pkl-lang.org), written in Rust.

This repository holds the in-progress implementation. Today the workspace
ships a working LSP that:

* tokenizes and parses Pkl source with a hand-rolled lexer + recursive-descent
  parser,
* runs a single-file semantic resolver: lexical scopes, symbol table,
  reference recording with span-keyed lookup,
* seeds every module with a structured model of Pkl's standard library
  (`String`, `Int`, `List`, `Map`, `Duration`, `DataSize`, ...) so built-in
  types resolve and hover,
* runs a single-pass type inferrer that assigns a `Ty` to every expression
  and resolves member access (`"hi".length`, `5.s`, `List(...).map`)
  against the stdlib catalogue, walking the inheritance chain so members
  inherited from `Collection<T>` work on `List<T>`,
* publishes syntax diagnostics on `textDocument/didOpen` / `didChange`,
* responds to `textDocument/documentSymbol` with a nested outline,
* answers `textDocument/hover` with a signature + doc-comment Markdown card
  (and credits `pkl.base` when the hovered symbol is a stdlib type or
  member),
* answers `textDocument/definition` for any name resolved in the current
  file (and jumps to the imported file for an import alias),
* resolves `import "..."` statements across files, including a
  user-configurable namespace mechanism (e.g. `import "switchyard:foo.pkl"`
  resolving to `$HOME/switchyard-pkl/foo.pkl`), with cross-file hover and
  goto-definition on imported members.

Type-error diagnostics, completion, references, and rename are next.

## Layout

```text
crates/
  pkl-syntax/   lexer, parser, AST, syntax diagnostics
  pkl-analyze/  resolver, symbol table, hover rendering
  pkl-stdlib/   structured catalogue of pkl.base types and members
  pkl-lsp/      LSP server binary built on top of the above
```

## Building

```sh
cargo build --release
```

The server binary is `target/release/pkl-lsp` and speaks LSP over stdio.

## Running tests

```sh
cargo test
```

## Documentation

* [Architecture](docs/ARCHITECTURE.md) — pipeline, crate layout, data
  structures.
* [Editor integration](docs/EDITORS.md) — VS Code, Neovim, Helix, Zed,
  Emacs.
* [Contributing](CONTRIBUTING.md) — coding conventions, test layout,
  release process.
* [TODO / roadmap](TODO.md) — outstanding work.

## Editor configuration

Any LSP-capable editor will work. Point it at the `pkl-lsp` binary for files
with the `pkl` language id. Set `PKL_LSP_LOG=debug` in the environment for
verbose tracing on stderr.

### Custom import namespaces

The server can resolve `import "name:rest"` against user-configured
filesystem roots, on top of the path forms Pkl supports out of the box.
Each prefix maps to a directory; `import "switchyard:config/main.pkl"` then
reads `$ROOT/config/main.pkl`.

Configure via either:

* `initializationOptions.namespaces` in the LSP handshake, e.g.

  ```json
  {
    "namespaces": {
      "switchyard": "$HOME/switchyard-pkl",
      "acme":       "/etc/acme-pkl"
    }
  }
  ```

  `$HOME` and `${VAR}` are expanded against the server's environment.

* `PKL_LSP_NAMESPACES` environment variable, comma-separated:

  ```sh
  export PKL_LSP_NAMESPACES="switchyard=$HOME/switchyard-pkl,acme=/etc/acme-pkl"
  ```

`initializationOptions` entries override env-var entries with the same name.
The official Pkl schemes `pkl:`, `package:`, `https:`, `http:`, and
`modulepath:` are recognised but not yet implemented — imports using them
surface as warnings in the editor.

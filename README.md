# pkl-lsp-rust

A language server for [Pkl](https://pkl-lang.org), written in Rust.

This repository ships a v1-class LSP for day-to-day Pkl editing. It focuses
on fast local feedback, durable parsing, and useful editor features without
embedding the full Pkl evaluator.

## Features

* Lossless lexer/parser for modern Pkl syntax, including interpolation,
  custom-delimited strings, constrained/default types, computed entries,
  `import*`, object-amend lambda parameters, object-body chains, and
  recovery for common mid-typing states.
* Syntax and analyzer diagnostics on `textDocument/didOpen` /
  `didChange`, including property and method return type mismatches.
* Standard-library model seeded from vendored upstream Pkl modules so
  built-in types, members, and `pkl:` imports resolve.
* Cross-file module graph for relative imports, `pkl:`, `package:`,
  `modulepath:`, glob imports, and user-configured namespaces.
* Hover, goto-definition, document symbols, workspace symbols, completion,
  signature help, semantic tokens, formatting, folding ranges, selection
  ranges, document highlights, inlay hints, references, rename, and small
  code actions.
* Workspace-aware import-path completion and cross-file hover/goto/
  references/rename through import aliases.

## Layout

```text
crates/
  pkl-syntax/   lexer, parser, CST, syntax diagnostics, formatter
  pkl-analyze/  resolver, symbol table, type inference, module graph
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

For the broader real-world smoke sweep, point the checked-in script at one
or more local repositories that contain `.pkl` files:

```sh
cargo build --release -p pkl-lsp
python3 scripts/realworld_smoke.py \
  --binary target/release/pkl-lsp \
  /path/to/apple-pkl \
  /path/to/pkl-swift-examples \
  /path/to/your-pkl-repo
```

## Documentation

* [Architecture](docs/ARCHITECTURE.md) — pipeline, crate layout, data
  structures.
* [Editor integration](docs/EDITORS.md) — VS Code, Neovim, Helix, Zed,
  Emacs.
* [Contributing](CONTRIBUTING.md) — coding conventions, test layout,
  release process.
* [Release process](docs/RELEASE.md) — release checks, tagging, and artifact
  publication.

## Editor configuration

Any LSP-capable editor will work. Point it at the `pkl-lsp` binary for files
with the `pkl` language id. Set `PKL_LSP_LOG=debug` in the environment for
verbose tracing on stderr.

### Custom import namespaces

The server can resolve `import "name:rest"` against user-configured
filesystem roots, on top of the path forms Pkl supports out of the box.
Each prefix maps to a directory; `import "switchyard:config/main.pkl"` then
reads `$ROOT/config/main.pkl`.

Extensionless namespace imports also resolve to `.pkl` files. If the mapped
root contains `PklProject.pkl`, the resolver also tries a same-named module
directory below the project root. For example, with
`switchyard=/repo/internal/config/pkl` and
`/repo/internal/config/pkl/PklProject.pkl` present,
`import "switchyard:automations"` resolves to
`/repo/internal/config/pkl/switchyard/automations.pkl`.

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

`package:` imports resolve against a configured on-disk package cache,
`modulepath:` imports resolve against configured search roots, and `pkl:`
imports resolve against the vendored standard library. Remote `http:` and
`https:` imports are available only when the analyzer is built with its
optional `remote` feature.

### Package cache and module paths

Configure package and modulepath resolution with initialization options:

```json
{
  "packageCache": "$HOME/.pkl/cache",
  "modulePaths": ["$HOME/work/config/pkl"]
}
```

or environment variables:

```sh
export PKL_LSP_PACKAGE_CACHE="$HOME/.pkl/cache"
export PKL_LSP_MODULE_PATHS="$HOME/work/config/pkl:/etc/pkl"
```

## Known limitations

This is a v1 language server, not a full reimplementation of the Pkl
runtime. The current boundaries are intentional so the editor remains fast
and predictable:

* The server does not evaluate Pkl modules. Diagnostics come from parsing,
  name resolution, import loading, and conservative type inference.
* Type inference is best-effort. Unknown or highly dynamic values are treated
  permissively to avoid false-positive errors.
* `package:` imports use an existing local cache; the server does not fetch
  packages from the network.
* Remote `http:` / `https:` imports are feature-gated and not enabled in the
  default binary.
* Formatting is pragmatic and CST-driven. It preserves common project layout
  and comments, but it is not guaranteed to match an official Pkl formatter
  byte-for-byte.
* Some evaluator-specific validation, such as checking constrained type
  predicates against concrete values, is out of scope for v1.

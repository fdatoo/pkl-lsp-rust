# Release Process

The public release artifact is the `pkl-lsp` binary. The workspace crates are
implementation crates for now; do not publish them to crates.io unless we are
ready to support their APIs independently of the editor server.

## Preflight

Run these checks locally from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo test --workspace --doc
cargo build --release -p pkl-lsp
```

Run the real-world smoke test against any local corpora you have available:

```sh
python3 scripts/realworld_smoke.py \
  --binary target/release/pkl-lsp \
  /path/to/apple-pkl \
  /path/to/pkl-swift-examples \
  /path/to/your-pkl-repo
```

The smoke script opens every `.pkl` file through the real LSP JSON-RPC path
and reports diagnostic counts. It should receive one diagnostic publication
per opened file and the server should shut down cleanly.

## Versioning

Update the workspace version in `Cargo.toml` before tagging. Use semantic
versioning for the binary:

* Patch: fixes that do not change LSP behavior or configuration.
* Minor: new LSP features, broader parser/analyzer support, new config keys.
* Major: breaking changes to command-line behavior, config shape, or editor
  expectations.

For v1, keep the release notes explicit about the known limitations listed in
`README.md`.

## Tagging

Create and push an annotated tag:

```sh
git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push origin main
git push origin vX.Y.Z
```

`.github/workflows/release.yml` builds Linux, macOS, and Windows artifacts
and publishes a GitHub release when a `v*` tag is pushed.

## After Publish

1. Confirm every expected artifact is attached to the GitHub release.
2. Download one artifact and run `pkl-lsp` with `--version` once that flag
   exists; until then, run it through an editor or the smoke script.
3. Update editor installation notes if artifact names or config keys changed.
4. Announce with the release notes and the known limitations.

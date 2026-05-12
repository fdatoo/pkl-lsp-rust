# Vendored Pkl standard library

These `.pkl` files are copied verbatim from the official Pkl distribution
at [github.com/apple/pkl](https://github.com/apple/pkl) so that the language
server can resolve `import "pkl:..."` references without a network round
trip.

Upstream source: <https://github.com/apple/pkl/tree/main/stdlib>
Snapshot commit: `bac8b47ba85745a3a99d75cd8ccf0e58eb026d21`
License: Apache-2.0 (see `LICENSE.txt` in this directory).

## Refreshing

```sh
cd /tmp && git clone --depth 1 --filter=blob:none --sparse https://github.com/apple/pkl.git pkl-upstream
cd pkl-upstream && git sparse-checkout set stdlib && git sparse-checkout add --skip-checks LICENSE.txt
cp stdlib/*.pkl LICENSE.txt <pkl-lsp-rust>/crates/pkl-stdlib/vendor/
```

Update the commit hash above when refreshing.

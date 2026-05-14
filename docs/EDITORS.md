# Editor integration

`pkl-lsp` is a stdio LSP server. Any editor that speaks LSP can drive it.
Point your client at the built binary, declare the `pkl` language id, and
optionally pass `initializationOptions.namespaces` to wire up custom
import prefixes (see `README.md`).

The recipes below assume you've run `cargo build --release` and have the
binary at `target/release/pkl-lsp`.

## VS Code

VS Code doesn't ship a generic LSP launcher; the simplest path is a small
client extension. The
[language-client](https://www.npmjs.com/package/vscode-languageclient)
package boils this down to a few lines:

```ts
// extension.ts
import { workspace, ExtensionContext } from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient;

export function activate(_context: ExtensionContext) {
  const serverOptions: ServerOptions = {
    command: "/path/to/pkl-lsp",
    args: [],
    transport: TransportKind.stdio,
  };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "pkl" }],
    initializationOptions: {
      namespaces: {
        switchyard: "${env:HOME}/switchyard-pkl",
      },
    },
  };
  client = new LanguageClient("pkl", "Pkl LSP", serverOptions, clientOptions);
  client.start();
}

export function deactivate() {
  return client?.stop();
}
```

Also register the language in `package.json`:

```json
"contributes": {
  "languages": [
    {
      "id": "pkl",
      "extensions": [".pkl"],
      "aliases": ["Pkl"]
    }
  ]
}
```

## Neovim (lspconfig)

```lua
require("lspconfig.configs").pkl = {
  default_config = {
    cmd = { "/path/to/pkl-lsp" },
    filetypes = { "pkl" },
    root_dir = require("lspconfig.util").root_pattern(".git", "PklProject"),
    init_options = {
      namespaces = {
        switchyard = vim.env.HOME .. "/switchyard-pkl",
      },
    },
  },
}
require("lspconfig").pkl.setup({})

vim.filetype.add({ extension = { pkl = "pkl" } })
```

## Helix

`languages.toml`:

```toml
[[language]]
name = "pkl"
scope = "source.pkl"
file-types = ["pkl"]
language-servers = ["pkl-lsp"]
indent = { tab-width = 2, unit = "  " }

[language-server.pkl-lsp]
command = "/path/to/pkl-lsp"

[language-server.pkl-lsp.config]
namespaces = { switchyard = "$HOME/switchyard-pkl" }
```

## Zed

`~/.config/zed/settings.json`:

```json
{
  "lsp": {
    "pkl-lsp": {
      "binary": {
        "path": "/path/to/pkl-lsp"
      },
      "initialization_options": {
        "namespaces": {
          "switchyard": "$HOME/switchyard-pkl"
        }
      }
    }
  },
  "language_models": {},
  "experimental.languages.pkl": {
    "language_servers": ["pkl-lsp"]
  }
}
```

## Emacs (eglot)

```elisp
(use-package eglot
  :config
  (add-to-list 'eglot-server-programs
               '(pkl-mode . ("/path/to/pkl-lsp"))))

(define-derived-mode pkl-mode prog-mode "Pkl"
  "Major mode for Pkl files.")
(add-to-list 'auto-mode-alist '("\\.pkl\\'" . pkl-mode))
```

Set `eglot-workspace-configuration` if your client passes
`initializationOptions` through that channel.

## Debugging the server

* `PKL_LSP_LOG=debug` enables verbose tracing on stderr.
* `PKL_LSP_NAMESPACES="name=/path,name2=/path"` is read as a fallback
  when the client doesn't deliver `initializationOptions.namespaces`.
* Namespace roots may point either at the module directory itself or at a
  Pkl project root containing `PklProject.pkl`. For project roots,
  `name:foo` also tries `$ROOT/name/foo.pkl`.
* Each LSP method runs synchronously against the per-document analysis;
  if a request hangs the most likely culprit is the document not yet
  having reached the server (no `didOpen`).

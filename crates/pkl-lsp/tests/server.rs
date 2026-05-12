//! End-to-end test that drives the LSP server over an in-memory pipe.
//!
//! This exercises the full JSON-RPC framing path: `initialize`, `did_open`,
//! a follow-up `documentSymbol` request, and `shutdown`. It guards against
//! regressions in the bits of plumbing the LSP needs to be useful.

use std::time::Duration;

use pkl_lsp::Backend;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tower_lsp::{LspService, Server};

#[tokio::test]
async fn initialize_did_open_document_symbol() {
    let (client_to_server, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, mut client_from_server) = tokio::io::duplex(64 * 1024);

    let (service, socket) = LspService::new(Backend::new);
    let server = Server::new(server_in, server_out, socket).serve(service);
    let server_handle = tokio::spawn(server);

    let mut writer = client_to_server;

    // ---- initialize ------------------------------------------------------
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": null,
                "capabilities": {}
            }
        }),
    )
    .await;

    let init_resp = read_message(&mut client_from_server).await;
    assert_eq!(init_resp["id"], 1);
    let caps = &init_resp["result"]["capabilities"];
    assert_eq!(caps["documentSymbolProvider"], json!(true));

    // ---- initialized notification ---------------------------------------
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        }),
    )
    .await;

    // ---- did_open --------------------------------------------------------
    let src = "class Foo { name: String }\n";
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": "file:///tmp/test.pkl",
                    "languageId": "pkl",
                    "version": 1,
                    "text": src
                }
            }
        }),
    )
    .await;

    // We expect a `publishDiagnostics` notification back.
    let diags = read_message(&mut client_from_server).await;
    assert_eq!(diags["method"], "textDocument/publishDiagnostics");
    assert_eq!(diags["params"]["uri"], "file:///tmp/test.pkl");
    assert_eq!(diags["params"]["diagnostics"].as_array().unwrap().len(), 0);

    // ---- documentSymbol --------------------------------------------------
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/documentSymbol",
            "params": {
                "textDocument": { "uri": "file:///tmp/test.pkl" }
            }
        }),
    )
    .await;

    let symbols = read_message(&mut client_from_server).await;
    assert_eq!(symbols["id"], 2);
    let result = symbols["result"].as_array().expect("nested symbols");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["name"], "Foo");
    // kind 5 = Class in LSP.
    assert_eq!(result[0]["kind"], 5);
    let children = result[0]["children"].as_array().expect("class children");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["name"], "name");

    // ---- shutdown --------------------------------------------------------
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}),
    )
    .await;
    let shut = read_message(&mut client_from_server).await;
    assert_eq!(shut["id"], 3);

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;

    // Server should terminate within a couple of seconds.
    let _ = tokio::time::timeout(Duration::from_secs(5), server_handle).await;
}

#[tokio::test]
async fn hover_and_goto_definition() {
    let (client_to_server, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, mut client_from_server) = tokio::io::duplex(64 * 1024);

    let (service, socket) = LspService::new(Backend::new);
    let server = Server::new(server_in, server_out, socket).serve(service);
    let server_handle = tokio::spawn(server);

    let mut writer = client_to_server;

    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"processId": null, "rootUri": null, "capabilities": {}}
        }),
    )
    .await;
    let init = read_message(&mut client_from_server).await;
    assert_eq!(init["result"]["capabilities"]["hoverProvider"], json!(true));
    assert_eq!(
        init["result"]["capabilities"]["definitionProvider"],
        json!(true)
    );

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )
    .await;

    // Layout:                       0123456789012345678901234
    //                               function greet(name: String): String = name
    //                                                                       ^ char 40 — body
    //                                              ^ char 15 — definition
    let src = "function greet(name: String): String = name\n";
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": "file:///tmp/hover.pkl",
                "languageId": "pkl",
                "version": 1,
                "text": src
            }}
        }),
    )
    .await;
    // diagnostics notification.
    let _ = read_message(&mut client_from_server).await;

    // ---- hover on the `name` reference at the end of the line -----------
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/hover",
            "params": {
                "textDocument": {"uri": "file:///tmp/hover.pkl"},
                "position": {"line": 0, "character": 40}
            }
        }),
    )
    .await;
    let hover = read_message(&mut client_from_server).await;
    assert_eq!(hover["id"], 2);
    let contents = &hover["result"]["contents"];
    assert_eq!(contents["kind"], "markdown");
    let value = contents["value"].as_str().unwrap();
    assert!(value.contains("name: String"), "got: {}", value);

    // ---- goto-def from the same position -------------------------------
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "textDocument/definition",
            "params": {
                "textDocument": {"uri": "file:///tmp/hover.pkl"},
                "position": {"line": 0, "character": 40}
            }
        }),
    )
    .await;
    let def = read_message(&mut client_from_server).await;
    assert_eq!(def["id"], 3);
    let location = &def["result"];
    assert_eq!(location["uri"], "file:///tmp/hover.pkl");
    // Definition of `name` starts at character 15.
    assert_eq!(location["range"]["start"]["character"], 15);

    // ---- shutdown -------------------------------------------------------
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), server_handle).await;
}

#[tokio::test]
async fn hover_on_stdlib_type() {
    let (client_to_server, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, mut client_from_server) = tokio::io::duplex(64 * 1024);

    let (service, socket) = LspService::new(Backend::new);
    let server = Server::new(server_in, server_out, socket).serve(service);
    let server_handle = tokio::spawn(server);

    let mut writer = client_to_server;

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"processId":null,"rootUri":null,"capabilities":{}}}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )
    .await;

    // Layout: `name: String = "hi"` — `String` starts at column 6.
    let src = "name: String = \"hi\"\n";
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": "file:///tmp/stdlib.pkl",
                "languageId": "pkl",
                "version": 1,
                "text": src
            }}
        }),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;

    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/hover",
            "params": {
                "textDocument": {"uri": "file:///tmp/stdlib.pkl"},
                "position": {"line": 0, "character": 6}
            }
        }),
    )
    .await;
    let hover = read_message(&mut client_from_server).await;
    let value = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(value.contains("class String"), "hover: {}", value);
    assert!(value.contains("from `pkl.base`"), "hover: {}", value);

    // Goto-def on the stdlib type should return null (no synthetic source).
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "textDocument/definition",
            "params": {
                "textDocument": {"uri": "file:///tmp/stdlib.pkl"},
                "position": {"line": 0, "character": 6}
            }
        }),
    )
    .await;
    let def = read_message(&mut client_from_server).await;
    assert!(
        def["result"].is_null(),
        "stdlib def should be null, got: {:?}",
        def
    );

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), server_handle).await;
}

#[tokio::test]
async fn hover_on_member_access() {
    let (client_to_server, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, mut client_from_server) = tokio::io::duplex(64 * 1024);

    let (service, socket) = LspService::new(Backend::new);
    let server = Server::new(server_in, server_out, socket).serve(service);
    let server_handle = tokio::spawn(server);

    let mut writer = client_to_server;

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"processId":null,"rootUri":null,"capabilities":{}}}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )
    .await;

    // Layout:        0123456789012345678901
    //                x = "hello".length
    // `length` starts at column 12.
    let src = "x = \"hello\".length\n";
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": "file:///tmp/member.pkl",
                "languageId": "pkl",
                "version": 1,
                "text": src
            }}
        }),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;

    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/hover",
            "params": {
                "textDocument": {"uri": "file:///tmp/member.pkl"},
                "position": {"line": 0, "character": 12}
            }
        }),
    )
    .await;
    let hover = read_message(&mut client_from_server).await;
    let value = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(value.contains("length: Int"), "hover: {}", value);
    assert!(value.contains("on `String`"), "hover: {}", value);

    // Goto-def on a stdlib member returns null.
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "textDocument/definition",
            "params": {
                "textDocument": {"uri": "file:///tmp/member.pkl"},
                "position": {"line": 0, "character": 12}
            }
        }),
    )
    .await;
    let def = read_message(&mut client_from_server).await;
    assert!(
        def["result"].is_null(),
        "stdlib member def should be null, got: {:?}",
        def
    );

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), server_handle).await;
}

#[tokio::test]
async fn cross_file_with_namespace() {
    use tempfile::tempdir;

    // Build a namespace target on disk:
    //   $NS_ROOT/config.pkl
    //     port: Int = 8080
    let ns_dir = tempdir().unwrap();
    let ns_file = ns_dir.path().join("config.pkl");
    std::fs::write(&ns_file, "port: Int = 8080\n").unwrap();

    // And a main file that imports through the `switchyard` namespace.
    let main_dir = tempdir().unwrap();
    let main_path = main_dir.path().join("main.pkl");
    let src = "import \"switchyard:config.pkl\" as cfg\nport = cfg.port\n";
    std::fs::write(&main_path, src).unwrap();

    let (client_to_server, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, mut client_from_server) = tokio::io::duplex(64 * 1024);

    let (service, socket) = LspService::new(Backend::new);
    let server = Server::new(server_in, server_out, socket).serve(service);
    let server_handle = tokio::spawn(server);

    let mut writer = client_to_server;

    // Wire up the namespace via initializationOptions.
    let init_params = json!({
        "processId": null,
        "rootUri": null,
        "capabilities": {},
        "initializationOptions": {
            "namespaces": {
                "switchyard": ns_dir.path().to_string_lossy()
            }
        }
    });
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params": init_params}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )
    .await;

    // Open main.pkl. We use the file:// URI corresponding to its tempdir
    // path so the graph's resolver-side URI matches.
    let main_uri = format!("file://{}", main_path.display());
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": main_uri,
                "languageId": "pkl",
                "version": 1,
                "text": src
            }}
        }),
    )
    .await;
    let diags = read_message(&mut client_from_server).await;
    let arr = diags["params"]["diagnostics"].as_array().unwrap();
    assert!(arr.is_empty(), "expected no diagnostics, got {:?}", arr);

    // Hover on `cfg.port` — the `port` identifier starts at column 13 of
    // line 1.
    // Line 1: `port = cfg.port`
    //          0123456789012345
    //                     ^^^^   `port` (the member) at column 11.
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "textDocument/hover",
            "params": {
                "textDocument": {"uri": main_uri},
                "position": {"line": 1, "character": 12}
            }
        }),
    )
    .await;
    let hover = read_message(&mut client_from_server).await;
    let value = hover["result"]["contents"]["value"]
        .as_str()
        .expect("hover value");
    assert!(
        value.contains("port: Int"),
        "expected cross-file hover, got: {}",
        value
    );

    // Goto-def on `cfg.port` should jump into the imported file.
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "textDocument/definition",
            "params": {
                "textDocument": {"uri": main_uri},
                "position": {"line": 1, "character": 12}
            }
        }),
    )
    .await;
    let def = read_message(&mut client_from_server).await;
    let result = &def["result"];
    let target_uri = result["uri"]
        .as_str()
        .unwrap_or_else(|| panic!("expected definition uri, got {:?}", def));
    assert!(
        target_uri.contains("config.pkl"),
        "expected jump into config.pkl, got {}",
        target_uri
    );

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), server_handle).await;
}

#[tokio::test]
async fn completion_on_member_and_top_level() {
    let (client_to_server, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, mut client_from_server) = tokio::io::duplex(64 * 1024);

    let (service, socket) = LspService::new(Backend::new);
    let server = Server::new(server_in, server_out, socket).serve(service);
    let server_handle = tokio::spawn(server);

    let mut writer = client_to_server;

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"processId":null,"rootUri":null,"capabilities":{}}}),
    )
    .await;
    let init = read_message(&mut client_from_server).await;
    assert!(init["result"]["capabilities"]["completionProvider"].is_object());
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )
    .await;

    // Layout (line 0):  x = "hi".
    //                   0123456789012
    // Trigger right after the `.` at column 9.
    let src = "x = \"hi\".\n";
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": "file:///tmp/comp.pkl",
                "languageId": "pkl",
                "version": 1,
                "text": src
            }}
        }),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;

    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/completion",
            "params": {
                "textDocument": {"uri": "file:///tmp/comp.pkl"},
                "position": {"line": 0, "character": 9}
            }
        }),
    )
    .await;
    let resp = read_message(&mut client_from_server).await;
    let items = resp["result"].as_array().expect("array result");
    let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
    assert!(
        labels.contains(&"length"),
        "expected `length`, got {:?}",
        labels
    );
    assert!(
        labels.contains(&"contains"),
        "expected `contains`, got {:?}",
        labels
    );

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), server_handle).await;
}

#[tokio::test]
async fn completion_inside_import_string_offers_workspace_files() {
    use tempfile::tempdir;

    // Workspace layout:
    //   $ROOT/main.pkl     (the file the cursor sits in)
    //   $ROOT/sibling.pkl  (a workspace candidate)
    //   $ROOT/sub/leaf.pkl (a descended candidate)
    let root = tempdir().unwrap();
    let main_path = root.path().join("main.pkl");
    let sibling_path = root.path().join("sibling.pkl");
    let leaf_path = root.path().join("sub/leaf.pkl");
    std::fs::create_dir_all(root.path().join("sub")).unwrap();
    let main_src = "import \"sib\"\n";
    std::fs::write(&main_path, main_src).unwrap();
    std::fs::write(&sibling_path, "x: Int = 1\n").unwrap();
    std::fs::write(&leaf_path, "y: Int = 2\n").unwrap();

    let (client_to_server, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, mut client_from_server) = tokio::io::duplex(64 * 1024);

    let (service, socket) = LspService::new(Backend::new);
    let server = Server::new(server_in, server_out, socket).serve(service);
    let server_handle = tokio::spawn(server);

    let mut writer = client_to_server;

    let root_uri = format!("file://{}", root.path().display());
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "capabilities": {},
                "workspaceFolders": [{
                    "uri": root_uri,
                    "name": "root"
                }]
            }
        }),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )
    .await;

    let main_uri = format!("file://{}", main_path.display());
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": main_uri,
                "languageId": "pkl",
                "version": 1,
                "text": main_src
            }}
        }),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;

    // Trigger completion with the cursor at the end of `sib` inside
    // the import quotes. Layout:
    //   import "sib"
    //   012345678901
    //   character 11 sits between `b` and the closing quote.
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/completion",
            "params": {
                "textDocument": {"uri": main_uri},
                "position": {"line": 0, "character": 11}
            }
        }),
    )
    .await;
    let resp = read_message(&mut client_from_server).await;
    let items = resp["result"].as_array().expect("array result");

    let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
    assert!(
        labels.contains(&"sibling.pkl"),
        "expected sibling.pkl in {:?}",
        labels
    );

    // Verify `sibling.pkl` carries the correct text_edit and filter_text.
    let sibling_item = items
        .iter()
        .find(|i| i["label"] == "sibling.pkl")
        .expect("sibling completion item");
    assert_eq!(sibling_item["filterText"], "sibling.pkl");
    assert_eq!(sibling_item["textEdit"]["newText"], "sibling.pkl");
    // Replace from inside the opening quote (column 8) to current cursor (11).
    assert_eq!(sibling_item["textEdit"]["range"]["start"]["character"], 8);
    assert_eq!(sibling_item["textEdit"]["range"]["end"]["character"], 11);
    assert_eq!(sibling_item["kind"], 17); // 17 = File in LSP.

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), server_handle).await;
}

#[tokio::test]
async fn completion_at_empty_import_string_lists_all_candidates() {
    use tempfile::tempdir;

    let root = tempdir().unwrap();
    let main_path = root.path().join("main.pkl");
    let a_path = root.path().join("alpha.pkl");
    let b_path = root.path().join("beta.pkl");
    let main_src = "import \"\"\n";
    std::fs::write(&main_path, main_src).unwrap();
    std::fs::write(&a_path, "x: Int = 1\n").unwrap();
    std::fs::write(&b_path, "y: Int = 2\n").unwrap();

    let (client_to_server, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, mut client_from_server) = tokio::io::duplex(64 * 1024);

    let (service, socket) = LspService::new(Backend::new);
    let server = Server::new(server_in, server_out, socket).serve(service);
    let server_handle = tokio::spawn(server);

    let mut writer = client_to_server;
    let root_uri = format!("file://{}", root.path().display());
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": root_uri,
                "capabilities": {},
                "workspaceFolders": [{"uri": root_uri, "name": "root"}]
            }
        }),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )
    .await;

    let main_uri = format!("file://{}", main_path.display());
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": main_uri,
                "languageId": "pkl",
                "version": 1,
                "text": main_src
            }}
        }),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;

    // `import ""` — cursor inside empty string at column 8.
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/completion",
            "params": {
                "textDocument": {"uri": main_uri},
                "position": {"line": 0, "character": 8}
            }
        }),
    )
    .await;
    let resp = read_message(&mut client_from_server).await;
    let items = resp["result"].as_array().expect("array result");
    let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
    // alpha.pkl and beta.pkl from the workspace scan plus `pkl:`
    // modules as fallback.
    assert!(labels.contains(&"alpha.pkl"), "got {:?}", labels);
    assert!(labels.contains(&"beta.pkl"), "got {:?}", labels);
    assert!(
        labels.iter().any(|l| l.starts_with("pkl:")),
        "expected pkl: modules in fallback, got {:?}",
        labels
    );

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), server_handle).await;
}

#[tokio::test]
async fn completion_inside_pkl_import_unchanged() {
    // Regression guard: `pkl:` imports keep emitting stdlib labels
    // exactly as before.
    let (client_to_server, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, mut client_from_server) = tokio::io::duplex(64 * 1024);

    let (service, socket) = LspService::new(Backend::new);
    let server = Server::new(server_in, server_out, socket).serve(service);
    let server_handle = tokio::spawn(server);

    let mut writer = client_to_server;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"processId":null,"rootUri":null,"capabilities":{}}}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )
    .await;

    let src = "import \"pkl:\"\n";
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": "file:///tmp/pkl-import.pkl",
                "languageId": "pkl",
                "version": 1,
                "text": src
            }}
        }),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;

    // `import "pkl:` — cursor at column 12 (between `:` and `"`).
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/completion",
            "params": {
                "textDocument": {"uri": "file:///tmp/pkl-import.pkl"},
                "position": {"line": 0, "character": 12}
            }
        }),
    )
    .await;
    let resp = read_message(&mut client_from_server).await;
    let items = resp["result"].as_array().expect("array result");
    let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
    // Every item is a `pkl:` module; none carry a textEdit (the legacy
    // branch leaves replacement up to the editor's word-completion).
    assert!(!labels.is_empty(), "expected pkl: modules");
    assert!(
        labels.iter().all(|l| l.starts_with("pkl:")),
        "expected only pkl: items, got {:?}",
        labels
    );
    assert!(
        items.iter().all(|i| i.get("textEdit").is_none()),
        "expected no textEdit on stdlib branch, got {:?}",
        items
    );

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), server_handle).await;
}

#[tokio::test]
async fn completion_after_workspace_folder_removed_returns_empty() {
    use tempfile::tempdir;
    // Simulate the workspace going away at runtime: we initialise the
    // server with a tempdir, drop the tempdir, then verify completion
    // doesn't panic and returns no workspace-file candidates.
    let main_src = "import \"\"\n";
    let main_uri_dir = tempdir().unwrap();
    let main_path = main_uri_dir.path().join("main.pkl");
    std::fs::write(&main_path, main_src).unwrap();

    let throwaway_dir = tempdir().unwrap();
    let throwaway_root = throwaway_dir.path().to_path_buf();

    let (client_to_server, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, mut client_from_server) = tokio::io::duplex(64 * 1024);

    let (service, socket) = LspService::new(Backend::new);
    let server = Server::new(server_in, server_out, socket).serve(service);
    let server_handle = tokio::spawn(server);

    let mut writer = client_to_server;
    let throwaway_uri = format!("file://{}", throwaway_root.display());
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": throwaway_uri,
                "capabilities": {},
                "workspaceFolders": [{"uri": throwaway_uri, "name": "root"}]
            }
        }),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
    )
    .await;
    // Drop the workspace root before opening any document.
    drop(throwaway_dir);

    let main_uri = format!("file://{}", main_path.display());
    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": main_uri,
                "languageId": "pkl",
                "version": 1,
                "text": main_src
            }}
        }),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;

    send(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "textDocument/completion",
            "params": {
                "textDocument": {"uri": main_uri},
                "position": {"line": 0, "character": 8}
            }
        }),
    )
    .await;
    let resp = read_message(&mut client_from_server).await;
    let items = resp["result"].as_array().expect("array result");
    // No workspace files survived. `did_open` adds the current document
    // itself to the index, so the only candidate it would have offered
    // is filtered out by `completions_for`. The stdlib fallback still
    // surfaces `pkl:` items though.
    let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
    assert!(
        labels.iter().all(|l| l.starts_with("pkl:")),
        "expected only pkl: fallback after workspace dropped, got {:?}",
        labels
    );

    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}),
    )
    .await;
    let _ = read_message(&mut client_from_server).await;
    send(
        &mut writer,
        &json!({"jsonrpc":"2.0","method":"exit","params":null}),
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), server_handle).await;
}

async fn send(writer: &mut DuplexStream, value: &Value) {
    let body = serde_json::to_string(value).unwrap();
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await.unwrap();
    writer.write_all(body.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();
}

async fn read_message(reader: &mut DuplexStream) -> Value {
    // Read headers line by line until blank line.
    let mut content_length: Option<usize> = None;
    let mut header_buf = Vec::with_capacity(128);
    loop {
        header_buf.clear();
        loop {
            let mut byte = [0u8; 1];
            reader.read_exact(&mut byte).await.unwrap();
            header_buf.push(byte[0]);
            if header_buf.ends_with(b"\r\n") {
                break;
            }
        }
        let line = std::str::from_utf8(&header_buf).unwrap().trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = Some(rest.trim().parse().unwrap());
        }
    }
    let n = content_length.expect("Content-Length header");
    let mut body = vec![0u8; n];
    reader.read_exact(&mut body).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

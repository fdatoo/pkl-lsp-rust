//! Tower-LSP backend for the Pkl language server.

use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use pkl_analyze::{FsLoader, FsLoaderConfig, ModuleGraph, WorkspaceIndex};
// (`with_stdlib` returns a chained loader that handles `pkl:` first.)

use pkl_syntax::Severity;

use crate::capabilities::server_capabilities;
use crate::code_actions::code_actions_at;
use crate::completion::complete_at;
use crate::config::InitOptions;
use crate::document::Document;
use crate::folding::folding_ranges;
use crate::formatting::format_document;
use crate::goto::definition_at;
use crate::highlights::highlights_at;
use crate::hover::hover_at;
use crate::inlay_hints::inlay_hints;
use crate::references::references_at;
use crate::rename::{prepare_rename_at, rename_at};
use crate::selection_range::selection_ranges as compute_selection_ranges;
use crate::semantic_tokens::semantic_tokens;
use crate::signature_help::signature_help_at;
use crate::symbols::document_symbols;
use crate::uri::url_to_module_uri;
use crate::workspace_symbols::workspace_symbols;

/// Backend state.
///
/// We keep two parallel views of every opened document:
///
/// * `documents` — rope + cached single-file analysis, used as the hot path
///   for hover / goto-definition on the file the cursor is on.
/// * `graph` — module-graph state covering every transitively imported file
///   too. Cross-module queries (hover on an imported member, goto-def into
///   an imported file) consult this.
///
/// Both are kept in sync on `didOpen` / `didChange`. The graph also pulls
/// in any new imports as they appear.
pub struct Backend {
    pub client: Client,
    pub documents: DashMap<Url, Document>,
    pub graph: Arc<RwLock<ModuleGraph>>,
    /// Workspace-scoped `.pkl` file index. Populated on `initialize` from
    /// `root_uri` / `workspace_folders`. Kept up to date via
    /// `did_open` and `did_change_watched_files` hooks.
    pub workspace_index: Arc<RwLock<WorkspaceIndex>>,
    /// Loader-facing configuration retained for editor features that need
    /// to suggest import paths before the graph has loaded a target.
    pub loader_config: Arc<RwLock<FsLoaderConfig>>,
    pub eval_command: Arc<RwLock<Vec<String>>>,
    /// Set after `initialize` when the client advertises
    /// `window.workDoneProgress`. Servers must not create progress
    /// tokens without that capability.
    pub supports_work_done_progress: std::sync::atomic::AtomicBool,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        let loader = FsLoader::with_stdlib(FsLoaderConfig::default());
        Self {
            client,
            documents: DashMap::new(),
            graph: Arc::new(RwLock::new(ModuleGraph::new(loader))),
            workspace_index: Arc::new(RwLock::new(WorkspaceIndex::empty())),
            loader_config: Arc::new(RwLock::new(FsLoaderConfig::default())),
            eval_command: Arc::new(RwLock::new(Vec::new())),
            supports_work_done_progress: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Re-issue diagnostics for the given URL. Pulls local parser/analyzer
    /// diagnostics plus any import-resolution errors surfaced by the module
    /// graph.
    async fn publish_diagnostics(&self, uri: &Url, run_eval: bool) {
        let Some(doc) = self.documents.get(uri) else {
            return;
        };
        let mut diags: Vec<Diagnostic> = doc
            .analysis
            .diagnostics
            .iter()
            .map(|d| Diagnostic {
                range: doc.span_to_range(d.span),
                severity: Some(match d.severity {
                    Severity::Error => DiagnosticSeverity::ERROR,
                    Severity::Warning => DiagnosticSeverity::WARNING,
                }),
                code: None,
                code_description: None,
                source: Some("pkl".to_string()),
                message: d.message.clone(),
                related_information: None,
                tags: None,
                data: None,
            })
            .collect();

        let version = Some(doc.version);

        // Import-resolution errors come from the graph.
        let module_uri = url_to_module_uri(uri);
        let graph = self.graph.read().await;
        if let Some(entry) = graph.get(&module_uri) {
            for err in &entry.import_errors {
                if let Some(info) = doc.analysis.resolution.imports.get(&err.local_name) {
                    diags.push(Diagnostic {
                        range: doc.span_to_range(info.local_name_span),
                        severity: Some(DiagnosticSeverity::WARNING),
                        code: None,
                        code_description: None,
                        source: Some("pkl".to_string()),
                        message: format!("import: {}", err.message),
                        related_information: None,
                        tags: None,
                        data: None,
                    });
                }
            }

            // Unresolved cross-file member accesses: `imported.foo` where
            // the imported module exists but doesn't expose `foo`.
            for member in doc.analysis.inference.member_refs.values() {
                if member.is_resolved() {
                    continue;
                }
                // Receiver must resolve to an import alias.
                let Some(recv_sym_id) = doc
                    .analysis
                    .resolution
                    .symbol_at_offset(member.receiver_span.start)
                else {
                    continue;
                };
                let recv_sym = doc.analysis.resolution.symbol(recv_sym_id);
                if !matches!(recv_sym.kind, pkl_analyze::SymbolKind::Import { .. }) {
                    continue;
                }
                let Some(target) = graph.imported_module(&module_uri, &recv_sym.name) else {
                    continue;
                };
                if graph
                    .lookup_top_level(target, &member.member_name)
                    .is_some()
                {
                    continue;
                }
                diags.push(Diagnostic {
                    range: doc.span_to_range(member.member_name_span),
                    severity: Some(DiagnosticSeverity::WARNING),
                    code: None,
                    code_description: None,
                    source: Some("pkl".to_string()),
                    message: format!(
                        "no member `{}` in imported module `{}`",
                        member.member_name, recv_sym.name
                    ),
                    related_information: None,
                    tags: None,
                    data: None,
                });
            }
        }
        drop(graph);
        drop(doc);
        if run_eval {
            if let Some(eval_diag) = self.eval_diagnostic(uri).await {
                diags.push(eval_diag);
            }
        }
        self.client
            .publish_diagnostics(uri.clone(), diags, version)
            .await;
    }

    async fn eval_diagnostic(&self, uri: &Url) -> Option<Diagnostic> {
        let command = self.eval_command.read().await.clone();
        if command.is_empty() {
            return None;
        }
        let path = uri.to_file_path().ok()?;
        let path_text = path.to_string_lossy().to_string();
        let program = command[0].clone();
        let args: Vec<String> = command[1..]
            .iter()
            .map(|arg| arg.replace("{file}", &path_text))
            .collect();
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new(program).args(args).output()
        })
        .await
        .ok()?;
        match output {
            Ok(output) if output.status.success() => None,
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let message = if stderr.is_empty() { stdout } else { stderr };
                Some(Diagnostic {
                    range: Range::default(),
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: None,
                    code_description: None,
                    source: Some("pkl eval".to_string()),
                    message: if message.is_empty() {
                        "evaluator command failed".to_string()
                    } else {
                        message
                    },
                    related_information: None,
                    tags: None,
                    data: None,
                })
            }
            Err(err) => Some(Diagnostic {
                range: Range::default(),
                severity: Some(DiagnosticSeverity::ERROR),
                code: None,
                code_description: None,
                source: Some("pkl eval".to_string()),
                message: format!("evaluator command failed to start: {}", err),
                related_information: None,
                tags: None,
                data: None,
            }),
        }
    }

    fn upsert_into_graph_blocking(graph: &mut ModuleGraph, uri: &Url, text: &str) {
        let module_uri = url_to_module_uri(uri);
        graph.upsert(module_uri, text.to_string(), true);
    }

    /// Try to start a `window/workDoneProgress` region for `uri`. Returns
    /// `true` on success — the caller must follow up with `end_progress`
    /// using the same token. Skipped silently when the client never
    /// advertised the capability.
    async fn begin_progress(&self, token: &NumberOrString, uri: &Url) -> bool {
        if !self
            .supports_work_done_progress
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return false;
        }
        if self
            .client
            .send_request::<request::WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
                token: token.clone(),
            })
            .await
            .is_err()
        {
            return false;
        }
        self.client
            .send_notification::<notification::Progress>(ProgressParams {
                token: token.clone(),
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                    WorkDoneProgressBegin {
                        title: format!("Loading {}", short_uri(uri)),
                        cancellable: Some(false),
                        message: None,
                        percentage: None,
                    },
                )),
            })
            .await;
        true
    }

    async fn end_progress(&self, token: &NumberOrString) {
        self.client
            .send_notification::<notification::Progress>(ProgressParams {
                token: token.clone(),
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(WorkDoneProgressEnd {
                    message: None,
                })),
            })
            .await;
    }
}

fn short_uri(uri: &Url) -> String {
    uri.path_segments()
        .and_then(|mut s| s.next_back())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uri.to_string())
}

/// Collect workspace roots from an `initialize` payload. Prefers
/// `workspace_folders` when the client advertises them; falls back to
/// `root_uri` for legacy clients. Non-`file://` URIs are skipped.
fn workspace_roots_from_params(params: &InitializeParams) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Some(folders) = &params.workspace_folders {
        for folder in folders {
            if let Ok(path) = folder.uri.to_file_path() {
                roots.push(path);
            }
        }
    }
    if roots.is_empty() {
        #[allow(deprecated)]
        if let Some(uri) = &params.root_uri {
            if let Ok(path) = uri.to_file_path() {
                roots.push(path);
            }
        }
    }
    roots
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Collect workspace roots before consuming the params payload.
        let roots = workspace_roots_from_params(&params);

        let opts = InitOptions::parse(params.initialization_options);
        let eval_command = opts.eval_command.clone();
        let cfg = opts.into_loader_config();
        let loader = FsLoader::with_stdlib(cfg.clone());
        *self.loader_config.write().await = cfg;
        *self.eval_command.write().await = eval_command;
        self.graph.write().await.set_loader(loader);

        let supports_progress = params
            .capabilities
            .window
            .as_ref()
            .and_then(|w| w.work_done_progress)
            .unwrap_or(false);
        self.supports_work_done_progress
            .store(supports_progress, std::sync::atomic::Ordering::Relaxed);

        // Scan workspace roots off the request hot path so the response
        // isn't gated on disk I/O.
        let workspace_index = self.workspace_index.clone();
        tokio::task::spawn_blocking(move || {
            let scanned = WorkspaceIndex::scan(roots);
            // We don't have access to the async lock from inside
            // `spawn_blocking`; instead, swap on the next read via a
            // synchronous handoff using `blocking_write` on the Tokio
            // `RwLock` (safe because we're inside a blocking task).
            *workspace_index.blocking_write() = scanned;
        });

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "pkl-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: server_capabilities(),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        tracing::info!("pkl-lsp initialized");
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text;
        let doc = Document::new(text.clone(), params.text_document.version);
        self.documents.insert(uri.clone(), doc);
        // Idempotent: the index dedupes on insert.
        if let Ok(path) = uri.to_file_path() {
            self.workspace_index.write().await.add(path);
        }
        let token = NumberOrString::String(format!("pkl-lsp/load/{}", uri));
        let progress_started = self.begin_progress(&token, &uri).await;
        {
            let mut g = self.graph.write().await;
            Self::upsert_into_graph_blocking(&mut g, &uri, &text);
        }
        if progress_started {
            self.end_progress(&token).await;
        }
        self.publish_diagnostics(&uri, false).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let version = params.text_document.version;
        let text = {
            let Some(mut doc) = self.documents.get_mut(&uri) else {
                return;
            };
            for change in params.content_changes {
                match change.range {
                    Some(range) => doc.apply_change(range, &change.text, version),
                    None => doc.replace(change.text, version),
                }
            }
            doc.rope.to_string()
        };
        {
            let mut g = self.graph.write().await;
            Self::upsert_into_graph_blocking(&mut g, &uri, &text);
        }
        self.publish_diagnostics(&uri, false).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.publish_diagnostics(&params.text_document.uri, true)
            .await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut graph = self.graph.write().await;
        let mut index = self.workspace_index.write().await;
        for change in &params.changes {
            let uri = url_to_module_uri(&change.uri);
            match change.typ {
                // Only refresh modules we already know about. New arbitrary
                // files aren't pulled in until something imports them.
                FileChangeType::CREATED | FileChangeType::CHANGED if graph.get(&uri).is_some() => {
                    graph.refresh_from_loader(&uri);
                }
                FileChangeType::DELETED => {
                    graph.remove(&uri);
                }
                _ => {}
            }
            // Mirror the change into the workspace index so import-path
            // completion stays in sync with disk regardless of whether
            // the module was already in the graph.
            if let Ok(path) = change.uri.to_file_path() {
                match change.typ {
                    FileChangeType::CREATED | FileChangeType::CHANGED => index.add(path),
                    FileChangeType::DELETED => index.remove(&path),
                    _ => {}
                }
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.documents.remove(&uri);
        // Don't remove from the graph — other open files may import it,
        // and we still want their analyses to see its surface.
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let symbols = document_symbols(&doc);
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let graph = self.graph.read().await;
        Ok(hover_at(&doc, &graph, &uri, position))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri.clone();
        let position = params.text_document_position.position;
        let include_decl = params.context.include_declaration;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let graph = self.graph.read().await;
        Ok(references_at(
            &uri,
            &doc,
            &self.documents,
            &graph,
            position,
            include_decl,
        ))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let position = params.position;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        Ok(prepare_rename_at(&doc, position))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri.clone();
        let position = params.text_document_position.position;
        let new_name = params.new_name;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let graph = self.graph.read().await;
        Ok(rename_at(
            &uri,
            &doc,
            &self.documents,
            &graph,
            position,
            new_name,
        ))
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        Ok(highlights_at(&doc, position))
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = params.text_document.uri;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let module_uri = url_to_module_uri(&uri);
        let graph = self.graph.read().await;
        let links = crate::import_paths::document_import_links(&doc, |local_name| {
            graph
                .imported_module(&module_uri, local_name)
                .and_then(|entry| crate::uri::module_uri_to_url(&entry.uri))
        });
        Ok(Some(links))
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = params.text_document.uri;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        Ok(Some(folding_ranges(&doc)))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let uri = params.text_document.uri;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        Ok(Some(compute_selection_ranges(&doc, params.positions)))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        Ok(Some(SemanticTokensResult::Tokens(semantic_tokens(&doc))))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        Ok(format_document(&doc))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        Ok(Some(inlay_hints(&doc, params.range)))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let graph = self.graph.read().await;
        Ok(signature_help_at(&doc, &graph, &uri, position))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let graph = self.graph.read().await;
        Ok(code_actions_at(
            &uri,
            &doc,
            &graph,
            range,
            &params.context.diagnostics,
        ))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let graph = self.graph.read().await;
        Ok(Some(workspace_symbols(&graph, &params.query)))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let graph = self.graph.read().await;
        let workspace_index = self.workspace_index.read().await;
        let loader_config = self.loader_config.read().await;
        Ok(complete_at(
            &doc,
            &graph,
            &workspace_index,
            &loader_config,
            &uri,
            position,
        ))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let position = params.text_document_position_params.position;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        let graph = self.graph.read().await;
        Ok(definition_at(&uri, &doc, &graph, position))
    }
}

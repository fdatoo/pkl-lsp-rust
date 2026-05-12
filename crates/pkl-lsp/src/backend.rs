//! Tower-LSP backend for the Pkl language server.

use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use pkl_analyze::{FsLoader, FsLoaderConfig, ModuleGraph};
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
            supports_work_done_progress: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Re-issue diagnostics for the given URL. Pulls both syntax errors
    /// (from the local document analysis) and any import-resolution errors
    /// surfaced by the module graph.
    async fn publish_diagnostics(&self, uri: &Url) {
        let Some(doc) = self.documents.get(uri) else {
            return;
        };
        let mut diags: Vec<Diagnostic> = doc
            .parsed
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
        self.client
            .publish_diagnostics(uri.clone(), diags, version)
            .await;
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

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let opts = InitOptions::parse(params.initialization_options);
        let cfg = opts.into_loader_config();
        let loader = FsLoader::with_stdlib(cfg);
        self.graph.write().await.set_loader(loader);

        let supports_progress = params
            .capabilities
            .window
            .as_ref()
            .and_then(|w| w.work_done_progress)
            .unwrap_or(false);
        self.supports_work_done_progress
            .store(supports_progress, std::sync::atomic::Ordering::Relaxed);

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
        let token = NumberOrString::String(format!("pkl-lsp/load/{}", uri));
        let progress_started = self.begin_progress(&token, &uri).await;
        {
            let mut g = self.graph.write().await;
            Self::upsert_into_graph_blocking(&mut g, &uri, &text);
        }
        if progress_started {
            self.end_progress(&token).await;
        }
        self.publish_diagnostics(&uri).await;
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
        self.publish_diagnostics(&uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.publish_diagnostics(&params.text_document.uri).await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut graph = self.graph.write().await;
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
        Ok(references_at(&uri, &doc, position, include_decl))
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
        Ok(signature_help_at(&doc, position))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let Some(doc) = self.documents.get(&uri) else {
            return Ok(None);
        };
        Ok(code_actions_at(&uri, &doc, range))
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
        Ok(complete_at(&doc, &graph, position))
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

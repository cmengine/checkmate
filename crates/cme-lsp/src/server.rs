//! The `tower-lsp-server` backend and the stdio entry point for `cme lsp`.
//!
//! The server keeps one salsa [`Database`] for the whole session. Every
//! document operation is a short synchronous query burst under a mutex —
//! nothing is awaited while the lock is held, and diagnostics are published
//! after the lock is released.
//!
//! Feature gating follows the analysis anchoring rule: pure-Checkmate files
//! get the full feature set; §9 schema files get diagnostics only; §8
//! megaprogram files get expansion diagnostics plus the `cme/expand`
//! preview request, because their parse/check spans live in expanded-text
//! coordinates that cannot anchor against the user's buffer.

use std::collections::HashMap;
use std::sync::Mutex;

use salsa::Setter;
use tower_lsp_server::jsonrpc;
use tower_lsp_server::ls_types::{
    self, CompletionOptions, CompletionParams, CompletionResponse, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentSymbolParams,
    DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams, OneOf,
    ReferenceParams, SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensParams,
    SemanticTokensServerCapabilities, ServerCapabilities, ServerInfo, TextDocumentIdentifier,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

use crate::analysis::Analysis;
use crate::convert;
use crate::db::{self, Database, FileKind, SourceFile};

/// Session state: the salsa database plus the uri → input map.
#[derive(Default)]
struct ServerState {
    db: Database,
    files: HashMap<Uri, SourceFile>,
}

impl std::fmt::Debug for ServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerState")
            .field("open_documents", &self.files.len())
            .finish()
    }
}

/// The Checkmate language server backend.
#[derive(Debug)]
pub struct CheckmateLsp {
    client: Client,
    state: Mutex<ServerState>,
}

impl CheckmateLsp {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            state: Mutex::new(ServerState::default()),
        }
    }

    /// Runs `f` with the document's salsa input, under the state lock.
    fn with_file<R>(
        &self,
        uri: &Uri,
        f: impl FnOnce(&dyn salsa::Database, SourceFile) -> R,
    ) -> Option<R> {
        let state = self.state.lock().ok()?;
        let file = *state.files.get(uri)?;
        Some(f(&state.db, file))
    }

    /// Updates (or creates) the salsa input for a document from its full
    /// text and publishes the resulting diagnostics.
    async fn upsert_and_publish(&self, uri: Uri, text: String, version: Option<i32>) {
        let diagnostics = {
            let state = &mut *self.state.lock().expect("lsp state poisoned");
            let ServerState { db, files } = state;
            let kind = db::sniff_kind(&text);
            if let Some(file) = files.get(&uri) {
                file.set_text(db).to(text);
                file.set_kind(db).to(kind);
            } else {
                let file = SourceFile::new(db, text, kind);
                files.insert(uri.clone(), file);
            }
            crate::features::diagnostics::publishable(db, files[&uri])
        };
        let _ = version;
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }

    /// Runs a position-feature computation against the document's analysis.
    /// The analysis borrows the salsa database behind the state lock, so it
    /// never escapes this call — only the owned result does.
    ///
    /// Returns `None` for unopened documents, schema files (diagnostics
    /// only), and megaprogram files (their parse spans live in expanded-text
    /// coordinates — see the module docs).
    fn with_analysis<R>(
        &self,
        uri: &Uri,
        f: impl FnOnce(&Analysis<'_>, &convert::LineIndex, &str) -> R,
    ) -> Option<R> {
        let state = self.state.lock().ok()?;
        let file = *state.files.get(uri)?;
        if file.kind(&state.db) != FileKind::Script {
            return None;
        }
        let text = file.text(&state.db);
        if cme_compiler::mega::expand::mentions_megaprogram(text) {
            return None;
        }
        let parsed = db::parse(&state.db, file);
        let index = convert::line_index(&state.db, file);
        let analysis = Analysis::build(text, parsed.statements(&state.db));
        Some(f(&analysis, &index, text))
    }
}

impl LanguageServer for CheckmateLsp {
    async fn initialize(&self, _: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string()]),
                    ..CompletionOptions::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: crate::features::tokens::legend(),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            ..SemanticTokensOptions::default()
                        },
                    ),
                ),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "cme".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(
                tower_lsp_server::ls_types::MessageType::INFO,
                "cme lsp ready",
            )
            .await;
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        self.upsert_and_publish(document.uri, document.text, Some(document.version))
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // Full sync: the last change carries the whole document.
        let Some(change) = params.content_changes.into_iter().next_back() else {
            return;
        };
        self.upsert_and_publish(
            params.text_document.uri,
            change.text,
            Some(params.text_document.version),
        )
        .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.client
            .publish_diagnostics(params.text_document.uri, Vec::new(), None)
            .await;
    }

    async fn hover(&self, params: HoverParams) -> jsonrpc::Result<Option<ls_types::Hover>> {
        let position = params.text_document_position_params;
        let hover = self
            .with_analysis(&position.text_document.uri, |analysis, index, text| {
                let offset = index.offset(text, position.position);
                crate::features::hover::hover(analysis, offset)
            })
            .flatten();
        Ok(hover)
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> jsonrpc::Result<Option<CompletionResponse>> {
        let position = params.text_document_position;
        let items = self
            .with_analysis(&position.text_document.uri, |analysis, index, text| {
                let offset = index.offset(text, position.position);
                crate::features::completion::completions(analysis, text, offset)
            })
            .unwrap_or_default();
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> jsonrpc::Result<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params;
        let uri = position.text_document.uri.clone();
        let location = self
            .with_analysis(&position.text_document.uri, |analysis, index, text| {
                let offset = index.offset(text, position.position);
                crate::features::definition::definition(analysis, offset)
                    .map(|span| crate::features::definition::location(index, text, uri, span))
            })
            .flatten();
        Ok(location.map(GotoDefinitionResponse::Scalar))
    }

    async fn references(
        &self,
        params: ReferenceParams,
    ) -> jsonrpc::Result<Option<Vec<ls_types::Location>>> {
        let position = params.text_document_position;
        let include_declaration = params.context.include_declaration;
        let uri = position.text_document.uri.clone();
        let locations = self
            .with_analysis(&position.text_document.uri, |analysis, index, text| {
                let offset = index.offset(text, position.position);
                crate::features::definition::references(analysis, offset, include_declaration)
                    .into_iter()
                    .map(|span| {
                        crate::features::definition::location(index, text, uri.clone(), span)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Some(locations))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> jsonrpc::Result<Option<DocumentSymbolResponse>> {
        let symbols = self
            .with_analysis(&params.text_document.uri, |analysis, index, text| {
                crate::features::symbols::document_symbols(analysis, index, text)
            })
            .unwrap_or_default();
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> jsonrpc::Result<Option<ls_types::SemanticTokensResult>> {
        let tokens = self
            .with_analysis(&params.text_document.uri, |analysis, index, text| {
                crate::features::tokens::semantic_tokens(analysis, index, text)
            })
            .map(ls_types::SemanticTokensResult::Tokens);
        Ok(tokens)
    }
}

impl CheckmateLsp {
    /// `cme/expand`: the §8 megaprogram expansion preview. Returns the
    /// expanded (pure Checkmate) text, `None` for files without megaprogram
    /// constructs and for failed expansions (whose diagnostics publish
    /// normally).
    pub async fn expand_preview(
        &self,
        params: TextDocumentIdentifier,
    ) -> jsonrpc::Result<Option<String>> {
        let expanded = self.with_file(&params.uri, |db, file| {
            let text = file.text(db);
            if !cme_compiler::mega::expand::mentions_megaprogram(text) {
                return None;
            }
            let expansion = db::expansion(db, file);
            if !expansion.diagnostics(db).is_empty() {
                return None;
            }
            Some(expansion.text(db).to_string())
        });
        Ok(expanded.flatten())
    }
}

/// Runs the language server over stdio. Blocks until the client
/// disconnects.
pub fn run_stdio() -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start the LSP runtime: {error}"))?;
    runtime.block_on(async {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let (service, socket) = LspService::build(CheckmateLsp::new)
            .custom_method("cme/expand", CheckmateLsp::expand_preview)
            .finish();
        Server::new(stdin, stdout, socket).serve(service).await;
    });
    Ok(())
}

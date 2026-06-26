use std::sync::Arc;
use tower_lsp_server::jsonrpc::Result;
#[allow(unused_imports)]
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

use crate::config;
use crate::offset::Encoding;
use crate::state::WorkspaceState;

/// Indicator strings: a file is parsed during scans only if it contains one of these.
/// On didOpen/didChange, files are always parsed regardless (REQ-ARCH-05).
const INDICATORS: &[&str] = &[
    "from fastapi",
    "import fastapi",
    "from starlette",
    "import starlette",
    "APIRouter",
    "TestClient",
    "httpx.",
    "include_router",
    "add_middleware",
    "url_for",
];

pub struct FastApiLsp {
    client: Client,
    state: Arc<WorkspaceState>,
}

impl LanguageServer for FastApiLsp {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let root = params
            .workspace_folders
            .as_deref()
            .and_then(|f| f.first())
            .and_then(|f| crate::uri::uri_to_path(&f.uri))
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        let cfg = config::load(&root, params.initialization_options);
        *self.state.config.write().await = cfg;

        let show_doc = params
            .capabilities
            .window
            .as_ref()
            .and_then(|w| w.show_document.as_ref())
            .map(|sd| sd.support)
            .unwrap_or(false);
        self.state
            .show_document_supported
            .store(show_doc, std::sync::atomic::Ordering::Relaxed);

        let code_lens_refresh = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.code_lens.as_ref())
            .and_then(|cl| cl.refresh_support)
            .unwrap_or(false);
        self.state
            .code_lens_refresh_supported
            .store(code_lens_refresh, std::sync::atomic::Ordering::Relaxed);

        let file_watch_dynamic = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.did_change_watched_files.as_ref())
            .and_then(|f| f.dynamic_registration)
            .unwrap_or(false);
        self.state
            .file_watch_dynamic_registration
            .store(file_watch_dynamic, std::sync::atomic::Ordering::Relaxed);

        let work_done_progress = params
            .capabilities
            .window
            .as_ref()
            .and_then(|w| w.work_done_progress)
            .unwrap_or(false);
        self.state
            .work_done_progress_supported
            .store(work_done_progress, std::sync::atomic::Ordering::Relaxed);

        let can_create_files = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.workspace_edit.as_ref())
            .and_then(|we| we.resource_operations.as_deref())
            .map(|ops| ops.contains(&ResourceOperationKind::Create))
            .unwrap_or(false);
        self.state
            .can_create_files
            .store(can_create_files, std::sync::atomic::Ordering::Relaxed);

        // Negotiate position encoding: prefer UTF-8 (LSP 3.17)
        let enc = params
            .capabilities
            .general
            .as_ref()
            .and_then(|g| g.position_encodings.as_deref())
            .and_then(|encs| {
                encs.iter()
                    .find(|e| e.as_str() == "utf-8")
                    .map(|_| Encoding::Utf8)
            })
            .unwrap_or(Encoding::Utf16);
        self.state.set_encoding(enc);

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "fastapi-lsp".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            capabilities: server_capabilities(&self.state).await,
            offset_encoding: Some(match enc {
                Encoding::Utf8 => "utf-8".to_owned(),
                Encoding::Utf16 => "utf-16".to_owned(),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        tracing::info!("fastapi-lsp initialized");
        let state = self.state.clone();
        let client = self.client.clone();

        // Register dynamic file watching only when client supports it (REQ-ARCH-12)
        if state
            .file_watch_dynamic_registration
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let watch_registration = Registration {
                id: "fastapi-lsp-file-watch".to_owned(),
                method: "workspace/didChangeWatchedFiles".to_owned(),
                register_options: serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                    watchers: vec![
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/*.py".to_owned()),
                            kind: None,
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/fastapi-lsp.toml".to_owned()),
                            kind: None,
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/pyproject.toml".to_owned()),
                            kind: None,
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/.env*".to_owned()),
                            kind: None,
                        },
                        // Template file extensions — create/delete/rename trigger index rebuild.
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/*.html".to_owned()),
                            kind: None,
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/*.jinja2".to_owned()),
                            kind: None,
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/*.jinja".to_owned()),
                            kind: None,
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/*.j2".to_owned()),
                            kind: None,
                        },
                    ],
                })
                .ok(),
            };
            if let Err(e) = client.register_capability(vec![watch_registration]).await {
                tracing::warn!("dynamic file-watch registration failed: {e}");
            }
        }

        // Start the debounce linker task (REQ-ARCH-04)
        let debounce_state = state.clone();
        let debounce_client = client.clone();
        tokio::spawn(async move {
            debounce_linker(debounce_state, debounce_client).await;
        });

        // Background workspace scan (REQ-ARCH-11: returns before scan finishes)
        tokio::spawn(async move {
            scan_workspace(&state, &client).await;
        });
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let lock = self.state.doc_lock(&uri);
        let _guard = lock.lock().await;

        self.state.open_docs.insert(uri.clone());
        self.state.file_sources.insert(uri.clone(), text.clone());
        let bytes = text.into_bytes();
        if is_template_file(uri.path().as_str()) {
            index_template_file(&self.state, &uri, &bytes);
        } else {
            index_file_forced(&self.state, &uri, bytes).await;
        }
        let env_ignore = self.state.config.read().await.env_ignore.clone();
        publish_diagnostics_for(&self.client, &self.state, &uri, &env_ignore).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version as i64;
        let lock = self.state.doc_lock(&uri);
        let _guard = lock.lock().await;

        if let Some(stored) = self.state.doc_versions.get(&uri)
            && version <= *stored
        {
            tracing::debug!(
                "did_change: ignoring stale version {} for {} (stored {})",
                version,
                uri.as_str(),
                *stored
            );
            return;
        }
        self.state.doc_versions.insert(uri.clone(), version);

        // Apply incremental edits in order to the stored source (REQ-ARCH-03)
        let enc = self.state.get_encoding();
        for change in params.content_changes {
            let new_text = if let Some(range) = change.range {
                // Incremental: replace [range.start, range.end) with change.text
                let current = self
                    .state
                    .file_sources
                    .get(&uri)
                    .map(|s| s.clone())
                    .unwrap_or_default();
                let src = current.as_bytes();
                let start = crate::offset::position_to_offset(src, range.start, enc);
                let end = crate::offset::position_to_offset(src, range.end, enc);
                let (start, end) = if start <= end {
                    (start, end)
                } else {
                    tracing::warn!(
                        "did_change: client sent inverted range, swapping to avoid panic"
                    );
                    (end, start)
                };
                let mut next = current.clone();
                next.replace_range(start..end, &change.text);
                next
            } else {
                // Full-text replacement
                change.text
            };
            self.state.file_sources.insert(uri.clone(), new_text);
        }

        let text = self
            .state
            .file_sources
            .get(&uri)
            .map(|s| s.clone())
            .unwrap_or_default();
        let bytes = text.into_bytes();
        if is_template_file(uri.path().as_str()) {
            index_template_file(&self.state, &uri, &bytes);
        } else {
            index_file_forced(&self.state, &uri, bytes).await;
        }
        // Debounce task will relink and publish after 300ms (REQ-ARCH-04)
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        let lock = self.state.doc_lock(&uri);
        let _guard = lock.lock().await;

        if let Some(text) = params.text {
            self.state.file_sources.insert(uri.clone(), text.clone());
            index_file_forced(&self.state, &uri, text.into_bytes()).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let lock = self.state.doc_lock(&uri);
        let _guard = lock.lock().await;

        self.state.open_docs.remove(&uri);
        self.state.file_sources.remove(&uri);
        self.state.parse_trees.remove(&uri);
        // Facts stay: diagnostics describe the workspace, not open tabs (REQ-ARCH-10)
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        // Merge at the session tier (InitializationOptions tier) — latest wins per key (REQ-CFG-06)
        // Server never pulls workspace/configuration; config arrives by push only.
        if let Ok(overlay) = serde_json::from_value::<crate::config::RawConfig>(params.settings) {
            // Extract workspace root under a short read lock; do FS I/O outside any lock
            let root = self.state.config.read().await.workspace_root.clone();
            let opts = serde_json::to_value(&overlay).unwrap_or_default();
            let join =
                tokio::task::spawn_blocking(move || crate::config::load(&root, Some(opts))).await;
            let updated = match join {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::error!("did_change_configuration: config::load panicked: {e}");
                    return;
                }
            };
            *self.state.config.write().await = updated;
            self.state.bump_generation();
        }
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        for change in params.changes {
            // Config file change: reload config and re-run diagnostics.
            if is_config_file(change.uri.path().as_str()) {
                let workspace_root = self.state.config.read().await.workspace_root.clone();
                if let Ok(cfg) =
                    tokio::task::spawn_blocking(move || crate::config::load(&workspace_root, None))
                        .await
                {
                    *self.state.config.write().await = cfg;
                    tracing::info!("workspace config reloaded");
                }
                self.state.bump_generation();
                continue;
            }

            // Template files (HTML/Jinja) don't go through the Python parser.
            // Only create/delete events change the index (which path exists); modify events
            // don't change the set of files so they don't need a re-link.
            if is_template_file(change.uri.path().as_str()) {
                if change.typ != FileChangeType::CHANGED {
                    self.state.bump_generation();
                }
                continue;
            }

            match change.typ {
                FileChangeType::DELETED => {
                    self.state.file_facts.remove(&change.uri);
                    self.state.bump_generation();
                }
                _ => {
                    // Ignore events for open documents — editor buffer is truth (REQ-ARCH-12)
                    if self.state.open_docs.contains(&change.uri) {
                        continue;
                    }
                    if let Some(path) = crate::uri::uri_to_path(&change.uri)
                        && let Ok(bytes) = std::fs::read(&path)
                        && has_indicators(&bytes)
                    {
                        index_file_forced(&self.state, &change.uri, bytes).await;
                    }
                }
            }
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        if let Some(h) = crate::features::hover::hover(&self.state, uri, pos) {
            return Ok(Some(h));
        }
        if let Some(h) = crate::features::hover::dep_hover(&self.state, uri, pos) {
            return Ok(Some(h));
        }
        if let Some(h) = crate::features::hover::include_hover(&self.state, uri, pos) {
            return Ok(Some(h));
        }
        if let Some(h) = crate::features::hover::env_hover(&self.state, uri, pos) {
            return Ok(Some(h));
        }
        Ok(crate::features::hover::settings_field_hover(
            &self.state,
            uri,
            pos,
        ))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        Ok(crate::features::completion::completion(
            &self.state,
            uri,
            pos,
        ))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        Ok(crate::features::goto::goto(&self.state, uri, pos))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let include_decl = params.context.include_declaration;
        let locs = crate::features::goto::references(&self.state, uri, pos, include_decl);
        Ok(if locs.is_empty() { None } else { Some(locs) })
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let (workspace_root, env_ignore) = {
            let cfg = self.state.config.read().await;
            (cfg.workspace_root.clone(), cfg.env_ignore.clone())
        };
        let show_doc = self
            .state
            .show_document_supported
            .load(std::sync::atomic::Ordering::Relaxed);
        let actions = crate::features::code_actions::code_actions(
            &self.state,
            &params,
            &workspace_root,
            &env_ignore,
            show_doc,
        );
        Ok(if actions.is_empty() {
            None
        } else {
            Some(actions)
        })
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = &params.text_document.uri;
        let range = params.range;
        let hints = crate::features::inlay_hints::inlay_hints(&self.state, uri, range);
        Ok(if hints.is_empty() { None } else { Some(hints) })
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let syms = crate::features::symbols::document_symbols(&self.state, uri);
        Ok(if syms.is_empty() {
            None
        } else {
            Some(DocumentSymbolResponse::Nested(syms))
        })
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        Ok(Some(crate::features::symbols::workspace_symbols(
            &self.state,
            &params.query,
        )))
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = &params.text_document.uri;
        let lenses = crate::features::codelens::code_lenses(&self.state, uri);
        Ok(if lenses.is_empty() {
            None
        } else {
            Some(lenses)
        })
    }

    async fn code_lens_resolve(&self, params: CodeLens) -> Result<CodeLens> {
        Ok(crate::features::codelens::resolve(&self.state, params))
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = &params.text_document.uri;
        let links = crate::features::document_link::document_links(&self.state, uri);
        if links.is_empty() {
            Ok(None)
        } else {
            Ok(Some(links))
        }
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        match params.command.as_str() {
            "fastapi-lsp.showTestRefs" => {
                let show_doc = self
                    .state
                    .show_document_supported
                    .load(std::sync::atomic::Ordering::Relaxed);
                if show_doc {
                    let args = &params.arguments;
                    // Argument is a JSON array of route_id strings
                    let route_ids: Vec<String> = args
                        .first()
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    let linked = self.state.linked.load();
                    'outer: for route_id_str in &route_ids {
                        let route_id = crate::state::RouteId(route_id_str.clone());
                        if let Some(sites) = linked.test_refs.get(&route_id)
                            && let Some(site) = sites.first()
                        {
                            let uri = site.location.uri.clone();
                            let line = site.location.range.start.line;
                            if uri.as_str().starts_with("file://") {
                                let _ = self
                                    .client
                                    .show_document(ShowDocumentParams {
                                        uri,
                                        external: Some(false),
                                        take_focus: Some(true),
                                        selection: Some(Range {
                                            start: Position::new(line, 0),
                                            end: Position::new(line, 0),
                                        }),
                                    })
                                    .await;
                                break 'outer;
                            }
                        }
                    }
                }
                Ok(None)
            }
            "fastapi-lsp.openFileAt" => {
                let args = &params.arguments;
                let uri_str = args
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let line = args.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                if let Ok(uri) = uri_str.parse::<Uri>()
                    && uri.as_str().starts_with("file://")
                {
                    let _ = self
                        .client
                        .show_document(ShowDocumentParams {
                            uri,
                            external: Some(false),
                            take_focus: Some(true),
                            selection: Some(Range {
                                start: Position::new(line, 0),
                                end: Position::new(line, 0),
                            }),
                        })
                        .await;
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let uri = params.text_document.uri;
        let env_ignore = self.state.config.read().await.env_ignore.clone();
        let mut items = crate::features::diagnostics::compute(&self.state, &uri, &env_ignore);
        if let Some(source) = self.state.file_sources.get(&uri) {
            items = crate::features::diagnostics::apply_noqa(items, &source);
        }
        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: None,
                    items,
                },
            }),
        ))
    }

    async fn workspace_diagnostic(
        &self,
        _params: WorkspaceDiagnosticParams,
    ) -> Result<WorkspaceDiagnosticReportResult> {
        let env_ignore = self.state.config.read().await.env_ignore.clone();
        // Collect URIs first: compute() re-locks the same file_facts shard via get(),
        // which deadlocks if a guard from this iterator is still held (REQ-ARCH-08).
        let uris: Vec<Uri> = self
            .state
            .file_facts
            .iter()
            .map(|e| e.key().clone())
            .collect();
        let items: Vec<WorkspaceDocumentDiagnosticReport> = uris
            .into_iter()
            .map(|uri| {
                let mut diags =
                    crate::features::diagnostics::compute(&self.state, &uri, &env_ignore);
                if let Some(source) = self.state.file_sources.get(&uri) {
                    diags = crate::features::diagnostics::apply_noqa(diags, &source);
                }
                WorkspaceDocumentDiagnosticReport::Full(WorkspaceFullDocumentDiagnosticReport {
                    uri,
                    version: None,
                    full_document_diagnostic_report: FullDocumentDiagnosticReport {
                        result_id: None,
                        items: diags,
                    },
                })
            })
            .collect();
        Ok(WorkspaceDiagnosticReportResult::Report(
            WorkspaceDiagnosticReport { items },
        ))
    }
}

async fn server_capabilities(state: &WorkspaceState) -> ServerCapabilities {
    let cfg = state.config.read().await;
    let f = &cfg.features;
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..Default::default()
            },
        )),
        hover_provider: f.hover.then_some(HoverProviderCapability::Simple(true)),
        completion_provider: f.completion.then_some(CompletionOptions {
            trigger_characters: Some(vec![
                "\"".to_owned(),
                "'".to_owned(),
                "/".to_owned(),
                ",".to_owned(),
            ]),
            ..Default::default()
        }),
        definition_provider: f.navigation.then_some(OneOf::Left(true)),
        references_provider: f.navigation.then_some(OneOf::Left(true)),
        document_symbol_provider: f.symbols.then_some(OneOf::Left(true)),
        workspace_symbol_provider: f.symbols.then_some(OneOf::Left(true)),
        code_action_provider: f
            .code_actions
            .then_some(CodeActionProviderCapability::Options(CodeActionOptions {
                code_action_kinds: Some(vec![
                    CodeActionKind::QUICKFIX,
                    CodeActionKind::REFACTOR_EXTRACT,
                    CodeActionKind::REFACTOR_REWRITE,
                    CodeActionKind::SOURCE,
                ]),
                ..Default::default()
            })),
        inlay_hint_provider: f.inlay_hints.then_some(OneOf::Left(true)),
        diagnostic_provider: Some(DiagnosticServerCapabilities::Options(DiagnosticOptions {
            identifier: Some("fastapi-lsp".to_owned()),
            inter_file_dependencies: true,
            workspace_diagnostics: true,
            ..Default::default()
        })),
        document_link_provider: f.document_links.then_some(DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: Default::default(),
        }),
        code_lens_provider: f.code_lens.then_some(CodeLensOptions {
            resolve_provider: Some(true),
        }),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: {
                let mut cmds = vec!["fastapi-lsp.openFileAt".to_owned()];
                if f.code_lens {
                    cmds.push("fastapi-lsp.showTestRefs".to_owned());
                }
                cmds
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub fn has_indicators(src: &[u8]) -> bool {
    let s = std::str::from_utf8(src).unwrap_or("");
    INDICATORS.iter().any(|ind| s.contains(ind))
}

fn is_template_file(path: &str) -> bool {
    matches!(
        path.rsplit('.').next().unwrap_or(""),
        "html" | "jinja2" | "jinja" | "j2"
    )
}

fn is_config_file(path: &str) -> bool {
    matches!(
        path.rsplit('/').next().unwrap_or(""),
        "pyproject.toml" | "fastapi-lsp.toml"
    )
}

pub(crate) fn is_test_file(uri: &Uri) -> bool {
    let path = uri.path().as_str();
    let filename = path.rsplit('/').next().unwrap_or("");
    filename.starts_with("test_")
        || filename.ends_with("_test.py")
        || path.contains("/tests/")
        || path.contains("/test/")
}

/// Lexically scan a template file for url_for/url_path_for sites (REQ-TPL-06).
/// Runs synchronously (no tree-sitter) — template files use a narrow text scan.
fn index_template_file(state: &WorkspaceState, uri: &Uri, src: &[u8]) {
    let sites = crate::parsing::templates::scan_url_for_sites(src);
    state.template_facts.insert(uri.clone(), sites);
    state.bump_generation();
}

/// Run all parsers for a single file and return the combined FileFacts.
/// Shared by the LSP server (index_file_forced) and the CLI scan (check::scan).
pub(crate) fn extract_all_facts(
    src: &[u8],
    tree: &tree_sitter::Tree,
    uri: &Uri,
    is_test: bool,
    client_fixtures: &[String],
    enc: crate::offset::Encoding,
) -> crate::state::FileFacts {
    let mut facts = crate::parsing::routes::extract(src, tree, uri, enc);
    crate::parsing::deps::extract(src, tree, &mut facts, enc);
    crate::parsing::models::extract(src, tree, &mut facts, enc);
    crate::parsing::templates::extract(src, tree, &mut facts, enc);
    crate::parsing::middleware::extract(src, tree, &mut facts, enc);
    crate::parsing::clients::extract(src, tree, &mut facts, is_test, client_fixtures, enc);
    crate::parsing::url_for::extract(src, tree, &mut facts, enc);
    crate::parsing::env::extract(src, tree, &mut facts, enc);
    crate::parsing::annotated::extract(src, tree, &mut facts, enc);
    crate::parsing::security::extract(src, tree, &mut facts, enc);
    facts
}

/// Parse and extract facts from a file unconditionally (didOpen/didChange/didSave).
/// CPU-bound tree-sitter work runs under spawn_blocking (REQ-ARCH-08).
async fn index_file_forced(state: &WorkspaceState, uri: &Uri, src: Vec<u8>) {
    let uri_clone = uri.clone();
    let client_fixtures = state.config.read().await.client_fixtures.clone();
    let is_test = is_test_file(uri);
    let enc = state.get_encoding();
    let join = tokio::task::spawn_blocking(move || {
        let any_indicators = has_indicators(&src);
        let tree = crate::parsing::parse_file(&src);
        let facts = extract_all_facts(&src, &tree, &uri_clone, is_test, &client_fixtures, enc);
        (tree, facts, any_indicators)
    })
    .await;

    let (tree, facts, any_indicators) = match join {
        Ok(tf) => tf,
        Err(e) => {
            tracing::error!(
                "index_file_forced: parser panicked for {}: {e}",
                uri.as_str()
            );
            return; // keep previous facts for this file intact
        }
    };

    if state.open_docs.contains(uri) {
        state.parse_trees.insert(uri.clone(), tree);
    }
    if any_indicators {
        state.file_facts.insert(uri.clone(), facts);
    } else {
        state.file_facts.remove(uri);
    }
    state.bump_generation();
}

async fn publish_diagnostics_for(
    client: &Client,
    state: &WorkspaceState,
    uri: &Uri,
    env_ignore: &[String],
) {
    let mut diags = crate::features::diagnostics::compute(state, uri, env_ignore);
    if let Some(source) = state.file_sources.get(uri) {
        diags = crate::features::diagnostics::apply_noqa(diags, &source);
    }
    client.publish_diagnostics(uri.clone(), diags, None).await;
}

/// Debounced pass-2 linker: wakes when generation changes, waits 300 ms,
/// then links if generation hasn't moved again (REQ-ARCH-04).
async fn debounce_linker(state: Arc<WorkspaceState>, client: Client) {
    let mut rx = state.link_rx.clone();
    let debounce = tokio::time::Duration::from_millis(300);

    loop {
        // Wait for a generation bump
        if rx.changed().await.is_err() {
            break;
        }
        let seen_gen = *rx.borrow();

        tokio::time::sleep(debounce).await;

        // Still the same generation? Run the link.
        if state.current_generation() == seen_gen {
            // Snapshot test-ref counts before relink to detect changes (REQ-LENS-03)
            let old_ref_counts: std::collections::HashMap<crate::state::RouteId, usize> = {
                let old = state.linked.load();
                old.test_refs
                    .iter()
                    .map(|(k, v)| (k.clone(), v.len()))
                    .collect()
            };

            crate::linking::relink(&state).await;

            // Skip publish if a new bump arrived during the relink — the next
            // iteration will relink again and publish a fresh set.
            if state.current_generation() != seen_gen {
                continue;
            }

            // Publish diagnostics for all workspace files (REQ-DIAG-01, REQ-ARCH-10).
            // Collect URIs first so no file_facts iterator guard is held while compute()
            // re-locks the same shard or across the publish await (REQ-ARCH-08 deadlock).
            let env_ignore = state.config.read().await.env_ignore.clone();
            let uris: Vec<Uri> = state.file_facts.iter().map(|e| e.key().clone()).collect();
            for uri in uris {
                let mut diags = crate::features::diagnostics::compute(&state, &uri, &env_ignore);
                if let Some(source) = state.file_sources.get(&uri) {
                    diags = crate::features::diagnostics::apply_noqa(diags, &source);
                }
                client.publish_diagnostics(uri, diags, None).await;
            }

            // Notify client to refresh code lenses only when test-ref counts changed (REQ-LENS-03)
            if state
                .code_lens_refresh_supported
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                let new_linked = state.linked.load();
                let counts_changed = new_linked.test_refs.len() != old_ref_counts.len()
                    || new_linked
                        .test_refs
                        .iter()
                        .any(|(k, v)| old_ref_counts.get(k).copied() != Some(v.len()));
                if counts_changed {
                    let _ = client.code_lens_refresh().await;
                }
            }
        }
        // Generation moved → another bump already queued, loop back
    }
}

async fn scan_workspace(state: &Arc<WorkspaceState>, client: &Client) {
    let root = {
        let cfg = state.config.read().await;
        cfg.workspace_root.clone()
    };

    // workDoneProgress: request token, report begin, scan, report end (REQ-ARCH-11)
    let progress_token = NumberOrString::String("fastapi-lsp/scan".to_owned());
    let supports_progress = state
        .work_done_progress_supported
        .load(std::sync::atomic::Ordering::Relaxed);
    let has_progress = supports_progress
        && client
            .send_request::<tower_lsp_server::ls_types::request::WorkDoneProgressCreate>(
                WorkDoneProgressCreateParams {
                    token: progress_token.clone(),
                },
            )
            .await
            .is_ok();

    if has_progress {
        client
            .send_notification::<tower_lsp_server::ls_types::notification::Progress>(
                ProgressParams {
                    token: progress_token.clone(),
                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                        WorkDoneProgressBegin {
                            title: "fastapi-lsp: indexing workspace".to_owned(),
                            cancellable: Some(false),
                            ..Default::default()
                        },
                    )),
                },
            )
            .await;
    }

    let entries: Vec<_> = walkdir::WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let skip = e.path().components().any(|c| {
                matches!(
                    c.as_os_str().to_str(),
                    Some(".venv") | Some("__pycache__") | Some(".git")
                )
            });
            !skip
        })
        .collect();

    for entry in entries {
        let path = entry.path();
        let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("");
        let filename = path.file_name().and_then(|x| x.to_str()).unwrap_or("");

        if ext == "py" {
            if let Ok(bytes) = std::fs::read(path) {
                // Indicator gating during scan (REQ-ARCH-05)
                if !has_indicators(&bytes) {
                    continue;
                }
                if let Some(uri) = crate::uri::path_to_uri(path) {
                    // Cache source text for apply_noqa; skip if already open via did_open.
                    if !state.open_docs.contains(&uri)
                        && let Ok(text) = String::from_utf8(bytes.clone())
                    {
                        state.file_sources.insert(uri.clone(), text);
                    }
                    index_file_forced(state, &uri, bytes).await;
                }
            }
        } else if ext == "html"
            && let Ok(bytes) = std::fs::read(path)
            && let Some(uri) = crate::uri::path_to_uri(path)
        {
            index_template_file(state, &uri, &bytes);
        } else if is_env_filename(filename)
            && let Ok(src) = std::fs::read_to_string(path)
            && let Some(uri) = crate::uri::path_to_uri(path)
        {
            index_env_file(state, &uri, &src);
        }
    }

    crate::linking::relink(state).await;

    if has_progress {
        client
            .send_notification::<tower_lsp_server::ls_types::notification::Progress>(
                ProgressParams {
                    token: progress_token,
                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(
                        WorkDoneProgressEnd {
                            message: Some("done".to_owned()),
                        },
                    )),
                },
            )
            .await;
    }

    // Publish to all workspace files after initial scan (REQ-DIAG-01, REQ-ARCH-10).
    // Collect URIs first so no file_facts iterator guard is held while compute()
    // re-locks the same shard via file_facts.get() or across the publish await —
    // that re-entrancy deadlocks under a concurrent writer (REQ-ARCH-08).
    let env_ignore = state.config.read().await.env_ignore.clone();
    let uris: Vec<Uri> = state.file_facts.iter().map(|e| e.key().clone()).collect();
    for uri in uris {
        let mut diags = crate::features::diagnostics::compute(state, &uri, &env_ignore);
        if let Some(source) = state.file_sources.get(&uri) {
            diags = crate::features::diagnostics::apply_noqa(diags, &source);
        }
        client.publish_diagnostics(uri, diags, None).await;
    }
}

pub fn is_env_filename(filename: &str) -> bool {
    filename == ".env" || filename.starts_with(".env.") || filename.ends_with(".env")
}

fn index_env_file(state: &WorkspaceState, uri: &Uri, src: &str) {
    let entries = crate::parsing::dotenv::parse(src, uri);
    state.env_file_entries.insert(uri.clone(), entries);
    state.bump_generation();
}

pub async fn run(tcp: Option<(std::net::IpAddr, u16)>) {
    let (service, socket) = LspService::build(|client| {
        let cfg = config::ResolvedConfig::default_for_root(
            std::env::current_dir().unwrap_or_else(|_| ".".into()),
        );
        FastApiLsp {
            client,
            state: WorkspaceState::new(cfg),
        }
    })
    .finish();

    if let Some((address, port)) = tcp {
        use tokio::net::TcpListener;
        let addr = format!("{address}:{port}");
        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("fastapi-lsp: failed to bind {addr}: {e}");
                std::process::exit(1);
            }
        };
        tracing::info!("fastapi-lsp listening on {addr}");
        let (stream, _) = match listener.accept().await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("fastapi-lsp: failed to accept connection on {addr}: {e}");
                std::process::exit(1);
            }
        };
        let (read, write) = tokio::io::split(stream);
        Server::new(read, write, socket).serve(service).await;
    } else {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        Server::new(stdin, stdout, socket).serve(service).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ResolvedConfig;
    use crate::state::WorkspaceState;

    fn make_state() -> std::sync::Arc<WorkspaceState> {
        WorkspaceState::new(ResolvedConfig::default_for_root(std::path::PathBuf::from(
            "/tmp",
        )))
    }

    fn make_uri(path: &str) -> tower_lsp_server::ls_types::Uri {
        format!("file://{path}").parse().unwrap()
    }

    #[tokio::test]
    async fn index_file_forced_inserts_facts_when_indicators_present() {
        let state = make_state();
        let uri = make_uri("/tmp/main.py");
        let src =
            b"from fastapi import FastAPI\napp = FastAPI()\n@app.get('/ping')\ndef ping(): pass\n"
                .to_vec();
        index_file_forced(&state, &uri, src).await;
        assert!(
            state.file_facts.contains_key(&uri),
            "facts should be inserted when indicators present"
        );
    }

    #[tokio::test]
    async fn index_file_forced_removes_stale_facts_when_indicators_disappear() {
        let state = make_state();
        let uri = make_uri("/tmp/utils.py");

        // First index with indicators
        let src_with = b"from fastapi import FastAPI\napp = FastAPI()\n".to_vec();
        index_file_forced(&state, &uri, src_with).await;
        assert!(state.file_facts.contains_key(&uri));

        // Re-index without indicators (user deleted all FastAPI code)
        let src_without = b"def helper(): return 42\n".to_vec();
        index_file_forced(&state, &uri, src_without).await;
        assert!(
            !state.file_facts.contains_key(&uri),
            "stale facts should be removed when indicators disappear"
        );
    }

    #[test]
    fn doc_versions_reject_stale_version() {
        let state = make_state();
        let uri = make_uri("/tmp/a.py");
        // Simulate: version 5 stored
        state.doc_versions.insert(uri.clone(), 5);
        // A stale version (≤5) should be caught
        let stored = state.doc_versions.get(&uri).map(|v| *v).unwrap_or(i64::MIN);
        assert!(3_i64 <= stored, "version 3 should be rejected (≤ stored 5)");
        assert!(5_i64 <= stored, "version 5 (equal) should be rejected");
        assert!(6_i64 > stored, "version 6 should be accepted");
    }
}

//! Owns the sourcekit-lsp process for a project and keeps open documents in
//! sync with it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::Rope;
use gpui_kit::{App, AppContext as _, BorrowAppContext as _, Context, EventEmitter, Task};
use serde_json::{Value, json};

use super::{LanguageServer, ServerEvent, path_to_uri, uri_to_path};
use crate::perf::Perf;

#[derive(Clone, Debug, PartialEq)]
pub enum LspStatus {
    Idle,
    Starting,
    Ready,
    Failed(String),
    Exited,
}

pub enum LspStoreEvent {
    DiagnosticsChanged(PathBuf),
    StatusChanged,
    /// The server asked us to apply an edit (e.g. from a code action command).
    ApplyEdit(lsp_types::WorkspaceEdit),
}

struct Document {
    language_id: &'static str,
    /// Latest text, kept here so syncing never reads the editor (which may
    /// be mid-update when it calls a provider).
    text: Rope,
    version: i32,
    synced_version: i32,
    opened: bool,
}

pub struct LspStore {
    root: PathBuf,
    extra_args: Vec<String>,
    server: Option<Arc<LanguageServer>>,
    status: LspStatus,
    documents: HashMap<PathBuf, Document>,
    diagnostics: BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    progress: BTreeMap<String, String>,
    flush_task: Option<Task<()>>,
    _start_task: Option<Task<()>>,
    _event_task: Option<Task<()>>,
}

impl EventEmitter<LspStoreEvent> for LspStore {}

impl LspStore {
    pub fn new(root: PathBuf, extra_args: Vec<String>) -> Self {
        Self {
            root,
            extra_args,
            server: None,
            status: LspStatus::Idle,
            documents: HashMap::new(),
            diagnostics: BTreeMap::new(),
            progress: BTreeMap::new(),
            flush_task: None,
            _start_task: None,
            _event_task: None,
        }
    }

    pub fn status(&self) -> &LspStatus {
        &self.status
    }

    pub fn server(&self) -> Option<Arc<LanguageServer>> {
        self.server
            .clone()
            .filter(|_| self.status == LspStatus::Ready)
    }

    /// The current `$/progress` titles (e.g. "Indexing").
    pub fn progress_message(&self) -> Option<String> {
        self.progress.values().last().cloned()
    }

    pub fn diagnostics(&self) -> &BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>> {
        &self.diagnostics
    }

    pub fn diagnostics_for(&self, path: &Path) -> &[lsp_types::Diagnostic] {
        self.diagnostics.get(path).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Replaces the extra command-line arguments and restarts the server if
    /// they changed (e.g. once a compilation database is available).
    pub fn set_extra_args(&mut self, args: Vec<String>, cx: &mut Context<Self>) {
        if args != self.extra_args {
            self.extra_args = args;
            if self.status != LspStatus::Idle {
                self.restart(cx);
            }
        }
    }

    pub fn restart(&mut self, cx: &mut Context<Self>) {
        if let Some(server) = self.server.take() {
            cx.background_spawn(async move { server.shutdown().await })
                .detach();
        }
        for doc in self.documents.values_mut() {
            doc.opened = false;
        }
        self.diagnostics.clear();
        self.progress.clear();
        self.status = LspStatus::Idle;
        if !self.documents.is_empty() {
            self.ensure_started(cx);
        }
        cx.emit(LspStoreEvent::StatusChanged);
    }

    fn ensure_started(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, LspStatus::Starting | LspStatus::Ready) {
            return;
        }
        self.status = LspStatus::Starting;
        cx.emit(LspStoreEvent::StatusChanged);
        let root = self.root.clone();
        let args = self.extra_args.clone();
        let (events_tx, events_rx) = smol::channel::unbounded();

        self._start_task = Some(cx.spawn(async move |this, cx| {
            let started = cx
                .background_spawn(async move {
                    let binary = crate::toolchain::find_tool("sourcekit-lsp")?;
                    let log = crate::paths::logs_dir().join("sourcekit-lsp.log");
                    let server = LanguageServer::spawn(
                        "sourcekit-lsp",
                        &binary,
                        &args,
                        &root,
                        Some(log),
                        events_tx,
                    )?;
                    server
                        .initialize(&root, json!({ "backgroundIndexing": true }))
                        .await
                        .map(|_| server)
                })
                .await;

            this.update(cx, |this, cx| match started {
                Ok(server) => {
                    log::info!("sourcekit-lsp ready (pid {})", server.pid());
                    cx.update_global::<Perf, _>(|perf, _| {
                        perf.language_server = Some((server.pid(), Default::default()));
                    });
                    this.server = Some(server);
                    this.status = LspStatus::Ready;
                    this.open_pending_documents(cx);
                    cx.emit(LspStoreEvent::StatusChanged);
                }
                Err(err) => {
                    log::error!("failed to start sourcekit-lsp: {err:#}");
                    this.status = LspStatus::Failed(format!("{err:#}"));
                    cx.emit(LspStoreEvent::StatusChanged);
                }
            })
            .ok();
        }));

        self._event_task = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = events_rx.recv().await {
                let alive = this
                    .update(cx, |this, cx| this.handle_server_event(event, cx))
                    .is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    fn handle_server_event(&mut self, event: ServerEvent, cx: &mut Context<Self>) {
        match event {
            ServerEvent::Notification { method, params } => match method.as_str() {
                "textDocument/publishDiagnostics" => {
                    let Ok(params) =
                        serde_json::from_value::<lsp_types::PublishDiagnosticsParams>(params)
                    else {
                        return;
                    };
                    let Some(path) = uri_to_path(&params.uri) else { return };
                    if params.diagnostics.is_empty() {
                        self.diagnostics.remove(&path);
                    } else {
                        self.diagnostics.insert(path.clone(), params.diagnostics);
                    }
                    cx.emit(LspStoreEvent::DiagnosticsChanged(path));
                }
                "$/progress" => {
                    let token = params
                        .get("token")
                        .map(|t| t.to_string())
                        .unwrap_or_default();
                    let value = params.get("value").cloned().unwrap_or(Value::Null);
                    match value.get("kind").and_then(Value::as_str) {
                        Some("begin") | Some("report") => {
                            let title = value
                                .get("title")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .or_else(|| self.progress.get(&token).cloned())
                                .unwrap_or_default();
                            let message = value.get("message").and_then(Value::as_str);
                            let text = match message {
                                Some(message) if !title.is_empty() => {
                                    format!("{title}: {message}")
                                }
                                Some(message) => message.to_string(),
                                None => title,
                            };
                            self.progress.insert(token, text);
                        }
                        Some("end") => {
                            self.progress.remove(&token);
                        }
                        _ => {}
                    }
                    cx.emit(LspStoreEvent::StatusChanged);
                }
                _ => {}
            },
            ServerEvent::Request { id, method, params } => {
                let Some(server) = self.server.clone() else { return };
                let result = match method.as_str() {
                    "workspace/configuration" => {
                        let count = params
                            .get("items")
                            .and_then(Value::as_array)
                            .map(Vec::len)
                            .unwrap_or(0);
                        Value::Array(vec![Value::Null; count])
                    }
                    "workspace/applyEdit" => {
                        if let Some(edit) = params
                            .get("edit")
                            .cloned()
                            .and_then(|e| serde_json::from_value(e).ok())
                        {
                            cx.emit(LspStoreEvent::ApplyEdit(edit));
                        }
                        json!({ "applied": true })
                    }
                    _ => Value::Null,
                };
                server.respond(id, result);
            }
            ServerEvent::Exited => {
                if self.status == LspStatus::Ready {
                    self.status = LspStatus::Exited;
                    self.server = None;
                    cx.emit(LspStoreEvent::StatusChanged);
                }
            }
        }
    }

    /// Registers an open editor buffer. Starts the server lazily, so projects
    /// without Swift/C files never pay for sourcekit-lsp.
    pub fn open_document(
        &mut self,
        path: PathBuf,
        language_id: &'static str,
        text: Rope,
        cx: &mut Context<Self>,
    ) {
        self.documents.insert(
            path.clone(),
            Document {
                language_id,
                text,
                version: 1,
                synced_version: 1,
                opened: false,
            },
        );
        if self.status == LspStatus::Ready {
            self.send_did_open(&path, cx);
        } else {
            self.ensure_started(cx);
        }
    }

    pub fn close_document(&mut self, path: &Path) {
        if let Some(doc) = self.documents.remove(path)
            && doc.opened
            && let Some(server) = &self.server
        {
            server.notify_raw(
                "textDocument/didClose",
                json!({ "textDocument": { "uri": path_to_uri(path).as_str() } }),
            );
        }
    }

    pub fn document_text(&self, path: &Path) -> Option<Rope> {
        self.documents.get(path).map(|d| d.text.clone())
    }

    pub fn is_open(&self, path: &Path) -> bool {
        self.documents.contains_key(path)
    }

    fn open_pending_documents(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<_> = self
            .documents
            .iter()
            .filter(|(_, d)| !d.opened)
            .map(|(p, _)| p.clone())
            .collect();
        for path in paths {
            self.send_did_open(&path, cx);
        }
    }

    fn send_did_open(&mut self, path: &Path, cx: &App) {
        let Some(server) = self.server.clone() else { return };
        let Some(doc) = self.documents.get_mut(path) else { return };
        let _ = cx;
        let text = doc.text.to_string();
        server.notify_raw(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": path_to_uri(path).as_str(),
                "languageId": doc.language_id,
                "version": doc.version,
                "text": text,
            }}),
        );
        doc.opened = true;
        doc.synced_version = doc.version;
    }

    /// Marks a document as edited; changes are flushed after a short pause so
    /// live diagnostics follow typing without flooding the server.
    pub fn document_changed(&mut self, path: &Path, text: Rope, cx: &mut Context<Self>) {
        let Some(doc) = self.documents.get_mut(path) else { return };
        doc.version += 1;
        doc.text = text;
        if self.flush_task.is_none() {
            self.flush_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                this.update(cx, |this, cx| {
                    this.flush_task = None;
                    this.flush_all(cx);
                })
                .ok();
            }));
        }
    }

    fn flush_all(&mut self, cx: &App) {
        let dirty: Vec<_> = self
            .documents
            .iter()
            .filter(|(_, d)| d.opened && d.version != d.synced_version)
            .map(|(p, _)| p.clone())
            .collect();
        for path in dirty {
            self.flush(&path, cx);
        }
    }

    fn flush(&mut self, path: &Path, cx: &App) {
        let Some(server) = self.server.clone() else { return };
        let Some(doc) = self.documents.get_mut(path) else { return };
        if !doc.opened || doc.version == doc.synced_version {
            return;
        }
        let _ = cx;
        let text = doc.text.to_string();
        server.notify_raw(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": path_to_uri(path).as_str(), "version": doc.version },
                "contentChanges": [{ "text": text }],
            }),
        );
        doc.synced_version = doc.version;
    }

    /// Flushes pending edits for `path` and returns the server, ready for a
    /// position-based request against the current text.
    pub fn synced_server(&mut self, path: &Path, cx: &App) -> Option<Arc<LanguageServer>> {
        if self.status != LspStatus::Ready {
            return None;
        }
        if !self.documents.get(path).is_some_and(|d| d.opened) {
            return None;
        }
        self.flush(path, cx);
        self.server.clone()
    }

    /// Like [`Self::synced_server`], but first adopts `text` (the editor's
    /// current contents) — providers run before change events are delivered.
    pub fn synced_server_with_text(
        &mut self,
        path: &Path,
        text: &Rope,
        cx: &App,
    ) -> Option<Arc<LanguageServer>> {
        if let Some(doc) = self.documents.get_mut(path)
            && doc.text != *text
        {
            doc.text = text.clone();
            doc.version += 1;
        }
        self.synced_server(path, cx)
    }

    pub fn did_save(&mut self, path: &Path, cx: &App) {
        self.flush(path, cx);
        if let Some(server) = self.server()
            && self.documents.get(path).is_some_and(|d| d.opened)
        {
            server.notify_raw(
                "textDocument/didSave",
                json!({ "textDocument": { "uri": path_to_uri(path).as_str() } }),
            );
        }
    }

    /// Tells the server files changed on disk (after a build, git checkout...).
    pub fn files_changed(&self, paths: &[PathBuf]) {
        let Some(server) = self.server() else { return };
        let changes: Vec<_> = paths
            .iter()
            .map(|p| json!({ "uri": path_to_uri(p).as_str(), "type": 2 }))
            .collect();
        server.notify_raw(
            "workspace/didChangeWatchedFiles",
            json!({ "changes": changes }),
        );
    }

    pub fn shutdown(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let server = self.server.take();
        self.status = LspStatus::Idle;
        cx.background_spawn(async move {
            if let Some(server) = server {
                server.shutdown().await;
            }
        })
    }
}

//! Owns a project's language servers and keeps open documents in sync with
//! them.
//!
//! A server runs per kind and folder ([`super::servers`]): sourcekit-lsp
//! for the project, vtsls, the HTML, CSS and JSON servers and the rest for
//! each web app in it. Each starts the first time a file it serves opens,
//! so a project without web files never runs one. A document is sent to
//! every server it has, each at the same version; requests go to the first
//! ready server offering what's asked ([`LspStore::synced_server`]), and
//! diagnostics are every server's.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{App, AppContext as _, BorrowAppContext as _, Context, EventEmitter, Task};
use serde_json::{Value, json};
use zcode_editor::Rope;

use super::install::{self, Found, Lookup};
use super::servers::{self, PackageCache, ServerContext, ServerSpec};
use super::{LanguageServer, ServerEvent, path_to_uri, uri_to_path};
use crate::perf::Perf;

#[derive(Clone, Debug, PartialEq)]
pub enum LspStatus {
    Idle,
    /// npm is installing zcode's copy of it.
    Installing,
    Starting,
    Ready,
    Failed(String),
    Exited,
}

pub enum LspStoreEvent {
    DiagnosticsChanged(PathBuf),
    StatusChanged,
    /// A server became ready (again, after a restart): what it offers may
    /// have changed, so ask again.
    ServerReady,
    /// The server asked us to apply an edit (e.g. from a code action command).
    ApplyEdit(lsp_types::WorkspaceEdit),
    /// The server's answers changed without an edit (sourcekit-lsp sends
    /// these once a file's build settings are known): ask again.
    RefreshInlayHints,
    RefreshSemanticTokens,
}

/// What a running server offers beyond completion, hover, definitions,
/// references, rename and code actions, read from its `initialize` response.
/// A feature it doesn't advertise is never asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Features {
    pub inlay_hints: bool,
    /// The legend, when it has semantic tokens; `semantic_tokens_full`
    /// when it answers for a whole document (otherwise for a range).
    pub semantic_tokens: Option<super::semantic_tokens::Legend>,
    pub semantic_tokens_full: bool,
    pub signature_help: Option<SignatureTriggers>,
    pub call_hierarchy: bool,
    pub type_hierarchy: bool,
    /// `textDocument/inlineCompletion` (LSP 3.18), for edit predictions.
    pub inline_completion: bool,
    /// Characters after which it has completions (`<`, `/` and `"` for
    /// HTML; `.` for most), beyond identifiers.
    pub completion_triggers: Vec<String>,
    /// `completionItem/resolve`: auto-imports, documentation.
    pub completion_resolve: bool,
    /// `codeAction/resolve`: actions whose edit comes when chosen (vtsls).
    pub code_action_resolve: bool,
}

/// Characters that open signature help, and ones that update it while it's
/// open (`(` and `,` for sourcekit-lsp).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignatureTriggers {
    pub triggers: Vec<String>,
    pub retriggers: Vec<String>,
}

impl Features {
    pub fn from_capabilities(capabilities: &Value) -> Self {
        let on = |pointer: &str| !matches!(capabilities.pointer(pointer), None | Some(Value::Null) | Some(Value::Bool(false)));
        let strings = |pointer: &str| -> Vec<String> {
            capabilities
                .pointer(pointer)
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default()
        };
        let semantic = capabilities.get("semanticTokensProvider").filter(|_| on("/semanticTokensProvider"));
        Self {
            inlay_hints: on("/inlayHintProvider"),
            semantic_tokens: semantic.and_then(super::semantic_tokens::Legend::from_capability),
            semantic_tokens_full: semantic.is_some_and(|_| on("/semanticTokensProvider/full")),
            signature_help: on("/signatureHelpProvider").then(|| SignatureTriggers {
                triggers: strings("/signatureHelpProvider/triggerCharacters"),
                retriggers: strings("/signatureHelpProvider/retriggerCharacters"),
            }),
            call_hierarchy: on("/callHierarchyProvider"),
            type_hierarchy: on("/typeHierarchyProvider"),
            inline_completion: on("/inlineCompletionProvider"),
            completion_triggers: strings("/completionProvider/triggerCharacters"),
            completion_resolve: on("/completionProvider/resolveProvider"),
            code_action_resolve: on("/codeActionProvider/resolveProvider"),
        }
    }

    pub fn summary(&self) -> Value {
        json!({
            "inlay_hints": self.inlay_hints,
            "semantic_tokens": self.semantic_tokens.is_some(),
            "semantic_tokens_full": self.semantic_tokens_full,
            "signature_help": self.signature_help.as_ref().map(|s| &s.triggers),
            "call_hierarchy": self.call_hierarchy,
            "type_hierarchy": self.type_hierarchy,
            "inline_completion": self.inline_completion,
        })
    }
}

/// The settings language servers follow (`lsp`, `auto_install_language_servers`
/// and the editor's tabs).
#[derive(Clone, Debug, PartialEq)]
pub struct LspConfig {
    pub auto_install: bool,
    pub servers: BTreeMap<String, crate::settings::LspServerSettings>,
    pub tab_size: u32,
    pub hard_tabs: bool,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self { auto_install: true, servers: BTreeMap::new(), tab_size: 4, hard_tabs: false }
    }
}

impl LspConfig {
    pub fn from_settings(settings: &crate::settings::Settings) -> Self {
        Self {
            auto_install: settings.auto_install_language_servers,
            servers: settings.lsp.clone(),
            tab_size: settings.tab_size,
            hard_tabs: settings.hard_tabs,
        }
    }

    fn server(&self, id: &str) -> crate::settings::LspServerSettings {
        self.servers.get(id).cloned().unwrap_or_default()
    }

    fn enabled(&self, id: &str) -> bool {
        self.servers.get(id).and_then(|s| s.enabled) != Some(false)
    }
}

/// A server of the project: one kind for one folder.
struct Server {
    spec: &'static ServerSpec,
    root: PathBuf,
    server: Option<Arc<LanguageServer>>,
    status: LspStatus,
    features: Features,
    /// What it's answered from for `workspace/configuration`.
    settings: Value,
    /// It takes ranged changes (`textDocumentSync.change` 2), not only the
    /// whole text.
    incremental: bool,
    progress: BTreeMap<String, String>,
    diagnostics: BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    /// It answers `textDocument/diagnostic` (ESLint only does): asked after
    /// each change, each file's last answer's id sent back.
    pull_diagnostics: bool,
    result_ids: HashMap<PathBuf, String>,
    pulls: HashMap<PathBuf, Task<()>>,
    _start_task: Option<Task<()>>,
    _event_task: Option<Task<()>>,
}

impl Server {
    fn ready(&self) -> Option<&Arc<LanguageServer>> {
        self.server.as_ref().filter(|_| self.status == LspStatus::Ready)
    }
}

/// A document's copy at one of its servers.
struct DocumentServer {
    id: usize,
    synced_version: i32,
    /// The text it has, which the next change is a range of.
    synced_text: Rope,
    opened: bool,
}

struct Document {
    language_id: &'static str,
    /// Latest text, kept here so syncing never reads the editor (which may
    /// be mid-update when it calls a provider).
    text: Rope,
    version: i32,
    /// Its servers, in the order they're asked.
    servers: Vec<DocumentServer>,
}

/// One server, as the status bar and `dump` show it.
#[derive(Clone, Debug)]
pub struct ServerSummary {
    pub id: &'static str,
    pub root: PathBuf,
    pub status: LspStatus,
    pub progress: Option<String>,
}

pub struct LspStore {
    root: PathBuf,
    /// sourcekit-lsp's command-line arguments (its compilation database).
    extra_args: Vec<String>,
    config: LspConfig,
    servers: Vec<Server>,
    documents: HashMap<PathBuf, Document>,
    /// Every server's diagnostics, file by file.
    diagnostics: BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    packages: PackageCache,
    /// Documents edited since their diagnostics were last asked for (a
    /// request may have sent the edit already).
    edited: std::collections::HashSet<PathBuf>,
    flush_task: Option<Task<()>>,
}

impl EventEmitter<LspStoreEvent> for LspStore {}

impl LspStore {
    pub fn new(root: PathBuf, extra_args: Vec<String>, config: LspConfig) -> Self {
        Self {
            root,
            extra_args,
            config,
            servers: vec![],
            documents: HashMap::new(),
            diagnostics: BTreeMap::new(),
            packages: PackageCache::default(),
            edited: Default::default(),
            flush_task: None,
        }
    }

    /// The status of `path`'s first server; sourcekit-lsp's (or the first
    /// server's) for a file with none.
    pub fn status_for(&self, path: Option<&Path>) -> LspStatus {
        let id = path
            .and_then(|path| self.documents.get(path))
            .and_then(|doc| doc.servers.first())
            .map(|s| s.id)
            .or_else(|| self.servers.iter().position(|s| s.spec.is_sourcekit()))
            .or((!self.servers.is_empty()).then_some(0));
        id.map(|id| self.servers[id].status.clone()).unwrap_or(LspStatus::Idle)
    }

    /// `path`'s servers, in the order they're asked.
    pub fn servers_for(&self, path: &Path) -> Vec<ServerSummary> {
        let Some(doc) = self.documents.get(path) else { return vec![] };
        doc.servers.iter().map(|s| self.summary_of(s.id)).collect()
    }

    /// Every server started so far.
    pub fn all_servers(&self) -> Vec<ServerSummary> {
        (0..self.servers.len()).map(|id| self.summary_of(id)).collect()
    }

    fn summary_of(&self, id: usize) -> ServerSummary {
        let server = &self.servers[id];
        ServerSummary { id: server.spec.id, root: server.root.clone(), status: server.status.clone(), progress: server.progress.values().last().cloned() }
    }

    /// What `path`'s first ready server offers; nothing until one is ready.
    pub fn features_for(&self, path: &Path) -> Features {
        self.ready_ids(path).next().map(|id| self.servers[id].features.clone()).unwrap_or_default()
    }

    /// What each of `path`'s ready servers offers, in order.
    pub fn document_features(&self, path: &Path) -> Vec<Features> {
        self.ready_ids(path).map(|id| self.servers[id].features.clone()).collect()
    }

    /// Server `id`'s own diagnostics for `path`.
    pub fn diagnostics_of(&self, id: usize, path: &Path) -> &[lsp_types::Diagnostic] {
        self.servers.get(id).and_then(|s| s.diagnostics.get(path)).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The version of the text the servers have, or will have at the next
    /// flush: a response to a request made at one version is for that text.
    pub fn document_version(&self, path: &Path) -> Option<i32> {
        self.documents.get(path).map(|d| d.version)
    }

    /// The current `$/progress` titles (e.g. "Indexing"), sourcekit-lsp's first.
    pub fn progress_message(&self) -> Option<String> {
        let mut servers: Vec<&Server> = self.servers.iter().collect();
        servers.sort_by_key(|s| !s.spec.is_sourcekit());
        servers.into_iter().find_map(|s| s.progress.values().last().cloned())
    }

    pub fn diagnostics(&self) -> &BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>> {
        &self.diagnostics
    }

    pub fn diagnostics_for(&self, path: &Path) -> &[lsp_types::Diagnostic] {
        self.diagnostics.get(path).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The servers' process ids, for memory sampling.
    pub fn pids(&self) -> Vec<i32> {
        self.servers.iter().filter_map(|s| s.ready().map(|server| server.pid())).collect()
    }

    /// Replaces sourcekit-lsp's extra command-line arguments and restarts it
    /// if they changed (e.g. once a compilation database is available).
    pub fn set_extra_args(&mut self, args: Vec<String>, cx: &mut Context<Self>) {
        if args != self.extra_args {
            self.extra_args = args;
            let ids: Vec<usize> = (0..self.servers.len()).filter(|&id| self.servers[id].spec.is_sourcekit()).collect();
            for id in ids {
                self.restart_server(id, cx);
            }
        }
    }

    /// New settings: servers turned off or on, or started differently,
    /// restart; the others are told their new settings.
    pub fn set_config(&mut self, config: LspConfig, cx: &mut Context<Self>) {
        if config == self.config {
            return;
        }
        let old = std::mem::replace(&mut self.config, config);
        let mut restart = vec![];
        let mut push = vec![];
        for (id, server) in self.servers.iter().enumerate() {
            let (before, after) = (old.server(server.spec.id), self.config.server(server.spec.id));
            if before.binary != after.binary || before.initialization_options != after.initialization_options {
                restart.push(id);
            } else if before.settings != after.settings || old.tab_size != self.config.tab_size || old.hard_tabs != self.config.hard_tabs {
                push.push(id);
            }
        }
        for id in push {
            self.push_settings(id);
        }
        for id in restart {
            self.restart_server(id, cx);
        }
        if old.auto_install != self.config.auto_install || old.servers.iter().map(|(k, v)| (k, v.enabled)).ne(self.config.servers.iter().map(|(k, v)| (k, v.enabled))) {
            self.rematch_documents(cx);
        }
    }

    /// Restarts every server.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        self.packages.clear();
        for id in 0..self.servers.len() {
            self.restart_server(id, cx);
        }
        self.rematch_documents(cx);
    }

    fn restart_server(&mut self, id: usize, cx: &mut Context<Self>) {
        let server = &mut self.servers[id];
        if let Some(running) = server.server.take() {
            cx.background_spawn(async move { running.shutdown().await }).detach();
        }
        server.status = LspStatus::Idle;
        server.features = Features::default();
        server.progress.clear();
        server.result_ids.clear();
        server.pulls.clear();
        server._start_task = None;
        server._event_task = None;
        let paths: Vec<PathBuf> = std::mem::take(&mut server.diagnostics).into_keys().collect();
        let mut used = false;
        for doc in self.documents.values_mut() {
            for s in doc.servers.iter_mut().filter(|s| s.id == id) {
                s.opened = false;
                used = true;
            }
        }
        for path in paths {
            self.merge_diagnostics(&path, cx);
        }
        if used {
            self.ensure_started(id, cx);
        }
        self.update_perf(cx);
        cx.emit(LspStoreEvent::StatusChanged);
    }

    /// The servers `path` (a file of `language_id`) should have, starting
    /// none: existing ones' ids, or new idle ones.
    fn match_servers(&mut self, path: &Path, language_id: &str) -> Vec<usize> {
        let inside = path.starts_with(&self.root);
        let mut ids = vec![];
        for spec in servers::SERVERS {
            if !self.config.enabled(spec.id) {
                continue;
            }
            // A file outside the project (a definition in a library) is
            // served by a server already running for the project's files.
            if !inside {
                if spec.languages.contains(&language_id)
                    && let Some(id) = self.servers.iter().position(|s| s.spec.id == spec.id)
                {
                    ids.push(id);
                }
                continue;
            }
            let Some(root) = spec.root_for(path, language_id, &self.root, &mut self.packages) else { continue };
            let id = match self.servers.iter().position(|s| s.spec.id == spec.id && s.root == root) {
                Some(id) => id,
                None => {
                    self.servers.push(Server {
                        spec,
                        root,
                        server: None,
                        status: LspStatus::Idle,
                        features: Features::default(),
                        settings: Value::Null,
                        incremental: false,
                        progress: BTreeMap::new(),
                        diagnostics: BTreeMap::new(),
                        pull_diagnostics: false,
                        result_ids: HashMap::new(),
                        pulls: HashMap::new(),
                        _start_task: None,
                        _event_task: None,
                    });
                    self.servers.len() - 1
                }
            };
            ids.push(id);
        }
        ids
    }

    /// Settings or a `package.json` changed: every document gets the
    /// servers it should have now.
    fn rematch_documents(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<(PathBuf, &'static str)> = self.documents.iter().map(|(p, d)| (p.clone(), d.language_id)).collect();
        for (path, language_id) in paths {
            let wanted = self.match_servers(&path, language_id);
            let Some(doc) = self.documents.get_mut(&path) else { continue };
            let gone: Vec<usize> = doc.servers.iter().filter(|s| !wanted.contains(&s.id)).filter(|s| s.opened).map(|s| s.id).collect();
            let version = doc.version;
            let mut kept: Vec<DocumentServer> = vec![];
            for id in &wanted {
                match doc.servers.iter().position(|s| s.id == *id) {
                    Some(ix) => kept.push(doc.servers.remove(ix)),
                    None => kept.push(DocumentServer { id: *id, synced_version: version, synced_text: Rope::new(), opened: false }),
                }
            }
            doc.servers = kept;
            for id in gone {
                if let Some(server) = self.servers[id].ready() {
                    server.notify_raw("textDocument/didClose", json!({ "textDocument": { "uri": path_to_uri(&path).as_str() } }));
                }
                if self.servers[id].diagnostics.remove(&path).is_some() {
                    self.merge_diagnostics(&path, cx);
                }
            }
            for id in wanted {
                match self.servers[id].status {
                    LspStatus::Ready => {
                        self.send_did_open(&path, id);
                        self.pull_diagnostics(&path, Some(id), cx);
                    }
                    _ => self.ensure_started(id, cx),
                }
            }
        }
        cx.emit(LspStoreEvent::StatusChanged);
    }

    fn ensure_started(&mut self, id: usize, cx: &mut Context<Self>) {
        let server = &mut self.servers[id];
        if !matches!(server.status, LspStatus::Idle | LspStatus::Exited) {
            return;
        }
        server.status = LspStatus::Starting;
        cx.emit(LspStoreEvent::StatusChanged);
        let spec = server.spec;
        let root = server.root.clone();
        let settings = self.config.server(spec.id);
        let lookup = Lookup {
            binary: settings.binary.clone().map(|b| (b.path, b.arguments)),
            root: root.clone(),
            project_root: self.root.clone(),
            auto_install: self.config.auto_install,
        };
        let mut args = if spec.is_sourcekit() { self.extra_args.clone() } else { vec![] };
        let tab = (self.config.tab_size, self.config.hard_tabs);
        // vtsls serves a Vue app's files through Vue's TypeScript plugin.
        let vue = spec.id == "vtsls" && self.packages.uses(&root.join("package.json"), &root, &["vue"], &[]);
        let (events_tx, events_rx) = smol::channel::unbounded();

        let task = cx.spawn(async move |this, cx| {
            let lookup_for_find = lookup.clone();
            // A stand-in (a recording proxy in the e2e tests) when set.
            let stand_in = std::env::var_os("ZCODE_SOURCEKIT_LSP").filter(|_| spec.is_sourcekit()).map(PathBuf::from);
            let found = match stand_in {
                Some(path) => Ok(Found::Program(install::Program { path, args: vec![], env: vec![] })),
                None => cx.background_spawn(async move { install::find(spec, &lookup_for_find) }).await,
            };
            let program = match found {
                Ok(Found::Program(program)) => Ok(program),
                Ok(Found::Install) => {
                    this.update(cx, |this, cx| {
                        this.servers[id].status = LspStatus::Installing;
                        cx.emit(LspStoreEvent::StatusChanged);
                    })
                    .ok();
                    let log = crate::paths::logs_dir().join(format!("{}-install.log", spec.id));
                    cx.background_spawn(async move { install::install(spec, &lookup, &log) }).await
                }
                Err(err) => Err(err),
            };
            let started = match program {
                Ok(mut program) => {
                    let project_root = this.update(cx, |this, _| this.root.clone()).unwrap_or_default();
                    let root = root.clone();
                    cx.background_spawn(async move {
                        let mut context = ServerContext {
                            root: Some(root.clone()),
                            typescript_lib: install::typescript_lib(spec, &root, &project_root),
                            tab_size: tab.0,
                            hard_tabs: tab.1,
                            ..Default::default()
                        };
                        if vue {
                            context.vue_plugin = vue_plugin(&root, &project_root);
                        }
                        let mut options = spec.initialization_options(&context);
                        if let Some(own) = &settings.initialization_options {
                            servers::merge_json(&mut options, own);
                        }
                        let mut server_settings = spec.settings(&context);
                        if let Some(own) = &settings.settings {
                            servers::merge_json(&mut server_settings, own);
                        }
                        program.args.append(&mut args);
                        let log = crate::paths::logs_dir().join(format!("{}.log", spec.id));
                        let server = LanguageServer::spawn(spec.id, &program.path, &program.args, &program.env, &root, Some(log), events_tx)?;
                        // sourcekit-lsp pushes its diagnostics, as it always has.
                        server.initialize(&root, options, !spec.is_sourcekit()).await?;
                        if !server_settings.as_object().is_none_or(|o| o.is_empty()) {
                            server.notify_raw("workspace/didChangeConfiguration", json!({ "settings": server_settings }));
                        }
                        anyhow::Ok((server, server_settings))
                    })
                    .await
                }
                Err(err) => Err(err),
            };
            this.update(cx, |this, cx| this.started(id, started, cx)).ok();
        });

        let events = cx.spawn(async move |this, cx| {
            while let Ok(event) = events_rx.recv().await {
                if this.update(cx, |this, cx| this.handle_server_event(id, event, cx)).is_err() {
                    break;
                }
            }
        });
        let server = &mut self.servers[id];
        server._start_task = Some(task);
        server._event_task = Some(events);
    }

    fn started(&mut self, id: usize, started: anyhow::Result<(Arc<LanguageServer>, Value)>, cx: &mut Context<Self>) {
        let server = &mut self.servers[id];
        match started {
            Ok((running, settings)) => {
                log::info!("{} ready for {} (pid {})", server.spec.id, server.root.display(), running.pid());
                let capabilities = running.capabilities();
                server.features = Features::from_capabilities(&capabilities);
                server.pull_diagnostics = capabilities.get("diagnosticProvider").is_some_and(|p| !p.is_null() && *p != Value::Bool(false));
                server.incremental = matches!(capabilities.get("textDocumentSync"), Some(sync) if sync.as_u64() == Some(2) || sync.pointer("/change").and_then(Value::as_u64) == Some(2));
                server.settings = settings;
                server.server = Some(running);
                server.status = LspStatus::Ready;
                let paths: Vec<PathBuf> =
                    self.documents.iter().filter(|(_, d)| d.servers.iter().any(|s| s.id == id && !s.opened)).map(|(p, _)| p.clone()).collect();
                for path in paths {
                    self.send_did_open(&path, id);
                    self.pull_diagnostics(&path, Some(id), cx);
                }
                self.update_perf(cx);
                cx.emit(LspStoreEvent::StatusChanged);
                cx.emit(LspStoreEvent::ServerReady);
            }
            Err(err) => {
                log::error!("failed to start {}: {err:#}", server.spec.id);
                server.status = LspStatus::Failed(format!("{err:#}"));
                cx.emit(LspStoreEvent::StatusChanged);
            }
        }
    }

    fn update_perf(&self, cx: &mut Context<Self>) {
        let pids = self.pids();
        cx.update_global::<Perf, _>(|perf, _| perf.set_language_servers(pids));
    }

    /// Tells server `id` its settings again (they changed).
    fn push_settings(&mut self, id: usize) {
        let spec = self.servers[id].spec;
        let context = ServerContext {
            root: Some(self.servers[id].root.clone()),
            typescript_lib: install::typescript_lib(spec, &self.servers[id].root, &self.root),
            vue_plugin: (spec.id == "vtsls").then(|| install::vue_language_server(&self.servers[id].root, &self.root)).flatten(),
            tab_size: self.config.tab_size,
            hard_tabs: self.config.hard_tabs,
        };
        let mut settings = spec.settings(&context);
        if let Some(own) = &self.config.server(spec.id).settings {
            servers::merge_json(&mut settings, own);
        }
        let server = &mut self.servers[id];
        if let Some(running) = server.ready() {
            running.notify_raw("workspace/didChangeConfiguration", json!({ "settings": settings }));
        }
        server.settings = settings;
    }

    fn handle_server_event(&mut self, id: usize, event: ServerEvent, cx: &mut Context<Self>) {
        match event {
            ServerEvent::Notification { method, params } => match method.as_str() {
                "textDocument/publishDiagnostics" => {
                    let Ok(params) = serde_json::from_value::<lsp_types::PublishDiagnosticsParams>(params) else {
                        return;
                    };
                    let Some(path) = uri_to_path(&params.uri) else { return };
                    let server = &mut self.servers[id];
                    if params.diagnostics.is_empty() {
                        server.diagnostics.remove(&path);
                    } else {
                        server.diagnostics.insert(path.clone(), params.diagnostics);
                    }
                    self.merge_diagnostics(&path, cx);
                }
                "$/progress" => {
                    let progress = &mut self.servers[id].progress;
                    let token = params.get("token").map(|t| t.to_string()).unwrap_or_default();
                    let value = params.get("value").cloned().unwrap_or(Value::Null);
                    match value.get("kind").and_then(Value::as_str) {
                        Some("begin") | Some("report") => {
                            let title = value
                                .get("title")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .or_else(|| progress.get(&token).cloned())
                                .unwrap_or_default();
                            let message = value.get("message").and_then(Value::as_str);
                            let text = match message {
                                Some(message) if !title.is_empty() => format!("{title}: {message}"),
                                Some(message) => message.to_string(),
                                None => title,
                            };
                            progress.insert(token, text);
                        }
                        Some("end") => {
                            progress.remove(&token);
                        }
                        _ => {}
                    }
                    cx.emit(LspStoreEvent::StatusChanged);
                }
                // Vue's server asks TypeScript (vtsls, with Vue's plugin) about
                // the script and the template's expressions through the client.
                "tsserver/request" => self.forward_to_tsserver(id, params, cx),
                "window/showMessage" | "window/logMessage" => {
                    let message = params.get("message").and_then(Value::as_str).unwrap_or("");
                    // 1: error, 2: warning.
                    match params.get("type").and_then(Value::as_u64) {
                        Some(1) => log::warn!("{}: {message}", self.servers[id].spec.id),
                        _ => log::debug!("{}: {message}", self.servers[id].spec.id),
                    }
                }
                _ => log::debug!("{}: {method} {}", self.servers[id].spec.id, params.to_string().chars().take(300).collect::<String>()),
            },
            ServerEvent::Request { id: request, method, params } => {
                let Some(server) = self.servers[id].server.clone() else { return };
                log::debug!("{} asks {method} {}", self.servers[id].spec.id, params.to_string().chars().take(300).collect::<String>());
                let result = match method.as_str() {
                    "workspace/configuration" => {
                        let settings = &self.servers[id].settings;
                        let spec = self.servers[id].spec;
                        let items = params.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                        Value::Array(
                            items.iter().map(|item| spec.configuration_item(settings, item.get("section").and_then(Value::as_str))).collect(),
                        )
                    }
                    "workspace/workspaceFolders" => {
                        let root = &self.servers[id].root;
                        json!([{
                            "uri": path_to_uri(root).as_str(),
                            "name": root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                        }])
                    }
                    "workspace/applyEdit" => {
                        if let Some(edit) = params.get("edit").cloned().and_then(|e| serde_json::from_value(e).ok()) {
                            cx.emit(LspStoreEvent::ApplyEdit(edit));
                        }
                        json!({ "applied": true })
                    }
                    "workspace/inlayHint/refresh" => {
                        cx.emit(LspStoreEvent::RefreshInlayHints);
                        Value::Null
                    }
                    "workspace/semanticTokens/refresh" => {
                        cx.emit(LspStoreEvent::RefreshSemanticTokens);
                        Value::Null
                    }
                    "workspace/diagnostic/refresh" => {
                        let paths: Vec<PathBuf> =
                            self.documents.iter().filter(|(_, d)| d.servers.iter().any(|s| s.id == id && s.opened)).map(|(p, _)| p.clone()).collect();
                        for path in paths {
                            self.pull_diagnostics(&path, Some(id), cx);
                        }
                        Value::Null
                    }
                    // Older ESLint servers ask before running the project's
                    // ESLint: 4 is "approved".
                    "eslint/confirmESLintExecution" => json!(4),
                    "eslint/openDoc" | "eslint/probeFailed" | "eslint/noLibrary" | "eslint/noConfig" => json!({}),
                    _ => Value::Null,
                };
                server.respond(request, result);
            }
            ServerEvent::Exited => {
                let server = &mut self.servers[id];
                if matches!(server.status, LspStatus::Ready | LspStatus::Starting) {
                    log::warn!("{} exited", server.spec.id);
                    server.status = LspStatus::Exited;
                    server.server = None;
                    server.features = Features::default();
                    server.progress.clear();
                    let paths: Vec<PathBuf> = std::mem::take(&mut server.diagnostics).into_keys().collect();
                    for doc in self.documents.values_mut() {
                        for s in doc.servers.iter_mut().filter(|s| s.id == id) {
                            s.opened = false;
                        }
                    }
                    for path in paths {
                        self.merge_diagnostics(&path, cx);
                    }
                    self.update_perf(cx);
                    cx.emit(LspStoreEvent::StatusChanged);
                }
            }
        }
    }

    /// `tsserver/request` from Vue's server (`[[id, command, args]]`):
    /// asked of vtsls for the same folder as `typescript.tsserverRequest`,
    /// answered with `tsserver/response` (`[[id, body]]`).
    fn forward_to_tsserver(&mut self, from: usize, params: Value, cx: &mut Context<Self>) {
        let Some(vue) = self.servers[from].ready().cloned() else { return };
        let root = self.servers[from].root.clone();
        let tsserver = self.servers.iter().find(|s| s.spec.id == "vtsls" && s.root == root).and_then(Server::ready).cloned();
        let requests: Vec<Value> = match &params {
            Value::Array(items) if items.first().is_some_and(Value::is_array) => items.clone(),
            Value::Array(_) => vec![params.clone()],
            _ => return,
        };
        for request in requests {
            let (Some(request_id), Some(command)) = (request.get(0).cloned(), request.get(1).and_then(Value::as_str).map(str::to_string)) else {
                continue;
            };
            let args = request.get(2).cloned().unwrap_or(Value::Null);
            log::debug!("tsserver/request {request_id} {command}");
            let vue = vue.clone();
            let Some(tsserver) = tsserver.clone() else {
                vue.notify_raw("tsserver/response", json!([[request_id, null]]));
                continue;
            };
            let answer = tsserver.request_raw(
                "workspace/executeCommand",
                json!({ "command": "typescript.tsserverRequest", "arguments": [command, args, { "isAsync": true, "lowPriority": true }] }),
            );
            cx.background_spawn(async move {
                let answer = answer.await;
                if let Err(err) = &answer {
                    log::debug!("tsserver/request {request_id}: {err:#}");
                }
                let body = answer.ok().and_then(|result| result.get("body").cloned()).unwrap_or(Value::Null);
                vue.notify_raw("tsserver/response", json!([[request_id, body]]));
            })
            .detach();
        }
    }

    /// Rebuilds `path`'s combined diagnostics from each server's.
    fn merge_diagnostics(&mut self, path: &Path, cx: &mut Context<Self>) {
        let merged: Vec<lsp_types::Diagnostic> =
            self.servers.iter().filter_map(|s| s.diagnostics.get(path)).flatten().cloned().collect();
        if merged.is_empty() {
            self.diagnostics.remove(path);
        } else {
            self.diagnostics.insert(path.to_path_buf(), merged);
        }
        cx.emit(LspStoreEvent::DiagnosticsChanged(path.to_path_buf()));
    }

    /// Registers an open editor buffer and starts the servers it needs.
    pub fn open_document(&mut self, path: PathBuf, language_id: &'static str, text: Rope, cx: &mut Context<Self>) {
        let ids = self.match_servers(&path, language_id);
        let servers = ids.iter().map(|&id| DocumentServer { id, synced_version: 1, synced_text: Rope::new(), opened: false }).collect();
        self.documents.insert(path.clone(), Document { language_id, text, version: 1, servers });
        for id in ids {
            match self.servers[id].status {
                LspStatus::Ready => {
                    self.send_did_open(&path, id);
                    self.pull_diagnostics(&path, Some(id), cx);
                }
                _ => self.ensure_started(id, cx),
            }
        }
        cx.emit(LspStoreEvent::StatusChanged);
    }

    pub fn close_document(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(doc) = self.documents.remove(path) else { return };
        // What a server was asked for goes with the file, as a pushing
        // server clears its own.
        let mut pulled = false;
        for server in self.servers.iter_mut().filter(|s| s.pull_diagnostics) {
            server.result_ids.remove(path);
            server.pulls.remove(path);
            pulled |= server.diagnostics.remove(path).is_some();
        }
        if pulled {
            self.merge_diagnostics(path, cx);
        }
        for s in doc.servers.iter().filter(|s| s.opened) {
            if let Some(server) = self.servers[s.id].ready() {
                server.notify_raw("textDocument/didClose", json!({ "textDocument": { "uri": path_to_uri(path).as_str() } }));
            }
        }
    }

    pub fn document_text(&self, path: &Path) -> Option<Rope> {
        self.documents.get(path).map(|d| d.text.clone())
    }

    pub fn is_open(&self, path: &Path) -> bool {
        self.documents.contains_key(path)
    }

    fn send_did_open(&mut self, path: &Path, id: usize) {
        let Some(server) = self.servers[id].ready().cloned() else { return };
        let Some(doc) = self.documents.get_mut(path) else { return };
        let Some(entry) = doc.servers.iter_mut().find(|s| s.id == id) else { return };
        server.notify_raw(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": path_to_uri(path).as_str(),
                "languageId": doc.language_id,
                "version": doc.version,
                "text": doc.text.to_string(),
            }}),
        );
        entry.opened = true;
        entry.synced_version = doc.version;
        entry.synced_text = doc.text.clone();
    }

    /// Marks a document as edited; changes are flushed after a short pause so
    /// live diagnostics follow typing without flooding the servers.
    pub fn document_changed(&mut self, path: &Path, text: Rope, cx: &mut Context<Self>) {
        let Some(doc) = self.documents.get_mut(path) else { return };
        doc.version += 1;
        doc.text = text;
        self.edited.insert(path.to_path_buf());
        if self.flush_task.is_none() {
            self.flush_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                this.update(cx, |this, cx| {
                    this.flush_task = None;
                    this.flush_all();
                    for path in std::mem::take(&mut this.edited) {
                        this.pull_diagnostics(&path, None, cx);
                    }
                })
                .ok();
            }));
        }
    }

    /// Sends every document's pending edits.
    fn flush_all(&mut self) {
        let dirty: Vec<_> = self
            .documents
            .iter()
            .filter(|(_, d)| d.servers.iter().any(|s| s.opened && s.synced_version != d.version))
            .map(|(p, _)| p.clone())
            .collect();
        for path in &dirty {
            self.flush(path);
        }
    }

    /// Asks `path`'s servers that answer `textDocument/diagnostic` (all of
    /// them, or `only`) for its diagnostics; a newer ask cancels the one
    /// before.
    fn pull_diagnostics(&mut self, path: &Path, only: Option<usize>, cx: &mut Context<Self>) {
        let Some(doc) = self.documents.get(path) else { return };
        let ids: Vec<usize> = doc.servers.iter().filter(|s| s.opened && only.is_none_or(|only| only == s.id)).map(|s| s.id).collect();
        for id in ids {
            let server = &mut self.servers[id];
            if !server.pull_diagnostics {
                continue;
            }
            let Some(running) = server.ready().cloned() else { continue };
            let request = running.request_raw(
                "textDocument/diagnostic",
                json!({ "textDocument": { "uri": path_to_uri(path).as_str() }, "previousResultId": server.result_ids.get(path) }),
            );
            let path = path.to_path_buf();
            let key = path.clone();
            let task = cx.spawn(async move |this, cx| {
                let report = match request.await {
                    Ok(report) => report,
                    Err(err) => {
                        log::debug!("textDocument/diagnostic {}: {err:#}", path.display());
                        return;
                    }
                };
                log::debug!("textDocument/diagnostic {}: {}", path.display(), report.to_string().chars().take(300).collect::<String>());
                this.update(cx, |this, cx| {
                    let server = &mut this.servers[id];
                    server.pulls.remove(&path);
                    if let Some(result) = report.get("resultId").and_then(Value::as_str) {
                        server.result_ids.insert(path.clone(), result.to_string());
                    }
                    // "unchanged": what it said last still holds.
                    if report.get("kind").and_then(Value::as_str) != Some("full") {
                        return;
                    }
                    let items: Vec<lsp_types::Diagnostic> = report.get("items").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
                    if items.is_empty() {
                        server.diagnostics.remove(&path);
                    } else {
                        server.diagnostics.insert(path.clone(), items);
                    }
                    this.merge_diagnostics(&path, cx);
                })
                .ok();
            });
            self.servers[id].pulls.insert(key, task);
        }
    }

    fn flush(&mut self, path: &Path) {
        let Some(doc) = self.documents.get_mut(path) else { return };
        let mut whole = None;
        for entry in doc.servers.iter_mut().filter(|s| s.opened && s.synced_version != doc.version) {
            let server = &self.servers[entry.id];
            let Some(running) = server.ready() else { continue };
            let change = if server.incremental {
                let (old, new) = super::changed_range(&entry.synced_text, &doc.text);
                json!({
                    "range": { "start": super::offset_to_lsp(&entry.synced_text, old.start), "end": super::offset_to_lsp(&entry.synced_text, old.end) },
                    "text": doc.text.slice(new).to_string(),
                })
            } else {
                json!({ "text": whole.get_or_insert_with(|| doc.text.to_string()) })
            };
            running.notify_raw(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": path_to_uri(path).as_str(), "version": doc.version },
                    "contentChanges": [change],
                }),
            );
            entry.synced_version = doc.version;
            entry.synced_text = doc.text.clone();
        }
    }

    /// `path`'s ready servers that have it open, in order.
    fn ready_ids(&self, path: &Path) -> impl Iterator<Item = usize> + '_ {
        self.documents
            .get(path)
            .into_iter()
            .flat_map(|doc| doc.servers.iter())
            .filter(|s| s.opened && self.servers[s.id].ready().is_some())
            .map(|s| s.id)
    }

    /// Adopts `text` (the editor's current contents: providers run before
    /// change events are delivered) and flushes `path`'s pending edits.
    fn sync(&mut self, path: &Path, text: Option<&Rope>) {
        if let Some(text) = text
            && let Some(doc) = self.documents.get_mut(path)
            && doc.text != *text
        {
            doc.text = text.clone();
            doc.version += 1;
            self.edited.insert(path.to_path_buf());
        }
        self.flush(path);
    }

    /// Flushes pending edits for `path` and returns its first ready server,
    /// for a request against the current text.
    pub fn synced_server(&mut self, path: &Path, _cx: &App) -> Option<Arc<LanguageServer>> {
        self.synced_server_supporting(path, None, None)
    }

    /// Like [`Self::synced_server`], but first adopts `text`.
    pub fn synced_server_with_text(&mut self, path: &Path, text: &Rope, _cx: &App) -> Option<Arc<LanguageServer>> {
        self.synced_server_supporting(path, Some(text), None)
    }

    /// `path`'s first ready server that advertises `capability` (a JSON
    /// pointer into its capabilities, like `/renameProvider`), synced.
    pub fn synced_server_supporting(&mut self, path: &Path, text: Option<&Rope>, capability: Option<&str>) -> Option<Arc<LanguageServer>> {
        self.synced_servers(path, text, capability).into_iter().next().map(|(_, server)| server)
    }

    /// Every ready server of `path` advertising `capability`, synced, with
    /// its id (for [`Self::server_by_id`] and [`Self::features_of`]).
    pub fn synced_servers(&mut self, path: &Path, text: Option<&Rope>, capability: Option<&str>) -> Vec<(usize, Arc<LanguageServer>)> {
        self.sync(path, text);
        self.ready_ids(path)
            .filter_map(|id| self.servers[id].ready().map(|server| (id, server.clone())))
            .filter(|(_, server)| capability.is_none_or(|c| server.supports(c)))
            .collect()
    }

    /// The first ready server of `path` whose features `wanted` accepts,
    /// synced with `text`, and those features.
    pub fn synced_server_with(&mut self, path: &Path, text: &Rope, wanted: impl Fn(&Features) -> bool) -> Option<(Arc<LanguageServer>, Features)> {
        self.sync(path, Some(text));
        self.ready_ids(path)
            .find(|&id| wanted(&self.servers[id].features))
            .and_then(|id| self.servers[id].ready().map(|server| (server.clone(), self.servers[id].features.clone())))
    }

    pub fn server_by_id(&self, id: usize) -> Option<Arc<LanguageServer>> {
        self.servers.get(id).and_then(Server::ready).cloned()
    }

    pub fn features_of(&self, id: usize) -> Features {
        self.servers.get(id).map(|s| s.features.clone()).unwrap_or_default()
    }

    /// Every ready server with workspace symbols (Open Quickly, @-mentions).
    pub fn workspace_symbol_servers(&self) -> Vec<Arc<LanguageServer>> {
        self.servers.iter().filter_map(Server::ready).filter(|s| s.supports("/workspaceSymbolProvider")).cloned().collect()
    }

    pub fn did_save(&mut self, path: &Path, _cx: &App) {
        self.flush(path);
        let Some(doc) = self.documents.get(path) else { return };
        for s in doc.servers.iter().filter(|s| s.opened) {
            if let Some(server) = self.servers[s.id].ready() {
                server.notify_raw("textDocument/didSave", json!({ "textDocument": { "uri": path_to_uri(path).as_str() } }));
            }
        }
    }

    /// Tells the servers files changed on disk (after a build, git
    /// checkout...). A changed `package.json` may bring or take servers
    /// (ESLint, Tailwind).
    pub fn files_changed(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let changes: Vec<_> = paths.iter().map(|p| json!({ "uri": path_to_uri(p).as_str(), "type": 2 })).collect();
        for server in self.servers.iter().filter_map(Server::ready) {
            server.notify_raw("workspace/didChangeWatchedFiles", json!({ "changes": changes }));
        }
        if paths.iter().any(|p| p.file_name().is_some_and(|n| n == "package.json")) {
            self.packages.clear();
            self.rematch_documents(cx);
        }
    }

    pub fn shutdown(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let running: Vec<Arc<LanguageServer>> = self.servers.iter_mut().filter_map(|s| s.server.take()).collect();
        for server in &mut self.servers {
            server.status = LspStatus::Idle;
        }
        cx.background_spawn(async move {
            futures::future::join_all(running.iter().map(|server| server.shutdown())).await;
        })
    }

    /// Every server and what it's doing, for `dump`.
    pub fn summary(&self) -> Value {
        Value::Array(
            self.servers
                .iter()
                .enumerate()
                .map(|(id, s)| {
                    json!({
                        "id": s.spec.id,
                        "root": s.root,
                        "status": format!("{:?}", s.status),
                        "pid": s.ready().map(|server| server.pid()),
                        "progress": s.progress.values().last(),
                        "documents": self.documents.values().filter(|d| d.servers.iter().any(|d| d.id == id && d.opened)).count(),
                    })
                })
                .collect(),
        )
    }
}

/// Vue's TypeScript plugin for vtsls: the web app's `@vue/language-server`,
/// else zcode's copy, installed now if need be (Vue's own server, which
/// starts at the same time, installs the same package; one waits for the
/// other).
fn vue_plugin(root: &Path, project_root: &Path) -> Option<PathBuf> {
    if let Some(dir) = install::vue_language_server(root, project_root) {
        return Some(dir);
    }
    let spec = servers::spec("vue-language-server")?;
    let lookup = Lookup { root: root.to_path_buf(), project_root: project_root.to_path_buf(), auto_install: true, ..Default::default() };
    let log = crate::paths::logs_dir().join("vue-language-server-install.log");
    install::install(spec, &lookup, &log).ok()?;
    install::vue_language_server(root, project_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_follow_what_the_server_advertises() {
        // As sourcekit-lsp (Xcode 27) answers `initialize`, trimmed.
        let sourcekit = json!({
            "inlayHintProvider": { "resolveProvider": true },
            "semanticTokensProvider": { "range": true, "full": true, "legend": { "tokenTypes": ["type", "keyword"], "tokenModifiers": [] } },
            "signatureHelpProvider": { "triggerCharacters": ["(", "["], "retriggerCharacters": [",", ":"] },
            "callHierarchyProvider": true,
            "typeHierarchyProvider": true,
            "completionProvider": { "triggerCharacters": ["."] },
        });
        let features = Features::from_capabilities(&sourcekit);
        assert!(features.inlay_hints && features.call_hierarchy && features.type_hierarchy && features.semantic_tokens_full);
        assert!(features.semantic_tokens.is_some());
        assert_eq!(features.signature_help.unwrap().retriggers, [",", ":"]);
        assert_eq!(features.completion_triggers, ["."]);
        assert!(!features.completion_resolve && !features.code_action_resolve);

        // An older server: range tokens only, nothing else.
        let older = json!({ "semanticTokensProvider": { "range": true, "legend": { "tokenTypes": [], "tokenModifiers": [] } }, "callHierarchyProvider": false });
        let features = Features::from_capabilities(&older);
        assert!(features.semantic_tokens.is_some() && !features.semantic_tokens_full);
        assert!(!features.inlay_hints && !features.call_hierarchy && features.signature_help.is_none());
        assert_eq!(Features::from_capabilities(&Value::Null), Features::default());

        // As vtsls answers: resolvable completions and code actions.
        let vtsls = json!({
            "completionProvider": { "triggerCharacters": [".", "\"", "'", "`", "/", "@", "<", "#", " "], "resolveProvider": true },
            "codeActionProvider": { "codeActionKinds": ["quickfix"], "resolveProvider": true },
        });
        let features = Features::from_capabilities(&vtsls);
        assert!(features.completion_resolve && features.code_action_resolve);
        assert!(features.completion_triggers.contains(&"<".to_string()));
    }
}

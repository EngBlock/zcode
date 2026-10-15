//! A small, dependency-light JSON-RPC client for language servers over stdio.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use anyhow::{Context as _, Result, anyhow, bail};
use futures::channel::oneshot;
use futures::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
use parking_lot::Mutex;
use serde_json::{Value, json};
use smol::channel;
use smol::io::BufReader;

/// Something the server sent that is not a response to one of our requests.
#[derive(Debug)]
pub enum ServerEvent {
    Notification { method: String, params: Value },
    Request { id: Value, method: String, params: Value },
    Exited,
}

type PendingMap = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value>>>>>;

pub struct LanguageServer {
    name: String,
    next_id: AtomicI64,
    outgoing: channel::Sender<String>,
    pending: PendingMap,
    pid: i32,
    child: Mutex<Option<smol::process::Child>>,
    capabilities: Mutex<Value>,
    alive: Arc<AtomicBool>,
}

impl LanguageServer {
    /// Spawns the server process and wires up its reader/writer tasks.
    /// Call [`Self::initialize`] next.
    pub fn spawn(
        name: impl Into<String>,
        binary: &Path,
        args: &[String],
        cwd: &Path,
        log_path: Option<PathBuf>,
        events: channel::Sender<ServerEvent>,
    ) -> Result<Arc<Self>> {
        let name = name.into();
        let stderr = match &log_path {
            Some(path) => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).ok();
                }
                std::fs::File::create(path)
                    .map(Stdio::from)
                    .unwrap_or_else(|_| Stdio::null())
            }
            None => Stdio::null(),
        };
        let mut child = smol::process::Command::new(binary)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start {}", binary.display()))?;

        let pid = child.id() as i32;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let (outgoing, outgoing_rx) = channel::unbounded::<String>();
        let pending: PendingMap = Default::default();
        let alive = Arc::new(AtomicBool::new(true));

        // Writer task: frames and writes every queued message.
        std::thread::Builder::new()
            .name(format!("{name}-lsp-writer"))
            .spawn(move || {
                smol::block_on(async move {
                    let mut stdin = stdin;
                    while let Ok(body) = outgoing_rx.recv().await {
                        let header = format!("Content-Length: {}\r\n\r\n", body.len());
                        if stdin.write_all(header.as_bytes()).await.is_err()
                            || stdin.write_all(body.as_bytes()).await.is_err()
                            || stdin.flush().await.is_err()
                        {
                            break;
                        }
                    }
                })
            })?;

        // Reader task: parses frames and routes responses / events.
        {
            let pending = pending.clone();
            let alive = alive.clone();
            let name = name.clone();
            std::thread::Builder::new()
                .name(format!("{name}-lsp-reader"))
                .spawn(move || {
                    smol::block_on(async move {
                        let mut reader = BufReader::new(stdout);
                        loop {
                            match read_message(&mut reader).await {
                                Ok(Some(message)) => {
                                    dispatch(message, &pending, &events).await;
                                }
                                Ok(None) => break,
                                Err(err) => {
                                    log::warn!("{name}: malformed message: {err:#}");
                                    break;
                                }
                            }
                        }
                        alive.store(false, Ordering::SeqCst);
                        for (_, tx) in pending.lock().drain() {
                            tx.send(Err(anyhow!("language server exited"))).ok();
                        }
                        events.send(ServerEvent::Exited).await.ok();
                    })
                })?;
        }

        Ok(Arc::new(Self {
            name,
            next_id: AtomicI64::new(1),
            outgoing,
            pending,
            pid,
            child: Mutex::new(Some(child)),
            capabilities: Mutex::new(Value::Null),
            alive,
        }))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn capabilities(&self) -> Value {
        self.capabilities.lock().clone()
    }

    /// Whether the server advertised `capability` (a JSON pointer like
    /// `/renameProvider`) with a truthy value.
    pub fn supports(&self, pointer: &str) -> bool {
        match self.capabilities.lock().pointer(pointer) {
            None | Some(Value::Null) | Some(Value::Bool(false)) => false,
            Some(_) => true,
        }
    }

    pub async fn initialize(&self, root: &Path, initialization_options: Value) -> Result<Value> {
        let root_uri = super::path_to_uri(root);
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "zcode", "version": env!("CARGO_PKG_VERSION") },
            "rootUri": root_uri.as_str(),
            "rootPath": root.to_string_lossy(),
            "workspaceFolders": [{
                "uri": root_uri.as_str(),
                "name": root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            }],
            "initializationOptions": initialization_options,
            "capabilities": client_capabilities(),
        });
        let result = self.request_raw("initialize", params).await?;
        *self.capabilities.lock() = result.get("capabilities").cloned().unwrap_or(Value::Null);
        self.notify_raw("initialized", json!({}));
        Ok(result)
    }

    /// Sends a request; dropping the returned future before it resolves sends
    /// `$/cancelRequest` so the server can stop working on it.
    pub fn request_raw(
        &self,
        method: &str,
        params: Value,
    ) -> impl std::future::Future<Output = Result<Value>> + Send + 'static {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let sent = self.outgoing.try_send(body.to_string()).is_ok();
        let guard = CancelGuard {
            id,
            outgoing: self.outgoing.clone(),
            pending: self.pending.clone(),
            done: false,
        };
        let method = method.to_string();
        async move {
            let mut guard = guard;
            if !sent {
                guard.done = true;
                bail!("language server is not running");
            }
            let result = rx.await;
            guard.done = true;
            result.map_err(|_| anyhow!("{method}: request dropped"))?
        }
    }

    pub fn request<R: lsp_types::request::Request>(
        &self,
        params: R::Params,
    ) -> impl std::future::Future<Output = Result<R::Result>> + Send + 'static {
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        let fut = self.request_raw(R::METHOD, params);
        async move {
            let value = fut.await?;
            serde_json::from_value(value).with_context(|| format!("decoding {} response", R::METHOD))
        }
    }

    pub fn notify_raw(&self, method: &str, params: Value) {
        let body = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.outgoing.try_send(body.to_string()).ok();
    }

    pub fn notify<N: lsp_types::notification::Notification>(&self, params: N::Params) {
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        self.notify_raw(N::METHOD, params);
    }

    pub fn respond(&self, id: Value, result: Value) {
        let body = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        self.outgoing.try_send(body.to_string()).ok();
    }

    /// Graceful shutdown with a hard kill as the fallback.
    pub async fn shutdown(&self) {
        if self.is_alive() {
            let shutdown = self.request_raw("shutdown", Value::Null);
            let timeout = smol::Timer::after(std::time::Duration::from_millis(500));
            futures::future::select(Box::pin(shutdown), timeout).await;
            self.notify_raw("exit", Value::Null);
        }
        if let Some(mut child) = self.child.lock().take() {
            child.kill().ok();
        }
    }
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.lock().take() {
            child.kill().ok();
        }
    }
}

struct CancelGuard {
    id: i64,
    outgoing: channel::Sender<String>,
    pending: PendingMap,
    done: bool,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if !self.done && self.pending.lock().remove(&self.id).is_some() {
            let body = json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": self.id } });
            self.outgoing.try_send(body.to_string()).ok();
        }
    }
}

async fn read_message<R: smol::io::AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Value>> {
    let mut content_length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            if content_length.is_some() {
                break;
            }
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(value.trim().parse::<usize>()?);
        }
    }
    let mut body = vec![0; content_length.unwrap_or(0)];
    reader.read_exact(&mut body).await?;
    Ok(Some(serde_json::from_slice(&body)?))
}

async fn dispatch(message: Value, pending: &PendingMap, events: &channel::Sender<ServerEvent>) {
    let method = message.get("method").and_then(Value::as_str).map(str::to_string);
    let id = message.get("id").cloned();
    match (method, id) {
        (Some(method), Some(id)) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            events.send(ServerEvent::Request { id, method, params }).await.ok();
        }
        (Some(method), None) => {
            let params = message.get("params").cloned().unwrap_or(Value::Null);
            events.send(ServerEvent::Notification { method, params }).await.ok();
        }
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else { return };
            let Some(tx) = pending.lock().remove(&id) else { return };
            let result = if let Some(error) = message.get("error") {
                Err(anyhow!(
                    "{}",
                    error.get("message").and_then(Value::as_str).unwrap_or("request failed")
                ))
            } else {
                Ok(message.get("result").cloned().unwrap_or(Value::Null))
            };
            tx.send(result).ok();
        }
        (None, None) => {}
    }
}

fn client_capabilities() -> Value {
    json!({
        "general": { "positionEncodings": ["utf-16"] },
        "window": { "workDoneProgress": true, "showMessage": {} },
        "workspace": {
            "applyEdit": true,
            "configuration": true,
            "workspaceFolders": true,
            "workspaceEdit": { "documentChanges": true },
            "symbol": { "symbolKind": { "valueSet": (1..=26).collect::<Vec<_>>() } },
            "didChangeWatchedFiles": { "dynamicRegistration": true },
        },
        "textDocument": {
            "synchronization": { "didSave": true, "willSave": false, "dynamicRegistration": false },
            "completion": {
                "contextSupport": true,
                "completionItem": {
                    "snippetSupport": true,
                    "labelDetailsSupport": true,
                    "documentationFormat": ["markdown", "plaintext"],
                    "insertReplaceSupport": true,
                    "resolveSupport": { "properties": ["documentation", "detail"] },
                },
            },
            "hover": { "contentFormat": ["markdown", "plaintext"] },
            "signatureHelp": { "signatureInformation": { "documentationFormat": ["markdown", "plaintext"] } },
            "definition": { "linkSupport": true },
            "declaration": { "linkSupport": true },
            "typeDefinition": { "linkSupport": true },
            "implementation": { "linkSupport": true },
            "references": {},
            "documentHighlight": {},
            "documentSymbol": {
                "hierarchicalDocumentSymbolSupport": true,
                "symbolKind": { "valueSet": (1..=26).collect::<Vec<_>>() },
            },
            "formatting": {},
            "rangeFormatting": {},
            "rename": { "prepareSupport": true },
            "codeAction": {
                "codeActionLiteralSupport": {
                    "codeActionKind": { "valueSet": [
                        "", "quickfix", "refactor", "refactor.extract", "refactor.inline",
                        "refactor.rewrite", "source", "source.organizeImports"
                    ] }
                },
                "resolveSupport": { "properties": ["edit"] },
            },
            "publishDiagnostics": { "relatedInformation": true, "codeDescriptionSupport": true },
            "callHierarchy": {},
        },
    })
}

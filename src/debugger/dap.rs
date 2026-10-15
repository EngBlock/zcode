//! A Debug Adapter Protocol client for `lldb-dap` over stdio.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use anyhow::{Context as _, Result, anyhow};
use futures::channel::oneshot;
use futures::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
use parking_lot::Mutex;
use serde_json::{Value, json};
use smol::channel;
use smol::io::BufReader;

#[derive(Debug, Clone)]
pub enum DapEvent {
    Event { event: String, body: Value },
    /// A reverse request from the adapter (e.g. `runInTerminal`).
    Request { seq: i64, command: String, arguments: Value },
    Exited,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value>>>>>;

pub struct DapClient {
    seq: AtomicI64,
    outgoing: channel::Sender<String>,
    pending: Pending,
    child: Mutex<Option<smol::process::Child>>,
}

impl DapClient {
    pub fn spawn(binary: &Path, events: channel::Sender<DapEvent>) -> Result<Arc<Self>> {
        let mut child = smol::process::Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start {}", binary.display()))?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let (outgoing, outgoing_rx) = channel::unbounded::<String>();
        let pending: Pending = Default::default();

        std::thread::Builder::new()
            .name("dap-writer".into())
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

        {
            let pending = pending.clone();
            std::thread::Builder::new()
                .name("dap-reader".into())
                .spawn(move || {
                    smol::block_on(async move {
                        let mut reader = BufReader::new(stdout);
                        while let Ok(Some(message)) = read_message(&mut reader).await {
                            match message.get("type").and_then(Value::as_str) {
                                Some("response") => {
                                    let seq = message.get("request_seq").and_then(Value::as_i64).unwrap_or(-1);
                                    if let Some(tx) = pending.lock().remove(&seq) {
                                        let ok = message.get("success").and_then(Value::as_bool).unwrap_or(false);
                                        let result = if ok {
                                            Ok(message.get("body").cloned().unwrap_or(Value::Null))
                                        } else {
                                            Err(anyhow!(
                                                "{}",
                                                message
                                                    .pointer("/body/error/format")
                                                    .or_else(|| message.get("message"))
                                                    .and_then(Value::as_str)
                                                    .unwrap_or("request failed")
                                            ))
                                        };
                                        tx.send(result).ok();
                                    }
                                }
                                Some("event") => {
                                    let event = message.get("event").and_then(Value::as_str).unwrap_or("").to_string();
                                    let body = message.get("body").cloned().unwrap_or(Value::Null);
                                    events.send(DapEvent::Event { event, body }).await.ok();
                                }
                                Some("request") => {
                                    events
                                        .send(DapEvent::Request {
                                            seq: message.get("seq").and_then(Value::as_i64).unwrap_or(0),
                                            command: message.get("command").and_then(Value::as_str).unwrap_or("").to_string(),
                                            arguments: message.get("arguments").cloned().unwrap_or(Value::Null),
                                        })
                                        .await
                                        .ok();
                                }
                                _ => {}
                            }
                        }
                        for (_, tx) in pending.lock().drain() {
                            tx.send(Err(anyhow!("debug adapter exited"))).ok();
                        }
                        events.send(DapEvent::Exited).await.ok();
                    })
                })?;
        }

        Ok(Arc::new(Self {
            seq: AtomicI64::new(1),
            outgoing,
            pending,
            child: Mutex::new(Some(child)),
        }))
    }

    pub fn request(
        &self,
        command: &str,
        arguments: Value,
    ) -> impl std::future::Future<Output = Result<Value>> + Send + 'static {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(seq, tx);
        let message = json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments });
        let sent = self.outgoing.try_send(message.to_string()).is_ok();
        let command = command.to_string();
        async move {
            if !sent {
                return Err(anyhow!("debug adapter is not running"));
            }
            rx.await.map_err(|_| anyhow!("{command}: no response"))?
        }
    }

    pub fn respond(&self, request_seq: i64, command: &str, body: Value) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let message = json!({
            "seq": seq, "type": "response", "request_seq": request_seq,
            "command": command, "success": true, "body": body,
        });
        self.outgoing.try_send(message.to_string()).ok();
    }

    pub fn kill(&self) {
        if let Some(mut child) = self.child.lock().take() {
            child.kill().ok();
        }
    }
}

impl Drop for DapClient {
    fn drop(&mut self) {
        self.kill();
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

/// The `initialize` request zcode sends.
pub fn initialize_args() -> Value {
    json!({
        "clientID": "zcode",
        "clientName": "zcode",
        "adapterID": "lldb-dap",
        "pathFormat": "path",
        "linesStartAt1": true,
        "columnsStartAt1": true,
        "supportsVariableType": true,
        "supportsRunInTerminalRequest": false,
    })
}

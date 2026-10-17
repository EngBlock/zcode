//! One `zcode mcp` connection, served on the main thread: its messages are
//! read and written by threads of their own, requests are answered as they
//! come (a tool call runs as a task, so a minutes-long build doesn't hold
//! up the next request), `notifications/cancelled` stops the call it names,
//! and a connection that closes cancels whatever it still had running.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::{App, AsyncApp, BackgroundExecutor};
use serde_json::Value;

use super::protocol::{self, LEGACY, MODERN};
use super::tools::{self, ToolError};

/// Where a connection comes from, as `zcode mcp` said.
#[derive(Clone, Debug, Default)]
pub struct Origin {
    /// The folder the client started `zcode mcp` in: which project it means.
    pub cwd: Option<PathBuf>,
    /// The agent panel's token, when the panel's agent started it.
    pub agent: Option<String>,
}

/// A tool call in progress.
pub struct Call {
    pub origin: Rc<Origin>,
    cancelled: Cell<bool>,
    progress_token: Option<Value>,
    progress: Cell<u64>,
    out: smol::channel::Sender<String>,
}

impl Call {
    pub fn cancelled(&self) -> bool {
        self.cancelled.get()
    }

    /// Reports progress, if the client asked for it: a step further, and
    /// what's happening now.
    pub fn progress(&self, message: &str) {
        let Some(token) = &self.progress_token else { return };
        let progress = self.progress.get() + 1;
        self.progress.set(progress);
        self.out.send_blocking(protocol::progress_notification(token, progress, message).to_string()).ok();
    }

    /// Resolves once the call is cancelled (checked a few times a second).
    pub async fn until_cancelled(&self, executor: BackgroundExecutor) {
        while !self.cancelled() {
            executor.timer(std::time::Duration::from_millis(200)).await;
        }
    }
}

struct Session {
    origin: Rc<Origin>,
    out: smol::channel::Sender<String>,
    calls: RefCell<HashMap<String, Rc<Call>>>,
}

/// Serves a connection whose first line was `mcp <hello>`.
pub fn serve(hello: &str, reader: BufReader<UnixStream>, stream: UnixStream, cx: &mut App) {
    let hello: Value = serde_json::from_str(hello).unwrap_or_default();
    let origin = Rc::new(Origin {
        cwd: hello.get("cwd").and_then(Value::as_str).map(PathBuf::from),
        agent: hello.get("agent").and_then(Value::as_str).map(str::to_string),
    });
    log::info!("MCP client connected{}", if origin.agent.is_some() { " (an agent in the panel)" } else { "" });
    let (incoming, incoming_rx) = smol::channel::unbounded::<String>();
    let (out, outgoing) = smol::channel::unbounded::<String>();
    let reading = std::thread::Builder::new().name("mcp-reader".into()).spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if !line.trim().is_empty() && incoming.send_blocking(line).is_err() {
                break;
            }
        }
    });
    let Ok(mut writer) = stream.try_clone() else { return };
    let writing = std::thread::Builder::new().name("mcp-writer".into()).spawn(move || {
        while let Ok(line) = outgoing.recv_blocking() {
            if writer.write_all(line.as_bytes()).and_then(|_| writer.write_all(b"\n")).is_err() {
                break;
            }
        }
        stream.shutdown(std::net::Shutdown::Both).ok();
    });
    if reading.is_err() || writing.is_err() {
        return;
    }
    let session = Rc::new(Session { origin, out, calls: RefCell::default() });
    cx.spawn(async move |cx| {
        while let Ok(line) = incoming_rx.recv().await {
            session.handle(&line, cx);
        }
        // The client went away: what it started stops.
        for call in session.calls.borrow().values() {
            call.cancelled.set(true);
        }
        log::info!("MCP client disconnected");
    })
    .detach();
}

impl Session {
    fn send(&self, message: Value) {
        self.out.send_blocking(message.to_string()).ok();
    }

    fn handle(self: &Rc<Self>, line: &str, cx: &mut AsyncApp) {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(err) => return self.send(protocol::error_response(&Value::Null, protocol::PARSE_ERROR, &format!("Parse error: {err}"), None)),
        };
        if message.is_array() {
            // Batches were taken out of the protocol in 2025-06-18.
            return self.send(protocol::error_response(&Value::Null, protocol::INVALID_REQUEST, "Batches aren’t supported", None));
        }
        let method = message.get("method").and_then(Value::as_str);
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match (method, message.get("id")) {
            (Some(method), Some(id)) => self.request(id.clone(), method, params, cx),
            (Some("notifications/cancelled"), None) => {
                if let Some(id) = params.get("requestId")
                    && let Some(call) = self.calls.borrow().get(&id.to_string())
                {
                    log::info!("MCP request {id} cancelled: {}", params.get("reason").and_then(Value::as_str).unwrap_or("no reason given"));
                    call.cancelled.set(true);
                }
            }
            // `notifications/initialized` and anything else zcode needn't act on.
            (Some(_), None) => {}
            // Answers to requests: zcode sends none.
            (None, _) => {}
        }
    }

    fn request(self: &Rc<Self>, id: Value, method: &str, params: Value, cx: &mut AsyncApp) {
        match method {
            // A handshake revision's client: its later requests carry no revision.
            "initialize" => return self.send(protocol::response(&id, protocol::initialize_result(&params))),
            "server/discover" => return self.send(protocol::response(&id, protocol::finish_result(protocol::discover_result(), true))),
            "ping" => return self.send(protocol::response(&id, serde_json::json!({}))),
            _ => {}
        }
        // 2026-07-28 puts the revision in every request; the handshake
        // revisions don't.
        let modern = match protocol::request_version(&params) {
            Some(MODERN) => true,
            Some(version) if LEGACY.contains(&version) => false,
            Some(version) => {
                let (code, message, data) = protocol::unsupported_version(version);
                return self.send(protocol::error_response(&id, code, &message, data));
            }
            None => false,
        };
        match method {
            "tools/list" => {
                let result = protocol::tools_list_result(tools::definitions(), modern);
                self.send(protocol::response(&id, protocol::finish_result(result, modern)));
            }
            "tools/call" => self.call(id, params, modern, cx),
            _ => self.send(protocol::error_response(&id, protocol::METHOD_NOT_FOUND, &format!("Unknown method {method}"), None)),
        }
    }

    fn call(self: &Rc<Self>, id: Value, params: Value, modern: bool, cx: &mut AsyncApp) {
        let Some(name) = params.get("name").and_then(Value::as_str).map(str::to_string) else {
            return self.send(protocol::error_response(&id, protocol::INVALID_PARAMS, "tools/call needs a tool name", None));
        };
        if tools::tool(&name).is_none() {
            return self.send(protocol::error_response(&id, protocol::INVALID_PARAMS, &format!("Unknown tool: {name}"), None));
        }
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => Value::Object(Default::default()),
            Some(arguments) if arguments.is_object() => arguments.clone(),
            Some(_) => return self.send(protocol::error_response(&id, protocol::INVALID_PARAMS, "arguments must be an object", None)),
        };
        let call = Rc::new(Call {
            origin: self.origin.clone(),
            cancelled: Cell::new(false),
            progress_token: protocol::progress_token(&params),
            progress: Cell::new(0),
            out: self.out.clone(),
        });
        let key = id.to_string();
        self.calls.borrow_mut().insert(key.clone(), call.clone());
        let session = self.clone();
        cx.spawn(async move |cx| {
            let result = tools::call(&name, &arguments, &call, cx).await;
            session.calls.borrow_mut().remove(&key);
            // A cancelled request gets no response.
            if call.cancelled() {
                return;
            }
            let message = match result {
                Ok(output) => protocol::response(&id, protocol::finish_result(protocol::tool_result(&output), modern)),
                Err(ToolError::Failed(message)) => protocol::response(&id, protocol::finish_result(protocol::tool_error(&message), modern)),
                Err(ToolError::InvalidParams(message)) => protocol::error_response(&id, protocol::INVALID_PARAMS, &message, None),
                Err(ToolError::Cancelled) => return,
            };
            session.send(message);
        })
        .detach();
    }
}

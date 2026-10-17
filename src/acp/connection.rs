//! A JSON-RPC 2.0 connection to an agent over its stdin and stdout, one
//! message per line (ACP's stdio transport). Unlike a language server, both
//! sides send requests: the agent asks the client to read and write files
//! and for the user's permission, so its requests arrive as events to be
//! answered with [`AgentConnection::respond`].

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use anyhow::{Context as _, Result};
use futures::channel::oneshot;
use parking_lot::Mutex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use smol::channel;

/// How many of the agent's last stderr lines are kept to explain an exit.
const STDERR_TAIL: usize = 40;

/// A JSON-RPC error, either side's.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub const INVALID_PARAMS: i64 = -32602;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INTERNAL: i64 = -32603;
    /// ACP's "authentication required".
    pub const AUTH_REQUIRED: i64 = -32000;
    /// ACP's "resource not found" (a file that doesn't exist).
    pub const NOT_FOUND: i64 = -32002;

    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Self::INTERNAL, message)
    }

    fn from_value(value: &Value) -> Self {
        Self {
            code: value.get("code").and_then(Value::as_i64).unwrap_or(Self::INTERNAL),
            message: value.get("message").and_then(Value::as_str).unwrap_or("error").to_string(),
            data: value.get("data").cloned(),
        }
    }

    fn to_value(&self) -> Value {
        let mut value = json!({ "code": self.code, "message": self.message });
        if let Some(data) = &self.data {
            value["data"] = data.clone();
        }
        value
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        // Agents often put the useful part (a stack, a cause) in `data`.
        if let Some(detail) = self.data.as_ref().and_then(|d| d.as_str().map(str::to_string).or_else(|| d.get("details").and_then(Value::as_str).map(str::to_string))) {
            write!(f, ": {detail}")?;
        }
        Ok(())
    }
}

/// What the agent sent that isn't the answer to one of the client's requests.
#[derive(Debug)]
pub enum Incoming {
    Notification { method: String, params: Value },
    Request { id: Value, method: String, params: Value },
    /// Its output closed: it exited or crashed. `stderr` is what it said last.
    Exited { stderr: String },
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>;

/// How to start an agent.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub program: std::path::PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub struct AgentConnection {
    next_id: AtomicI64,
    outgoing: channel::Sender<String>,
    pending: Pending,
    child: Mutex<Option<Child>>,
    pid: i32,
    alive: Arc<AtomicBool>,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

impl AgentConnection {
    /// Starts the agent in `cwd` (in a process group of its own, so the
    /// processes it starts end with it) and its reader and writer threads.
    pub fn spawn(launch: &Launch, cwd: &Path, events: channel::Sender<Incoming>) -> Result<Arc<Self>> {
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .current_dir(cwd)
            .envs(launch.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = command.spawn().with_context(|| format!("couldn’t start {}", launch.program.display()))?;
        let pid = child.id() as i32;
        let mut stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr_pipe = child.stderr.take().context("no stderr")?;
        let (outgoing, outgoing_rx) = channel::unbounded::<String>();
        let pending: Pending = Default::default();
        let alive = Arc::new(AtomicBool::new(true));
        let stderr: Arc<Mutex<VecDeque<String>>> = Default::default();

        std::thread::Builder::new().name("acp-writer".into()).spawn(move || {
            while let Ok(line) = outgoing_rx.recv_blocking() {
                if stdin.write_all(line.as_bytes()).and_then(|_| stdin.write_all(b"\n")).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        })?;

        {
            let stderr = stderr.clone();
            std::thread::Builder::new().name("acp-stderr".into()).spawn(move || {
                for line in BufReader::new(stderr_pipe).lines().map_while(Result::ok) {
                    log::debug!("agent: {line}");
                    let mut tail = stderr.lock();
                    if tail.len() == STDERR_TAIL {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            })?;
        }

        {
            let (pending, alive, stderr) = (pending.clone(), alive.clone(), stderr.clone());
            let outgoing = outgoing.clone();
            std::thread::Builder::new().name("acp-reader".into()).spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Value>(&line) {
                        Ok(message) => dispatch(message, &pending, &events, &outgoing),
                        // Some agents log to stdout; skip what isn't JSON.
                        Err(_) => log::warn!("agent wrote something that isn’t JSON-RPC: {}", line.chars().take(200).collect::<String>()),
                    }
                }
                alive.store(false, Ordering::SeqCst);
                for (_, tx) in pending.lock().drain() {
                    tx.send(Err(RpcError::internal("the agent exited"))).ok();
                }
                // Give the stderr thread a moment to read the last words.
                std::thread::sleep(std::time::Duration::from_millis(50));
                let stderr = stderr.lock().iter().cloned().collect::<Vec<_>>().join("\n");
                events.send_blocking(Incoming::Exited { stderr }).ok();
            })?;
        }

        Ok(Arc::new(Self { next_id: AtomicI64::new(0), outgoing, pending, child: Mutex::new(Some(child)), pid, alive, stderr }))
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// The last things the agent wrote to stderr.
    pub fn stderr_tail(&self) -> String {
        self.stderr.lock().iter().cloned().collect::<Vec<_>>().join("\n")
    }

    /// Sends a request; the future resolves with its result.
    pub fn request(&self, method: &str, params: Value) -> impl Future<Output = Result<Value, RpcError>> + use<> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        let sent = if self.is_alive() {
            self.pending.lock().insert(id, tx);
            self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
        } else {
            false
        };
        async move {
            if !sent {
                return Err(RpcError::internal("the agent isn’t running"));
            }
            rx.await.unwrap_or_else(|_| Err(RpcError::internal("the agent exited")))
        }
    }

    /// A request whose result is read as `T`.
    pub fn request_as<T: DeserializeOwned, P: Serialize>(&self, method: &str, params: P) -> impl Future<Output = Result<T, RpcError>> + use<T, P> {
        let response = self.request(method, serde_json::to_value(params).unwrap_or(Value::Null));
        async move {
            let value = response.await?;
            serde_json::from_value(value).map_err(|err| RpcError::internal(format!("unexpected response: {err}")))
        }
    }

    pub fn notify(&self, method: &str, params: impl Serialize) {
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Answers one of the agent's requests.
    pub fn respond(&self, id: Value, result: Result<Value, RpcError>) {
        let message = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error.to_value() }),
        };
        self.send(message);
    }

    fn send(&self, message: Value) -> bool {
        self.outgoing.send_blocking(message.to_string()).is_ok()
    }

    /// Ends the agent and whatever it started.
    pub fn kill(&self) {
        self.outgoing.close();
        if let Some(mut child) = self.child.lock().take() {
            // SAFETY: signalling the process group the agent leads.
            unsafe {
                libc::killpg(self.pid, libc::SIGTERM);
            }
            // Reap it off the main thread; force it after a moment.
            std::thread::spawn(move || {
                for _ in 0..20 {
                    if child.try_wait().ok().flatten().is_some() {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                unsafe {
                    libc::killpg(child.id() as i32, libc::SIGKILL);
                }
                child.wait().ok();
            });
        }
    }
}

impl Drop for AgentConnection {
    fn drop(&mut self) {
        self.kill();
    }
}

fn dispatch(message: Value, pending: &Pending, events: &channel::Sender<Incoming>, outgoing: &channel::Sender<String>) {
    let method = message.get("method").and_then(Value::as_str).map(str::to_string);
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match (method, message.get("id").cloned()) {
        (Some(method), Some(id)) => {
            if events.send_blocking(Incoming::Request { id: id.clone(), method, params }).is_err() {
                // Nobody's listening any more: don't leave the agent waiting.
                let error = RpcError::internal("the client closed");
                outgoing.send_blocking(json!({ "jsonrpc": "2.0", "id": id, "error": error.to_value() }).to_string()).ok();
            }
        }
        (Some(method), None) => {
            events.send_blocking(Incoming::Notification { method, params }).ok();
        }
        (None, Some(id)) => {
            let Some(id) = id.as_i64() else { return };
            let Some(tx) = pending.lock().remove(&id) else { return };
            let result = match message.get("error") {
                Some(error) if !error.is_null() => Err(RpcError::from_value(error)),
                _ => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            tx.send(result).ok();
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell script standing in for an agent: answers `initialize`,
    /// then asks the client for a file and echoes what it got back.
    fn script_agent(dir: &Path) -> Launch {
        let script = dir.join("agent.sh");
        std::fs::write(
            &script,
            r#"read -r line
echo '{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":1}}'
echo 'not json, ignored'
echo '{"jsonrpc":"2.0","id":"r1","method":"fs/read_text_file","params":{"path":"/tmp/x"}}'
read -r answer
echo "{\"jsonrpc\":\"2.0\",\"method\":\"echo\",\"params\":$answer}"
read -r line
echo '{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"Authentication required"}}'
echo 'bye' >&2
"#,
        )
        .unwrap();
        Launch { program: "/bin/sh".into(), args: vec![script.to_string_lossy().into()], env: vec![] }
    }

    #[test]
    fn requests_both_ways_errors_and_exit() {
        let dir = std::env::temp_dir().join(format!("zcode-acp-conn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = channel::unbounded();
        let connection = AgentConnection::spawn(&script_agent(&dir), &dir, tx).unwrap();
        smol::block_on(async {
            let init = connection.request("initialize", json!({ "protocolVersion": 1 })).await.unwrap();
            assert_eq!(init["protocolVersion"], 1);
            let Incoming::Request { id, method, params } = rx.recv().await.unwrap() else { panic!("expected a request") };
            assert_eq!((id.clone(), method.as_str(), params["path"].as_str()), (json!("r1"), "fs/read_text_file", Some("/tmp/x")));
            connection.respond(id, Ok(json!({ "content": "hello" })));
            let Incoming::Notification { method, params } = rx.recv().await.unwrap() else { panic!("expected the echo") };
            assert_eq!(method, "echo");
            assert_eq!(params["id"], "r1");
            assert_eq!(params["result"]["content"], "hello");
            let err = connection.request("session/new", json!({})).await.unwrap_err();
            assert_eq!(err.code, RpcError::AUTH_REQUIRED);
            let Incoming::Exited { stderr } = rx.recv().await.unwrap() else { panic!("expected the exit") };
            assert_eq!(stderr, "bye");
            assert!(!connection.is_alive());
            assert!(connection.request("session/prompt", json!({})).await.is_err());
        });
        std::fs::remove_dir_all(&dir).ok();
    }
}

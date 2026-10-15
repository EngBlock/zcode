//! The debugger model: breakpoints plus one lldb-dap session at a time.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::{AppContext as _, Context, EventEmitter, Task};
use serde_json::{Value, json};
use smol::channel;

use super::dap::{DapClient, DapEvent, initialize_args};

#[derive(Clone, Debug, PartialEq)]
pub enum DebugState {
    Inactive,
    Starting,
    Running,
    Stopped { thread_id: i64, reason: String, description: Option<String> },
    Exited,
}

impl DebugState {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopped { .. })
    }
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    pub path: Option<PathBuf>,
    /// 1-based.
    pub line: u32,
}

#[derive(Clone, Debug)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub ty: Option<String>,
    pub reference: i64,
    pub depth: usize,
    pub expanded: bool,
}

pub enum DebuggerEvent {
    StateChanged,
    BreakpointsChanged(PathBuf),
    Output(String),
    /// Execution stopped in a frame with source; reveal it.
    Stopped { path: PathBuf, line: u32 },
    /// The session ended (debuggee exited or was detached).
    Ended { exit_code: Option<i64> },
}

pub enum Target {
    Launch { program: PathBuf, cwd: PathBuf, args: Vec<String> },
    Attach { pid: i32 },
}

pub struct Debugger {
    /// 1-based lines with breakpoints, per file.
    pub breakpoints: BTreeMap<PathBuf, BTreeSet<u32>>,
    pub breakpoints_active: bool,
    pub state: DebugState,
    pub threads: Vec<(i64, String)>,
    pub frames: Vec<Frame>,
    pub selected_frame: usize,
    pub variables: Vec<Variable>,
    client: Option<Arc<DapClient>>,
    exit_code: Option<i64>,
    _event_task: Option<Task<()>>,
    _fetch_task: Option<Task<()>>,
}

impl EventEmitter<DebuggerEvent> for Debugger {}

impl Default for Debugger {
    fn default() -> Self {
        Self {
            breakpoints: BTreeMap::new(),
            breakpoints_active: true,
            state: DebugState::Inactive,
            threads: vec![],
            frames: vec![],
            selected_frame: 0,
            variables: vec![],
            client: None,
            exit_code: None,
            _event_task: None,
            _fetch_task: None,
        }
    }
}

impl Debugger {
    /// Whether ⌘R should run under the debugger (Xcode: breakpoints exist and
    /// are activated).
    pub fn wants_debugging(&self) -> bool {
        self.breakpoints_active && self.breakpoints.values().any(|lines| !lines.is_empty())
    }

    pub fn has_breakpoint(&self, path: &Path, line: u32) -> bool {
        self.breakpoints.get(path).is_some_and(|l| l.contains(&line))
    }

    pub fn toggle_breakpoint(&mut self, path: PathBuf, line: u32, cx: &mut Context<Self>) {
        let lines = self.breakpoints.entry(path.clone()).or_default();
        if !lines.remove(&line) {
            lines.insert(line);
        }
        if lines.is_empty() {
            self.breakpoints.remove(&path);
        }
        self.sync_breakpoints(&path, cx);
        cx.emit(DebuggerEvent::BreakpointsChanged(path));
        cx.notify();
    }

    pub fn toggle_breakpoints_active(&mut self, cx: &mut Context<Self>) {
        self.breakpoints_active = !self.breakpoints_active;
        let paths: Vec<_> = self.breakpoints.keys().cloned().collect();
        for path in paths {
            self.sync_breakpoints(&path, cx);
            cx.emit(DebuggerEvent::BreakpointsChanged(path));
        }
        cx.notify();
    }

    pub fn remove_all_breakpoints(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<_> = std::mem::take(&mut self.breakpoints).into_keys().collect();
        for path in paths {
            self.sync_breakpoints(&path, cx);
            cx.emit(DebuggerEvent::BreakpointsChanged(path));
        }
        cx.notify();
    }

    fn breakpoint_args(&self, path: &Path) -> Value {
        let lines: Vec<Value> = if self.breakpoints_active {
            self.breakpoints
                .get(path)
                .map(|l| l.iter().map(|line| json!({ "line": line })).collect())
                .unwrap_or_default()
        } else {
            vec![]
        };
        json!({ "source": { "path": path }, "breakpoints": lines })
    }

    fn sync_breakpoints(&self, path: &Path, cx: &mut Context<Self>) {
        if let Some(client) = &self.client
            && self.state.is_active()
        {
            let request = client.request("setBreakpoints", self.breakpoint_args(path));
            cx.background_spawn(async move {
                request.await.ok();
            })
            .detach();
        }
    }

    pub fn start(&mut self, target: Target, cx: &mut Context<Self>) {
        self.stop(cx);
        self.state = DebugState::Starting;
        self.exit_code = None;
        self.frames.clear();
        self.variables.clear();
        cx.emit(DebuggerEvent::StateChanged);
        cx.notify();

        let (tx, rx) = channel::unbounded();
        let client = match crate::toolchain::find_tool("lldb-dap")
            .and_then(|binary| DapClient::spawn(&binary, tx))
        {
            Ok(client) => client,
            Err(err) => {
                cx.emit(DebuggerEvent::Output(format!("Could not start lldb-dap: {err:#}")));
                self.finish(None, cx);
                return;
            }
        };
        self.client = Some(client.clone());

        let (command, args) = match target {
            Target::Launch { program, cwd, args } => (
                "launch",
                json!({ "program": program, "cwd": cwd, "args": args, "stopOnEntry": false }),
            ),
            Target::Attach { pid } => ("attach", json!({ "pid": pid })),
        };
        let initialize = client.request("initialize", initialize_args());
        let session = client.clone();
        cx.spawn(async move |this, cx| {
            if let Err(err) = initialize.await {
                this.update(cx, |this, cx| {
                    cx.emit(DebuggerEvent::Output(format!("lldb-dap failed to initialize: {err:#}")));
                    this.finish(None, cx);
                })
                .ok();
                return;
            }
            // The launch/attach response arrives after configurationDone.
            let started = session.request(command, args).await;
            this.update(cx, |this, cx| match started {
                Ok(_) => {
                    if this.state == DebugState::Starting {
                        this.state = DebugState::Running;
                        cx.emit(DebuggerEvent::StateChanged);
                        cx.notify();
                    }
                }
                Err(err) => {
                    cx.emit(DebuggerEvent::Output(format!("Debugger {command} failed: {err:#}")));
                    this.finish(None, cx);
                }
            })
            .ok();
        })
        .detach();

        self._event_task = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                if this.update(cx, |this, cx| this.handle_event(event, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn handle_event(&mut self, event: DapEvent, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else { return };
        match event {
            DapEvent::Event { event, body } => match event.as_str() {
                "initialized" => {
                    let requests: Vec<_> = self
                        .breakpoints
                        .keys()
                        .map(|path| client.request("setBreakpoints", self.breakpoint_args(path)))
                        .collect();
                    let done = client.clone();
                    cx.background_spawn(async move {
                        for request in requests {
                            request.await.ok();
                        }
                        done.request("configurationDone", json!({})).await.ok();
                    })
                    .detach();
                }
                "stopped" => {
                    let thread_id = body
                        .get("threadId")
                        .and_then(Value::as_i64)
                        .or_else(|| self.threads.first().map(|t| t.0))
                        .unwrap_or(1);
                    self.state = DebugState::Stopped {
                        thread_id,
                        reason: body.get("reason").and_then(Value::as_str).unwrap_or("pause").to_string(),
                        description: body.get("description").and_then(Value::as_str).map(str::to_string),
                    };
                    cx.emit(DebuggerEvent::StateChanged);
                    self.fetch_stack(thread_id, cx);
                }
                "continued" => {
                    self.state = DebugState::Running;
                    self.frames.clear();
                    self.variables.clear();
                    cx.emit(DebuggerEvent::StateChanged);
                }
                "output" => {
                    let category = body.get("category").and_then(Value::as_str).unwrap_or("console");
                    if category != "telemetry"
                        && let Some(text) = body.get("output").and_then(Value::as_str)
                    {
                        for line in text.lines() {
                            cx.emit(DebuggerEvent::Output(line.to_string()));
                        }
                    }
                }
                "exited" => {
                    self.exit_code = body.get("exitCode").and_then(Value::as_i64);
                }
                "terminated" => {
                    let code = self.exit_code;
                    self.finish(code, cx);
                }
                _ => {}
            },
            DapEvent::Request { seq, command, .. } => {
                client.respond(seq, &command, json!({}));
            }
            DapEvent::Exited => {
                if self.state.is_active() {
                    let code = self.exit_code;
                    self.finish(code, cx);
                }
            }
        }
        cx.notify();
    }

    fn finish(&mut self, exit_code: Option<i64>, cx: &mut Context<Self>) {
        if let Some(client) = self.client.take() {
            let disconnect = client.request("disconnect", json!({ "terminateDebuggee": true }));
            cx.background_spawn(async move {
                let timeout = smol::Timer::after(std::time::Duration::from_millis(800));
                futures::future::select(Box::pin(disconnect), timeout).await;
                client.kill();
            })
            .detach();
        }
        let was_active = self.state != DebugState::Inactive && self.state != DebugState::Exited;
        self.state = DebugState::Exited;
        self.frames.clear();
        self.variables.clear();
        self.threads.clear();
        cx.emit(DebuggerEvent::StateChanged);
        if was_active {
            cx.emit(DebuggerEvent::Ended { exit_code });
        }
        cx.notify();
    }

    /// Stops the session and terminates the debuggee.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if self.client.is_some() {
            self.finish(None, cx);
        }
    }

    fn thread_id(&self) -> Option<i64> {
        match &self.state {
            DebugState::Stopped { thread_id, .. } => Some(*thread_id),
            DebugState::Running => self.threads.first().map(|t| t.0).or(Some(1)),
            _ => None,
        }
    }

    fn send(&mut self, command: &str, cx: &mut Context<Self>) {
        let (Some(client), Some(thread_id)) = (self.client.clone(), self.thread_id()) else {
            return;
        };
        let request = client.request(command, json!({ "threadId": thread_id }));
        cx.background_spawn(async move {
            request.await.ok();
        })
        .detach();
    }

    /// Continue when stopped, pause when running (Xcode's ⌃⌘Y).
    pub fn continue_or_pause(&mut self, cx: &mut Context<Self>) {
        match self.state {
            DebugState::Stopped { .. } => self.send("continue", cx),
            DebugState::Running => self.send("pause", cx),
            _ => {}
        }
    }

    pub fn step_over(&mut self, cx: &mut Context<Self>) {
        if matches!(self.state, DebugState::Stopped { .. }) {
            self.send("next", cx);
        }
    }

    pub fn step_into(&mut self, cx: &mut Context<Self>) {
        if matches!(self.state, DebugState::Stopped { .. }) {
            self.send("stepIn", cx);
        }
    }

    pub fn step_out(&mut self, cx: &mut Context<Self>) {
        if matches!(self.state, DebugState::Stopped { .. }) {
            self.send("stepOut", cx);
        }
    }

    fn fetch_stack(&mut self, thread_id: i64, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else { return };
        self._fetch_task = Some(cx.spawn(async move |this, cx| {
            let threads = client.request("threads", json!({})).await.unwrap_or(Value::Null);
            let stack = client
                .request("stackTrace", json!({ "threadId": thread_id, "levels": 64 }))
                .await
                .unwrap_or(Value::Null);
            let frames: Vec<Frame> = stack
                .get("stackFrames")
                .and_then(Value::as_array)
                .map(|frames| {
                    frames
                        .iter()
                        .map(|f| Frame {
                            id: f.get("id").and_then(Value::as_i64).unwrap_or(0),
                            name: f.get("name").and_then(Value::as_str).unwrap_or("?").to_string(),
                            path: f.pointer("/source/path").and_then(Value::as_str).map(PathBuf::from),
                            line: f.get("line").and_then(Value::as_u64).unwrap_or(0) as u32,
                        })
                        .collect()
                })
                .unwrap_or_default();
            // Select the first frame with source, like Xcode.
            let selected = frames
                .iter()
                .position(|f| f.path.as_ref().is_some_and(|p| p.exists()))
                .unwrap_or(0);
            this.update(cx, |this, cx| {
                this.threads = threads
                    .get("threads")
                    .and_then(Value::as_array)
                    .map(|ts| {
                        ts.iter()
                            .map(|t| {
                                (
                                    t.get("id").and_then(Value::as_i64).unwrap_or(0),
                                    t.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                this.frames = frames;
                this.select_frame(selected, cx);
            })
            .ok();
        }));
    }

    pub fn select_frame(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(frame) = self.frames.get(index).cloned() else {
            cx.notify();
            return;
        };
        self.selected_frame = index;
        if let Some(path) = frame.path.clone().filter(|p| p.exists()) {
            cx.emit(DebuggerEvent::Stopped { path, line: frame.line });
        }
        let Some(client) = self.client.clone() else { return };
        self._fetch_task = Some(cx.spawn(async move |this, cx| {
            let scopes = client
                .request("scopes", json!({ "frameId": frame.id }))
                .await
                .unwrap_or(Value::Null);
            let mut variables = vec![];
            for scope in scopes.get("scopes").and_then(Value::as_array).cloned().unwrap_or_default() {
                let name = scope.get("name").and_then(Value::as_str).unwrap_or("");
                // Locals first; registers are rarely wanted.
                if name.eq_ignore_ascii_case("registers") {
                    continue;
                }
                let reference = scope.get("variablesReference").and_then(Value::as_i64).unwrap_or(0);
                let vars = client
                    .request("variables", json!({ "variablesReference": reference }))
                    .await
                    .unwrap_or(Value::Null);
                variables.extend(parse_variables(&vars, 0));
            }
            this.update(cx, |this, cx| {
                this.variables = variables;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Expands or collapses a structured variable in the Variables view.
    pub fn toggle_variable(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(var) = self.variables.get(index).cloned() else { return };
        if var.reference == 0 {
            return;
        }
        if var.expanded {
            let end = self.variables[index + 1..]
                .iter()
                .position(|v| v.depth <= var.depth)
                .map(|p| index + 1 + p)
                .unwrap_or(self.variables.len());
            self.variables.drain(index + 1..end);
            self.variables[index].expanded = false;
            cx.notify();
            return;
        }
        let Some(client) = self.client.clone() else { return };
        self._fetch_task = Some(cx.spawn(async move |this, cx| {
            let vars = client
                .request("variables", json!({ "variablesReference": var.reference }))
                .await
                .unwrap_or(Value::Null);
            let children = parse_variables(&vars, var.depth + 1);
            this.update(cx, |this, cx| {
                if this.variables.get(index).is_some_and(|v| v.reference == var.reference) {
                    this.variables[index].expanded = true;
                    this.variables.splice(index + 1..index + 1, children);
                    cx.notify();
                }
            })
            .ok();
        }));
    }
}

fn parse_variables(body: &Value, depth: usize) -> Vec<Variable> {
    body.get("variables")
        .and_then(Value::as_array)
        .map(|vars| {
            vars.iter()
                .take(500)
                .map(|v| Variable {
                    name: v.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    value: v.get("value").and_then(Value::as_str).unwrap_or("").to_string(),
                    ty: v.get("type").and_then(Value::as_str).map(str::to_string),
                    reference: v.get("variablesReference").and_then(Value::as_i64).unwrap_or(0),
                    depth,
                    expanded: false,
                })
                .collect()
        })
        .unwrap_or_default()
}

//! The debugger model: breakpoints plus one lldb-dap session at a time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use gpui_kit::{AppContext as _, Context, EventEmitter, Task};
use serde_json::{Value, json};
use smol::channel;

use super::breakpoints::{self, Action, Breakpoint, Kind, LogPart};
use super::dap::{DapClient, DapEvent, initialize_args};

/// lldb-dap's prefix that makes `evaluate` run an lldb command, as Xcode's
/// console does, rather than guess between a command and an expression.
const COMMAND_PREFIX: &str = "`";

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
    /// An expression for the variable (`self.name`), from lldb-dap.
    pub evaluate_name: Option<String>,
    pub reference: i64,
    pub depth: usize,
    pub expanded: bool,
}

pub enum DebuggerEvent {
    StateChanged,
    BreakpointsChanged,
    Output(String),
    /// Execution stopped in a frame with source; reveal it.
    Stopped { path: PathBuf, line: u32 },
    /// The session ended (debuggee exited or was detached).
    Ended { exit_code: Option<i64> },
}

pub enum Target {
    Launch { program: PathBuf, cwd: PathBuf, args: Vec<String>, env: Vec<(String, String)> },
    Attach { pid: i32 },
}

/// Which breakpoints a sync sends to lldb-dap.
#[derive(Clone, Debug, PartialEq)]
enum Scope {
    File(PathBuf),
    Exceptions,
    Symbolic(u64),
}

/// What a sync request's response tells about lldb's breakpoint ids.
enum Mapping {
    /// `setBreakpoints`: lldb's ids in the order of these breakpoints.
    Lines(Vec<u64>),
    /// `breakpoint set`: the id in its output.
    Created(u64),
    None,
}

pub struct Debugger {
    /// In the order they were added; the Breakpoint navigator groups them.
    pub breakpoints: Vec<Breakpoint>,
    pub breakpoints_active: bool,
    next_breakpoint_id: u64,
    /// This session's lldb breakpoint ids → ours.
    lldb_ids: HashMap<i64, u64>,
    /// This session's exception filters → the breakpoint that set each.
    exception_owners: HashMap<&'static str, u64>,
    /// Hits per breakpoint this session (`%H`), ignored ones included.
    hits: HashMap<u64, u32>,
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
            breakpoints: vec![],
            breakpoints_active: true,
            next_breakpoint_id: 1,
            lldb_ids: HashMap::new(),
            exception_owners: HashMap::new(),
            hits: HashMap::new(),
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
        self.breakpoints_active && self.breakpoints.iter().any(|bp| bp.enabled)
    }

    pub fn breakpoint(&self, id: u64) -> Option<&Breakpoint> {
        self.breakpoints.iter().find(|bp| bp.id == id)
    }

    pub fn breakpoint_at(&self, path: &Path, line: u32) -> Option<&Breakpoint> {
        self.breakpoints.iter().find(|bp| bp.is_at(path, line))
    }

    pub fn has_breakpoint(&self, path: &Path, line: u32) -> bool {
        self.breakpoint_at(path, line).is_some()
    }

    /// The line breakpoints in `path`.
    pub fn line_breakpoints<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a Breakpoint> + 'a {
        self.breakpoints.iter().filter(move |bp| bp.path() == Some(path))
    }

    /// Replaces every breakpoint (restoring a session), without syncing.
    pub fn restore_breakpoints(&mut self, breakpoints: Vec<Breakpoint>) {
        self.breakpoints.clear();
        for mut bp in breakpoints {
            bp.id = self.next_breakpoint_id;
            self.next_breakpoint_id += 1;
            self.breakpoints.push(bp);
        }
    }

    /// Adds a breakpoint and returns its id.
    pub fn add_breakpoint(&mut self, mut bp: Breakpoint, cx: &mut Context<Self>) -> u64 {
        bp.id = self.next_breakpoint_id;
        self.next_breakpoint_id += 1;
        let id = bp.id;
        let scope = Self::scope(&bp);
        self.breakpoints.push(bp);
        self.changed(vec![scope], cx);
        id
    }

    /// Adds a breakpoint on a line, or removes the one there (⌘\).
    pub fn toggle_breakpoint(&mut self, path: PathBuf, line: u32, cx: &mut Context<Self>) {
        match self.breakpoint_at(&path, line).map(|bp| bp.id) {
            Some(id) => self.remove_breakpoint(id, cx),
            None => {
                self.add_breakpoint(Breakpoint::new(Kind::Line { path, line }), cx);
            }
        }
    }

    pub fn update_breakpoint(&mut self, id: u64, edit: impl FnOnce(&mut Breakpoint), cx: &mut Context<Self>) {
        let Some(bp) = self.breakpoints.iter_mut().find(|bp| bp.id == id) else { return };
        let before = Self::scope(bp);
        edit(bp);
        bp.id = id;
        let after = Self::scope(bp);
        self.changed(vec![before, after], cx);
    }

    pub fn set_breakpoint_enabled(&mut self, id: u64, enabled: bool, cx: &mut Context<Self>) {
        self.update_breakpoint(id, |bp| bp.enabled = enabled, cx);
    }

    pub fn remove_breakpoint(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.breakpoints.iter().position(|bp| bp.id == id) else { return };
        let bp = self.breakpoints.remove(ix);
        self.changed(vec![Self::scope(&bp)], cx);
    }

    /// Moves line breakpoints after an edit in `path` (`None`: the line was
    /// deleted, and the breakpoint with it).
    pub fn move_line_breakpoints(&mut self, path: &Path, moves: Vec<(u64, Option<u32>)>, cx: &mut Context<Self>) {
        for (id, line) in moves {
            match line {
                Some(line) => {
                    if let Some(Kind::Line { line: current, .. }) =
                        self.breakpoints.iter_mut().find(|bp| bp.id == id).map(|bp| &mut bp.kind)
                    {
                        *current = line;
                    }
                }
                None => self.breakpoints.retain(|bp| bp.id != id),
            }
        }
        // Lines joined by the edit keep one breakpoint, the first.
        let mut seen = std::collections::HashSet::new();
        self.breakpoints
            .retain(|bp| bp.path() != Some(path) || seen.insert(bp.line()));
        self.changed(vec![Scope::File(path.to_path_buf())], cx);
    }

    /// Keeps line breakpoints with a file or folder that was renamed or moved.
    pub fn move_files(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        let mut scopes = vec![];
        for bp in &mut self.breakpoints {
            if let Kind::Line { path, .. } = &mut bp.kind
                && let Ok(rest) = path.strip_prefix(from)
            {
                scopes.push(Scope::File(path.clone()));
                *path = if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) };
                scopes.push(Scope::File(path.clone()));
            }
        }
        if !scopes.is_empty() {
            self.changed(scopes, cx);
        }
    }

    /// Deletes the line breakpoints in a file or folder that was deleted.
    pub fn remove_breakpoints_in(&mut self, folder: &Path, cx: &mut Context<Self>) {
        let ids: Vec<u64> = self
            .breakpoints
            .iter()
            .filter(|bp| bp.path().is_some_and(|p| p.starts_with(folder)))
            .map(|bp| bp.id)
            .collect();
        for id in ids {
            self.remove_breakpoint(id, cx);
        }
    }

    pub fn toggle_breakpoints_active(&mut self, cx: &mut Context<Self>) {
        self.breakpoints_active = !self.breakpoints_active;
        let scopes = self.breakpoints.iter().map(Self::scope).collect();
        self.changed(scopes, cx);
    }

    pub fn remove_all_breakpoints(&mut self, cx: &mut Context<Self>) {
        let scopes = std::mem::take(&mut self.breakpoints).iter().map(Self::scope).collect();
        self.changed(scopes, cx);
    }

    fn scope(bp: &Breakpoint) -> Scope {
        match &bp.kind {
            Kind::Line { path, .. } => Scope::File(path.clone()),
            Kind::Symbolic { .. } => Scope::Symbolic(bp.id),
            Kind::SwiftError | Kind::Exception { .. } => Scope::Exceptions,
        }
    }

    fn changed(&mut self, mut scopes: Vec<Scope>, cx: &mut Context<Self>) {
        scopes.dedup();
        let mut unique: Vec<Scope> = vec![];
        for scope in scopes {
            if !unique.contains(&scope) {
                unique.push(scope);
            }
        }
        if let Some(client) = self.client.clone()
            && self.state.is_active()
        {
            let requests = unique.iter().flat_map(|scope| self.sync_requests(&client, scope)).collect();
            self.run_sync(requests, None, cx);
        }
        cx.emit(DebuggerEvent::BreakpointsChanged);
        cx.notify();
    }

    /// The paths a file may appear under in debug info. xcodebuild records
    /// `/var/...`/`/tmp/...` where the editor has the real `/private/...`
    /// path, while SwiftPM records the real path; lldb matches either one.
    fn source_paths(path: &Path) -> Vec<PathBuf> {
        let mut paths = vec![path.to_path_buf()];
        if let Ok(rest) = path.strip_prefix("/private")
            && ["var", "tmp", "etc"].iter().any(|p| rest.starts_with(p))
        {
            paths.push(Path::new("/").join(rest));
        }
        paths
    }

    /// The requests that bring lldb's breakpoints for `scope` up to date.
    fn sync_requests(&mut self, client: &DapClient, scope: &Scope) -> Vec<(BoxFuture<'static, anyhow::Result<Value>>, Mapping)> {
        let active = self.breakpoints_active;
        match scope {
            Scope::File(path) => {
                let lines: Vec<&Breakpoint> = self
                    .line_breakpoints(path)
                    .filter(|bp| active && bp.enabled)
                    .collect();
                let ids: Vec<u64> = lines.iter().map(|bp| bp.id).collect();
                let args: Vec<Value> = lines.iter().filter_map(|bp| bp.source_breakpoint()).collect();
                Self::source_paths(path)
                    .into_iter()
                    .map(|source| {
                        let request = client.request("setBreakpoints", json!({ "source": { "path": source }, "breakpoints": args }));
                        (Box::pin(request) as BoxFuture<'static, _>, Mapping::Lines(ids.clone()))
                    })
                    .collect()
            }
            Scope::Exceptions => {
                self.exception_owners.clear();
                for bp in self.breakpoints.iter().filter(|bp| active && bp.enabled) {
                    for filter in bp.exception_filters() {
                        self.exception_owners.entry(filter).or_insert(bp.id);
                    }
                }
                let filters: Vec<&str> = self.exception_owners.keys().copied().collect();
                let request = client.request("setExceptionBreakpoints", json!({ "filters": filters }));
                vec![(Box::pin(request), Mapping::None)]
            }
            Scope::Symbolic(id) => {
                let delete = format!("{COMMAND_PREFIX}breakpoint delete {}", Breakpoint::lldb_name(*id));
                let mut requests = vec![(
                    Box::pin(client.request("evaluate", json!({ "expression": delete, "context": "repl" })))
                        as BoxFuture<'static, _>,
                    Mapping::None,
                )];
                if let Some(command) = self
                    .breakpoint(*id)
                    .filter(|bp| active && bp.enabled)
                    .and_then(|bp| bp.symbolic_command())
                {
                    let command = format!("{COMMAND_PREFIX}{command}");
                    requests.push((
                        Box::pin(client.request("evaluate", json!({ "expression": command, "context": "repl" }))),
                        Mapping::Created(*id),
                    ));
                }
                requests
            }
        }
    }

    /// Every breakpoint, as sent when a session starts.
    fn all_sync_requests(&mut self, client: &DapClient) -> Vec<(BoxFuture<'static, anyhow::Result<Value>>, Mapping)> {
        let mut scopes = vec![Scope::Exceptions];
        for bp in &self.breakpoints {
            let scope = Self::scope(bp);
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
        }
        scopes.iter().flat_map(|scope| self.sync_requests(client, scope)).collect()
    }

    /// Sends sync requests in order, noting lldb's ids, then `then`.
    fn run_sync(
        &mut self,
        requests: Vec<(BoxFuture<'static, anyhow::Result<Value>>, Mapping)>,
        then: Option<BoxFuture<'static, anyhow::Result<Value>>>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            for (request, mapping) in requests {
                let Ok(body) = request.await else { continue };
                let ids: Vec<(i64, u64)> = match mapping {
                    Mapping::Lines(ids) => body
                        .get("breakpoints")
                        .and_then(Value::as_array)
                        .map(|bps| bps.iter().map(|bp| bp.get("id").and_then(Value::as_i64).unwrap_or(-1)).zip(ids).collect())
                        .unwrap_or_default(),
                    Mapping::Created(id) => body
                        .get("result")
                        .and_then(Value::as_str)
                        .and_then(breakpoints::created_id)
                        .map(|lldb| vec![(lldb, id)])
                        .unwrap_or_default(),
                    Mapping::None => vec![],
                };
                this.update(cx, |this, _| this.lldb_ids.extend(ids)).ok();
            }
            if let Some(then) = then {
                then.await.ok();
            }
        })
        .detach();
    }

    pub fn start(&mut self, target: Target, cx: &mut Context<Self>) {
        self.stop(cx);
        self.state = DebugState::Starting;
        self.exit_code = None;
        self.frames.clear();
        self.variables.clear();
        self.lldb_ids.clear();
        self.exception_owners.clear();
        self.hits.clear();
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
            Target::Launch { program, cwd, args, env } => {
                let env: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
                ("launch", json!({ "program": program, "cwd": cwd, "args": args, "env": env, "stopOnEntry": false }))
            }
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
                    let requests = self.all_sync_requests(&client);
                    // configurationDone after every breakpoint is set.
                    let done = client.clone();
                    let then: BoxFuture<'static, _> = Box::pin(async move { done.request("configurationDone", json!({})).await });
                    self.run_sync(requests, Some(then), cx);
                }
                "stopped" => {
                    let thread_id = body
                        .get("threadId")
                        .and_then(Value::as_i64)
                        .or_else(|| self.threads.first().map(|t| t.0))
                        .unwrap_or(1);
                    let hit = self.hit_breakpoints(&body);
                    let mut actions = vec![];
                    for bp in hit.iter().filter_map(|&id| self.breakpoint(id)).cloned().collect::<Vec<_>>() {
                        let hits = self.hits.entry(bp.id).or_insert(bp.ignore_count);
                        *hits += 1;
                        if !bp.actions.is_empty() {
                            actions.push((bp, *hits));
                        }
                    }
                    // Xcode's "Automatically continue after evaluating
                    // actions": the stop is never shown.
                    let resume = !hit.is_empty()
                        && hit.iter().all(|&id| self.breakpoint(id).is_some_and(|bp| bp.auto_continue));
                    if !resume {
                        self.state = DebugState::Stopped {
                            thread_id,
                            reason: body.get("reason").and_then(Value::as_str).unwrap_or("pause").to_string(),
                            description: body.get("description").and_then(Value::as_str).map(str::to_string),
                        };
                        cx.emit(DebuggerEvent::StateChanged);
                        self.fetch_stack(thread_id, cx);
                    }
                    if !actions.is_empty() || resume {
                        self.run_actions(thread_id, actions, resume, cx);
                    }
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

    /// Our breakpoints that caused a stop.
    fn hit_breakpoints(&self, body: &Value) -> Vec<u64> {
        match body.get("reason").and_then(Value::as_str) {
            Some("breakpoint") => body
                .get("hitBreakpointIds")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|id| self.lldb_ids.get(&id.as_i64()?).copied())
                .collect(),
            // Exception stops name the filter, not a breakpoint.
            Some("exception") => body
                .get("description")
                .and_then(Value::as_str)
                .and_then(breakpoints::filter_for_stop)
                .and_then(|filter| self.exception_owners.get(filter).copied())
                .into_iter()
                .collect(),
            _ => vec![],
        }
    }

    /// Runs breakpoint actions in the stopped frame, then continues when
    /// `resume`.
    fn run_actions(&mut self, thread_id: i64, actions: Vec<(Breakpoint, u32)>, resume: bool, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else { return };
        cx.spawn(async move |this, cx| {
            let frame_id = client
                .request("stackTrace", json!({ "threadId": thread_id, "levels": 1 }))
                .await
                .ok()
                .and_then(|stack| stack.pointer("/stackFrames/0/id").and_then(Value::as_i64));
            let output = |text: String, cx: &mut gpui_kit::AsyncApp| {
                this.update(cx, |_, cx| {
                    for line in text.lines() {
                        cx.emit(DebuggerEvent::Output(line.to_string()));
                    }
                })
                .ok();
            };
            for (bp, hits) in actions {
                for action in &bp.actions {
                    match action {
                        Action::Log { message, speak } => {
                            let mut text = String::new();
                            for part in breakpoints::parse_log_message(message, &bp.display_name(), hits) {
                                match part {
                                    LogPart::Text(part) => text.push_str(&part),
                                    LogPart::Expression(expression) => {
                                        let value = client
                                            .request(
                                                "evaluate",
                                                json!({ "expression": expression, "context": "watch", "frameId": frame_id }),
                                            )
                                            .await;
                                        match value {
                                            Ok(value) => text.push_str(value.get("result").and_then(Value::as_str).unwrap_or("")),
                                            Err(err) => text.push_str(&format!("<{err}>")),
                                        }
                                    }
                                }
                            }
                            if *speak {
                                breakpoints::speak(text);
                            } else {
                                output(text, cx);
                            }
                        }
                        Action::Command { command } => {
                            let result = client
                                .request(
                                    "evaluate",
                                    json!({ "expression": format!("{COMMAND_PREFIX}{command}"), "context": "repl", "frameId": frame_id }),
                                )
                                .await;
                            output(command_output(result), cx);
                        }
                        Action::Sound { name } => {
                            breakpoints::play_sound(name);
                        }
                    }
                }
            }
            if resume {
                client.request("continue", json!({ "threadId": thread_id })).await.ok();
            }
        })
        .detach();
    }

    /// The client and selected frame while execution is paused.
    pub fn paused_frame(&self) -> Option<(Arc<DapClient>, Option<i64>)> {
        if !matches!(self.state, DebugState::Stopped { .. }) {
            return None;
        }
        Some((self.client.clone()?, self.frames.get(self.selected_frame).map(|f| f.id)))
    }

    /// Runs a line typed at the console's `(lldb)` prompt.
    pub fn run_console_command(&mut self, command: &str, cx: &mut Context<Self>) {
        cx.emit(DebuggerEvent::Output(format!("(lldb) {command}")));
        let Some((client, frame_id)) = self.paused_frame() else {
            cx.emit(DebuggerEvent::Output("The process is running; pause it to use the console.".into()));
            return;
        };
        let request = client.request(
            "evaluate",
            json!({ "expression": format!("{COMMAND_PREFIX}{command}"), "context": "repl", "frameId": frame_id }),
        );
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                for line in command_output(result).lines() {
                    cx.emit(DebuggerEvent::Output(line.to_string()));
                }
                // The command may have changed values (`expr i = 5`).
                if matches!(this.state, DebugState::Stopped { .. }) {
                    this.fetch_variables(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Prints a variable's description (`po`) to the console, as the
    /// Variables view's Print Description does.
    pub fn print_description(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(var) = self.variables.get(index) else { return };
        let expression = self.variable_expression(index).unwrap_or_else(|| var.name.clone());
        cx.emit(DebuggerEvent::Output(format!("Printing description of {expression}:")));
        let Some((client, frame_id)) = self.paused_frame() else { return };
        let request = client.request(
            "evaluate",
            json!({ "expression": format!("{COMMAND_PREFIX}po {expression}"), "context": "repl", "frameId": frame_id }),
        );
        cx.spawn(async move |this, cx| {
            let text = command_output(request.await);
            this.update(cx, |_, cx| {
                for line in text.lines() {
                    cx.emit(DebuggerEvent::Output(line.to_string()));
                }
            })
            .ok();
        })
        .detach();
    }

    /// An expression for a variable: lldb-dap's, else its path from the
    /// top-level variable (`self.items[0]`).
    pub fn variable_expression(&self, index: usize) -> Option<String> {
        let var = self.variables.get(index)?;
        if let Some(name) = var.evaluate_name.clone().filter(|n| !n.is_empty()) {
            return Some(name);
        }
        let mut path = var.name.clone();
        let mut depth = var.depth;
        for parent in self.variables[..index].iter().rev() {
            if depth == 0 {
                break;
            }
            if parent.depth == depth - 1 {
                path = if path.starts_with('[') { format!("{}{path}", parent.name) } else { format!("{}.{path}", parent.name) };
                depth -= 1;
            }
        }
        Some(path)
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
                            path: f
                                .pointer("/source/path")
                                .and_then(Value::as_str)
                                .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p))),
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
        self.fetch_variables(cx);
    }

    /// Reads the selected frame's variables into the Variables view.
    fn fetch_variables(&mut self, cx: &mut Context<Self>) {
        let Some(frame) = self.frames.get(self.selected_frame).cloned() else { return };
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
                    evaluate_name: v.get("evaluateName").and_then(Value::as_str).map(str::to_string),
                    reference: v.get("variablesReference").and_then(Value::as_i64).unwrap_or(0),
                    depth,
                    expanded: false,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What an lldb command printed, or its error.
fn command_output(result: anyhow::Result<Value>) -> String {
    match result {
        Ok(body) => body.get("result").and_then(Value::as_str).unwrap_or("").trim_end().to_string(),
        Err(err) => format!("{err:#}").trim_end().to_string(),
    }
}

//! The agent panel (View ▸ Show Coding Assistant, ⌘0): a conversation with
//! an external coding agent (Claude Code, Codex, Gemini CLI or one from
//! `agent_servers` in settings.json) through the Agent Client Protocol.
//!
//! Xcode 27 shows its Coding Assistant in the navigator area; zcode's sits
//! on the inspector's side, as Zed's agent panel does, so the project
//! navigator stays in view while an agent works and the conversation keeps
//! a width of its own. Nothing here exists until the panel is first shown,
//! and no agent starts until the first message is sent.
//!
//! The panel keeps the conversation (`acp::Thread`), the agent's process
//! (`acp::AgentConnection`) and its session; what the agent does to files
//! goes through the workspace's buffers (`files.rs`), @-mentions come from
//! a picker of the project's files and symbols (`mentions.rs`), and the
//! thread is drawn by `render.rs`.

mod files;
mod mentions;
mod render;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::input::{InputContent, InputEvent, TextareaState};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::*;
use serde_json::{Value, json};

use super::Workspace;
use crate::acp::schema::{
    ClientCapabilities, ContentBlock, EmbeddedResource, EmbeddedResourceResource, FileSystemCapabilities, Implementation, InitializeRequest,
    InitializeResponse, NewSessionRequest, NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse, ReadTextFileRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, ResourceLink, SelectedPermissionOutcome, SessionId,
    SessionNotification, StopReason, TextContent, TextResourceContents, WriteTextFileRequest,
};
use crate::acp::{AgentConnection, AgentServer, Change, Entry, Incoming, RpcError, Thread, ToolContent, UserMessage};

pub use mentions::MentionDelegate;

/// The workspace's side of the panel.
#[derive(Default)]
pub struct AgentState {
    pub panel: Option<Entity<AgentPanel>>,
    pub shown: bool,
    /// The mention picker is open: closing it goes back to the composer.
    pub(super) mention_open: bool,
    /// The session's agent and Follow, for a panel made later.
    pub(super) restored: Option<crate::session::AgentPanelState>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Starting,
    Generating,
    Error(String),
    /// The agent wants the user to sign in first: its ways to, as (id, name, description).
    AuthRequired(Vec<(String, String, Option<String>)>),
}

/// A running agent: its process, and the session the thread is in.
struct Session {
    connection: Arc<AgentConnection>,
    /// `None` until `session/new` answers (and after New Thread).
    id: Option<SessionId>,
    prompt_capabilities: PromptCapabilities,
    auth_methods: Vec<(String, String, Option<String>)>,
    _events: Task<()>,
}

/// A file the agent changed in this thread: what it was before.
#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: PathBuf,
    /// `None`: the agent created it.
    pub before: Option<String>,
    /// Lines added and removed, as of the agent's last change.
    pub added: usize,
    pub removed: usize,
}

/// What the user's message becomes for the agent.
#[derive(Clone, Debug)]
enum Part {
    Text(String),
    Mention { uri: String, name: String, text: Option<String> },
}

pub struct AgentPanel {
    workspace: WeakEntity<Workspace>,
    root: PathBuf,
    focus_handle: FocusHandle,
    pub agents: Vec<AgentServer>,
    pub agent: AgentServer,
    always_allow: bool,
    pub follow: bool,
    session: Option<Session>,
    pub thread: Thread,
    pub status: Status,
    pub composer: Entity<TextareaState>,
    scroller: Entity<MessageScrollerState>,
    /// The message waiting for the agent (to start, or to be signed in to).
    pending_prompt: Option<Vec<Part>>,
    pub changes: Vec<FileChange>,
    /// Headless buffers the agent's writes opened, let go with the thread.
    opened_buffers: Vec<PathBuf>,
    /// Thoughts and tool calls shown in full.
    expanded: HashSet<usize>,
    /// Tool calls' diffs as drawn, by entry and content index.
    diff_cache: HashMap<(usize, usize), std::rc::Rc<Vec<render::DiffLine>>>,
    last_stop: Option<StopReason>,
    turn: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for AgentPanel {}

impl Focusable for AgentPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.read(cx).focus_handle(cx)
    }
}

impl AgentPanel {
    pub fn new(workspace: WeakEntity<Workspace>, root: PathBuf, settings: &crate::settings::Settings, restored: Option<&crate::session::AgentPanelState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let agents = settings.agent_servers.clone();
        let wanted = restored.and_then(|r| r.agent.clone()).unwrap_or_else(|| settings.default_agent.clone());
        let agent = agents.iter().find(|a| a.id == wanted).or(agents.first()).cloned().unwrap_or_else(|| crate::acp::agents::builtins().remove(0));
        let composer = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 12).submit_on_enter(true).placeholder(placeholder(&agent)));
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let subscriptions = vec![cx.subscribe_in(&composer, window, |this, composer, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
            InputEvent::Change => {
                // "@" typed at a word's start asks what to mention.
                let state = composer.read(cx);
                let cursor = state.cursor();
                let text = state.value();
                if cursor > 0 && text.get(cursor - 1..cursor) == Some("@") && text[..cursor - 1].chars().last().is_none_or(char::is_whitespace) {
                    let at = cursor - 1;
                    let panel = cx.entity().downgrade();
                    window.defer(cx, move |window, cx| {
                        panel.update(cx, |this, cx| this.show_mentions(Some(at), window, cx)).ok();
                    });
                }
            }
            _ => {}
        })];
        Self {
            workspace,
            root,
            focus_handle: cx.focus_handle(),
            agents,
            agent,
            always_allow: settings.always_allow_tool_actions,
            follow: restored.is_some_and(|r| r.follow),
            session: None,
            thread: Thread::default(),
            status: Status::Idle,
            composer,
            scroller,
            pending_prompt: None,
            changes: vec![],
            opened_buffers: vec![],
            expanded: HashSet::new(),
            diff_cache: HashMap::new(),
            last_stop: None,
            turn: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self.status, Status::Starting | Status::Generating)
    }

    pub fn running(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.connection.is_alive())
    }

    pub fn agent_pid(&self) -> Option<i32> {
        self.session.as_ref().filter(|s| s.connection.is_alive()).map(|s| s.connection.pid())
    }

    pub fn session_state(&self) -> crate::session::AgentPanelState {
        crate::session::AgentPanelState { agent: Some(self.agent.id.clone()), follow: self.follow }
    }

    /// The settings changed: the agents, and whether to ask before tools run.
    pub fn set_settings(&mut self, settings: &crate::settings::Settings, cx: &mut Context<Self>) {
        self.always_allow = settings.always_allow_tool_actions;
        if self.agents != settings.agent_servers {
            self.agents = settings.agent_servers.clone();
            // A changed command takes effect with the next agent started.
            if let Some(agent) = self.agents.iter().find(|a| a.id == self.agent.id) {
                self.agent = agent.clone();
            }
        }
        cx.notify();
    }

    pub fn set_follow(&mut self, follow: bool, cx: &mut Context<Self>) {
        self.follow = follow;
        if let Some(workspace) = self.workspace.upgrade() {
            cx.defer(move |cx| workspace.update(cx, |ws, cx| ws.save_session(cx)));
        }
        cx.notify();
    }

    /// Another agent: the thread starts over with it.
    pub fn select_agent(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.agents.iter().find(|a| a.id == id).cloned() else { return };
        if agent == self.agent && self.session.is_none() {
            return;
        }
        self.agent = agent;
        self.stop_agent();
        self.new_thread(window, cx);
        let placeholder = placeholder(&self.agent);
        self.composer.update(cx, |state, cx| state.set_placeholder(placeholder, window, cx));
        if let Some(workspace) = self.workspace.upgrade() {
            cx.defer(move |cx| workspace.update(cx, |ws, cx| ws.save_session(cx)));
        }
    }

    fn stop_agent(&mut self) {
        if let Some(session) = self.session.take() {
            session.connection.kill();
        }
        self.turn = None;
    }

    /// New Thread: the conversation, its changes and the buffers it opened
    /// are let go; the agent (if running) starts a new session next time.
    pub fn new_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            self.cancel(cx);
        }
        self.turn = None;
        if let Some(session) = &mut self.session {
            session.id = None;
        }
        self.thread = Thread::default();
        self.status = Status::Idle;
        self.pending_prompt = None;
        self.changes.clear();
        self.expanded.clear();
        self.diff_cache.clear();
        self.last_stop = None;
        let opened = std::mem::take(&mut self.opened_buffers);
        if let Some(workspace) = self.workspace.upgrade() {
            cx.defer(move |cx| workspace.update(cx, |ws, cx| ws.release_agent_buffers(&opened, cx)));
        }
        self.scroller.update(cx, |scroller, cx| scroller.reset(0, cx));
        self.composer.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // Sending

    /// Sends the composer's message, starting the agent first if need be.
    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() || matches!(self.status, Status::AuthRequired(_)) {
            return;
        }
        let content = self.composer.read(cx).content();
        let text = content.text().to_string();
        if text.trim().is_empty() {
            return;
        }
        let parts = self.parts(&content);
        let mentions = content.tokens().iter().map(|span| span.token().label().to_string()).collect();
        self.composer.update(cx, |state, cx| state.set_value("", window, cx));
        let change = self.thread.push_user(UserMessage { text: text.trim().to_string(), mentions });
        self.list_changed(change, cx);
        self.scroller.update(cx, |scroller, cx| scroller.scroll_to_end(cx));
        self.pending_prompt = Some(parts);
        self.run_pending(window, cx);
    }

    /// The message as text and mentions (whose text `read_mentions` fills in).
    fn parts(&self, content: &InputContent) -> Vec<Part> {
        let text = content.text();
        let mut parts = vec![];
        let mut at = 0;
        for span in content.tokens() {
            let range = span.range();
            if range.start > at {
                parts.push(Part::Text(text[at..range.start].to_string()));
            }
            let uri = span.token().id().to_string();
            parts.push(Part::Mention { uri, name: span.token().label().trim_start_matches('@').to_string(), text: None });
            at = range.end;
        }
        if at < text.len() {
            parts.push(Part::Text(text[at..].to_string()));
        }
        parts
    }

    /// The mentioned files' text from their buffers (unsaved edits
    /// included). Not while the workspace is being updated: `send` can run
    /// inside its update (the control channel), the prompt task never does.
    fn read_mentions(&self, parts: &[Part], cx: &App) -> Vec<Part> {
        let workspace = self.workspace.upgrade();
        parts
            .iter()
            .map(|part| match part {
                Part::Mention { uri, name, text: None } => {
                    let (path, lines) = mentions::parse_uri(uri);
                    let text = workspace.as_ref().and_then(|ws| ws.read(cx).mention_text(&path, lines, cx));
                    Part::Mention { uri: uri.clone(), name: name.clone(), text }
                }
                part => part.clone(),
            })
            .collect()
    }

    /// Runs the waiting message: starts the agent and its session if
    /// there isn't one, then prompts it.
    fn run_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parts) = self.pending_prompt.clone() else { return };
        let ready = self.session.as_ref().filter(|s| s.connection.is_alive()).map(|s| (s.connection.clone(), s.id.clone()));
        self.status = if ready.as_ref().is_some_and(|(_, id)| id.is_some()) { Status::Generating } else { Status::Starting };
        cx.notify();
        let agent = self.agent.clone();
        let root = self.root.clone();
        self.turn = Some(cx.spawn_in(window, async move |this, cx| {
            let connection = match ready {
                Some((connection, _)) => connection,
                None => {
                    let launch = match cx.background_spawn({
                        let agent = agent.clone();
                        async move { agent.launch() }
                    })
                    .await
                    {
                        Ok(launch) => launch,
                        Err(message) => return this.update(cx, |this, cx| this.fail(message, cx)).unwrap_or(()),
                    };
                    let Ok(started) = this.update_in(cx, |this, window, cx| this.start_agent(&launch, window, cx)) else { return };
                    let connection = match started {
                        Ok(connection) => connection,
                        Err(message) => return this.update(cx, |this, cx| this.fail(message, cx)).unwrap_or(()),
                    };
                    let request = InitializeRequest::new(agent_client_protocol_schema::ProtocolVersion::V1)
                        .client_capabilities(ClientCapabilities::new().fs(FileSystemCapabilities::new().read_text_file(true).write_text_file(true)).terminal(false))
                        .client_info(Implementation::new("zcode", env!("CARGO_PKG_VERSION")).title("zcode".to_string()));
                    match connection.request_as::<InitializeResponse, _>("initialize", request).await {
                        Ok(response) => this
                            .update(cx, |this, _| {
                                if let Some(session) = &mut this.session {
                                    session.prompt_capabilities = response.agent_capabilities.prompt_capabilities.clone();
                                    session.auth_methods = auth_methods(&response);
                                }
                            })
                            .ok(),
                        Err(err) => return this.update(cx, |this, cx| this.fail(format!("{} didn’t start: {err}", agent.name), cx)).unwrap_or(()),
                    };
                    connection
                }
            };
            // A session for the thread.
            let has_session = this.update(cx, |this, _| this.session.as_ref().is_some_and(|s| s.id.is_some())).unwrap_or(false);
            if !has_session {
                match connection.request_as::<NewSessionResponse, _>("session/new", NewSessionRequest::new(root.clone())).await {
                    Ok(response) => {
                        let ok = this.update(cx, |this, cx| {
                            if let Some(session) = &mut this.session {
                                session.id = Some(response.session_id.clone());
                            }
                            this.thread.modes = response.modes.clone();
                            cx.notify();
                        });
                        if ok.is_err() {
                            return;
                        }
                    }
                    Err(err) if err.code == RpcError::AUTH_REQUIRED => {
                        this.update(cx, |this, cx| {
                            let methods = this.session.as_ref().map(|s| s.auth_methods.clone()).unwrap_or_default();
                            this.status = Status::AuthRequired(methods);
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                    Err(err) => return this.update(cx, |this, cx| this.fail(format!("{} couldn’t start a session: {err}", agent.name), cx)).unwrap_or(()),
                }
            }
            let Ok(Some((session_id, blocks))) = this.update(cx, |this, cx| {
                let session = this.session.as_ref()?;
                let blocks = prompt_blocks(&this.read_mentions(&parts, cx), &session.prompt_capabilities);
                this.pending_prompt = None;
                this.status = Status::Generating;
                cx.notify();
                Some((session.id.clone()?, blocks))
            }) else {
                return;
            };
            let result = connection.request_as::<PromptResponse, _>("session/prompt", PromptRequest::new(session_id, blocks)).await;
            this.update(cx, |this, cx| this.turn_ended(result, cx)).ok();
        }));
    }

    /// Starts the agent's process and the task that handles what it sends.
    fn start_agent(&mut self, launch: &crate::acp::Launch, window: &mut Window, cx: &mut Context<Self>) -> Result<Arc<AgentConnection>, String> {
        let (tx, rx) = smol::channel::unbounded();
        let connection = AgentConnection::spawn(launch, &self.root, tx).map_err(|err| format!("{err:#}"))?;
        let id = Arc::as_ptr(&connection) as usize;
        let events = cx.spawn_in(window, async move |this, cx| {
            while let Ok(incoming) = rx.recv().await {
                let handled = this.update_in(cx, |this, window, cx| this.handle_incoming(id, incoming, window, cx));
                if handled.is_err() {
                    break;
                }
            }
        });
        self.session = Some(Session {
            connection: connection.clone(),
            id: None,
            prompt_capabilities: PromptCapabilities::default(),
            auth_methods: vec![],
            _events: events,
        });
        Ok(connection)
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        log::warn!("agent: {message}");
        self.status = Status::Error(message);
        self.turn = None;
        cx.notify();
    }

    fn turn_ended(&mut self, result: Result<PromptResponse, RpcError>, cx: &mut Context<Self>) {
        self.turn = None;
        for ix in self.thread.end_turn() {
            self.list_changed(Change::Updated(ix), cx);
        }
        match result {
            Ok(response) => {
                self.last_stop = Some(response.stop_reason);
                self.status = match response.stop_reason {
                    StopReason::MaxTokens => Status::Error("The agent stopped: it reached its token limit.".into()),
                    StopReason::MaxTurnRequests => Status::Error("The agent stopped: it reached its limit of requests for one turn.".into()),
                    StopReason::Refusal => Status::Error("The agent refused to go on.".into()),
                    _ => Status::Idle,
                };
            }
            Err(err) => {
                let exited = !self.running();
                self.status = Status::Error(if exited { self.exit_message() } else { format!("{}: {err}", self.agent.name) });
            }
        }
        cx.notify();
    }

    fn exit_message(&self) -> String {
        let stderr = self.session.as_ref().map(|s| s.connection.stderr_tail()).unwrap_or_default();
        let last = stderr.lines().rev().filter(|l| !l.trim().is_empty()).take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        if last.is_empty() { format!("{} exited.", self.agent.name) } else { format!("{} exited: {last}", self.agent.name) }
    }

    /// Stop: asks the agent to end the turn; pending permission requests
    /// are answered as cancelled, as the protocol requires.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            if self.status == Status::Starting {
                self.turn = None;
                self.status = Status::Idle;
                cx.notify();
            }
            return;
        };
        if let Some(id) = &session.id {
            session.connection.notify("session/cancel", crate::acp::schema::CancelNotification::new(id.clone()));
        } else {
            // Still starting: forget the message.
            self.turn = None;
            self.pending_prompt = None;
            self.status = Status::Idle;
        }
        for (call, _) in self.thread.pending_permissions() {
            self.answer_permission(&call, None, cx);
        }
        cx.notify();
    }

    /// Signs in with one of the agent's methods, then sends the waiting message.
    pub fn authenticate(&mut self, method: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connection) = self.session.as_ref().map(|s| s.connection.clone()) else { return };
        self.status = Status::Starting;
        cx.notify();
        self.turn = Some(cx.spawn_in(window, async move |this, cx| {
            let result = connection.request("authenticate", json!({ "methodId": method })).await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(_) => this.run_pending(window, cx),
                Err(err) => this.fail(format!("Signing in failed: {err}"), cx),
            })
            .ok();
        }));
    }

    pub fn set_mode(&mut self, mode: String, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        let Some(id) = session.id.clone() else { return };
        if let Some(modes) = &mut self.thread.modes {
            modes.current_mode_id = crate::acp::schema::SessionModeId::new(mode.clone());
        }
        let request = session.connection.request("session/set_mode", json!({ "sessionId": id, "modeId": mode }));
        cx.spawn(async move |this, cx| {
            if let Err(err) = request.await {
                this.update(cx, |this, cx| this.fail(format!("Couldn’t change the mode: {err}"), cx)).ok();
            }
        })
        .detach();
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // What the agent sends

    fn handle_incoming(&mut self, connection: usize, incoming: Incoming, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.session.as_ref().is_some_and(|s| Arc::as_ptr(&s.connection) as usize == connection);
        match incoming {
            Incoming::Notification { method, params } if method == "session/update" => {
                let Ok(notification) = serde_json::from_value::<SessionNotification>(params) else {
                    // A kind of update this version doesn't know.
                    return;
                };
                if !current || self.session.as_ref().and_then(|s| s.id.as_ref()) != Some(&notification.session_id) {
                    return;
                }
                self.apply_update(notification.update, window, cx);
            }
            Incoming::Notification { .. } => {}
            Incoming::Request { id, method, params } => {
                let Some(connection) = self.session.as_ref().filter(|_| current).map(|s| s.connection.clone()) else { return };
                match self.handle_request(id.clone(), &method, params, window, cx) {
                    Some(result) => connection.respond(id, result),
                    // Answered later (a permission prompt).
                    None => {}
                }
            }
            Incoming::Exited { .. } if current => {
                let message = self.exit_message();
                let was_busy = self.busy();
                self.turn = None;
                for (call, _) in self.thread.pending_permissions() {
                    self.thread.resolve_permission(&call, None);
                }
                // Started again with the next message, in a new session.
                self.session = None;
                if was_busy {
                    self.fail(message, cx);
                }
                cx.notify();
            }
            Incoming::Exited { .. } => {}
        }
    }

    fn apply_update(&mut self, update: crate::acp::schema::SessionUpdate, window: &mut Window, cx: &mut Context<Self>) {
        use crate::acp::schema::SessionUpdate;
        let follow_target = match &update {
            SessionUpdate::ToolCall(call) => call.locations.first().map(|l| (l.path.clone(), l.line)),
            SessionUpdate::ToolCallUpdate(update) => update.fields.locations.as_ref().and_then(|l| l.first()).map(|l| (l.path.clone(), l.line)),
            _ => None,
        };
        let change = self.thread.apply(update);
        // A finished edit the agent made itself shows in the editor at once.
        let changed = match change {
            Change::Updated(ix) => Some(ix),
            Change::Appended => self.thread.entries.len().checked_sub(1),
            Change::None | Change::Other => None,
        };
        if let Some(ix) = changed {
            self.track_tool_diffs(ix, window, cx);
        }
        self.list_changed(change, cx);
        if self.follow
            && let Some((path, line)) = follow_target
        {
            self.follow_to(&path, line.map(|l| l.saturating_sub(1)), window, cx);
        }
    }

    fn handle_request(&mut self, id: Value, method: &str, params: Value, window: &mut Window, cx: &mut Context<Self>) -> Option<Result<Value, RpcError>> {
        log::info!("agent asks: {method} {}", params.get("path").and_then(Value::as_str).unwrap_or(""));
        let invalid = |err: serde_json::Error| Some(Err(RpcError::new(RpcError::INVALID_PARAMS, err.to_string())));
        match method {
            "fs/read_text_file" => {
                let request: ReadTextFileRequest = match serde_json::from_value(params) {
                    Ok(request) => request,
                    Err(err) => return invalid(err),
                };
                let workspace = self.workspace.upgrade()?;
                let result = workspace.read(cx).agent_read_file(&request.path, request.line, request.limit, cx);
                Some(result.map(|content| json!({ "content": content })))
            }
            "fs/write_text_file" => {
                let request: WriteTextFileRequest = match serde_json::from_value(params) {
                    Ok(request) => request,
                    Err(err) => return invalid(err),
                };
                let workspace = self.workspace.upgrade()?;
                let opened = workspace.read(cx).buffer(&request.path).is_none();
                let result = workspace.update(cx, |ws, cx| ws.agent_write_file(&request.path, &request.content, window, cx));
                Some(result.map(|(before, first_line)| {
                    let path = request.path.canonicalize().unwrap_or(request.path.clone());
                    if opened {
                        self.opened_buffers.push(path.clone());
                    }
                    self.record_change(&path, before, &request.content, cx);
                    if self.follow {
                        self.follow_to(&path, Some(first_line), window, cx);
                    }
                    json!({})
                }))
            }
            "session/request_permission" => {
                let request: RequestPermissionRequest = match serde_json::from_value(params) {
                    Ok(request) => request,
                    Err(err) => return invalid(err),
                };
                let call_id = request.tool_call.tool_call_id.0.to_string();
                let follow_target = request.tool_call.fields.locations.as_ref().and_then(|l| l.first()).map(|l| (l.path.clone(), l.line));
                let change = self.thread.request_permission(id, request.tool_call, request.options);
                self.list_changed(change, cx);
                self.scroller.update(cx, |scroller, cx| scroller.scroll_to_end(cx));
                if self.always_allow
                    && let Some(option) = self.thread.tool_call(&call_id).and_then(|c| c.permission.as_ref()).and_then(|p| p.auto_allow()).map(|o| o.option_id.0.to_string())
                {
                    self.answer_permission(&call_id, Some(option), cx);
                } else if self.follow
                    && let Some((path, line)) = follow_target
                {
                    self.follow_to(&path, line.map(|l| l.saturating_sub(1)), window, cx);
                }
                None
            }
            _ => Some(Err(RpcError::new(RpcError::METHOD_NOT_FOUND, format!("zcode doesn’t support {method}")))),
        }
    }

    /// The user's answer to a tool call's permission request (`None`: cancelled).
    pub fn answer_permission(&mut self, call: &str, option: Option<String>, cx: &mut Context<Self>) {
        let Some((ix, request)) = self.thread.resolve_permission(call, option.clone()) else { return };
        let outcome = match option {
            Some(option) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option)),
            None => RequestPermissionOutcome::Cancelled,
        };
        if let Some(session) = &self.session {
            session.connection.respond(request, serde_json::to_value(RequestPermissionResponse::new(outcome)).map_err(|e| RpcError::internal(e.to_string())));
        }
        self.list_changed(Change::Updated(ix), cx);
    }

    fn follow_to(&mut self, path: &Path, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else { return };
        let path = path.to_path_buf();
        let composer = self.composer.read(cx).focus_handle(cx);
        let keep_focus = composer.is_focused(window).then_some(composer);
        // Opening a file updates the workspace, which may be drawing us.
        window.defer(cx, move |window, cx| workspace.update(cx, |ws, cx| ws.agent_follow(&path, line, keep_focus, window, cx)));
    }

    /// A file the agent wrote: remembered with what it was first.
    fn record_change(&mut self, path: &Path, before: Option<String>, now: &str, cx: &mut Context<Self>) {
        let ix = match self.changes.iter().position(|c| c.path == path) {
            Some(ix) => ix,
            None => {
                self.changes.push(FileChange { path: path.to_path_buf(), before, added: 0, removed: 0 });
                self.changes.len() - 1
            }
        };
        let change = &mut self.changes[ix];
        let (added, removed) = line_stats(change.before.as_deref().unwrap_or(""), now);
        change.added = added;
        change.removed = removed;
        cx.notify();
    }

    /// A tool call's diffs: files the agent changed without asking zcode
    /// to write them are reloaded and remembered too.
    fn track_tool_diffs(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Entry::Tool(call)) = self.thread.entries.get(ix) else { return };
        self.diff_cache.retain(|(entry, _), _| *entry != ix);
        if call.status != crate::acp::schema::ToolCallStatus::Completed {
            return;
        }
        let diffs: Vec<(PathBuf, Option<String>, String)> = call
            .content
            .iter()
            .filter_map(|c| match c {
                ToolContent::Diff { path, old, new } => Some((path.clone(), old.clone(), new.clone())),
                _ => None,
            })
            .collect();
        let Some(workspace) = self.workspace.upgrade() else { return };
        for (path, old, new) in diffs {
            let path = path.canonicalize().unwrap_or(path);
            if !path.starts_with(&self.root) {
                continue;
            }
            let (workspace, panel) = (workspace.clone(), cx.entity().downgrade());
            window.defer(cx, move |window, cx| {
                let adopted = workspace.update(cx, |ws, cx| ws.agent_adopt_disk_change(&path, (old, new), window, cx));
                panel
                    .update(cx, |this, cx| match adopted {
                        Some((before, now, opened)) => {
                            if opened {
                                this.opened_buffers.push(path.clone());
                            }
                            this.record_change(&path, before, &now, cx);
                        }
                        // Written through zcode (already recorded) or not readable.
                        None => {
                            if let Ok(now) = std::fs::read_to_string(&path)
                                && let Some(before) = this.changes.iter().find(|c| c.path == path).map(|c| c.before.clone())
                            {
                                this.record_change(&path, before, &now, cx);
                            }
                        }
                    })
                    .ok();
            });
        }
    }

    /// Review: the agent's changes to `path` compared with what it was before them.
    pub fn review(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(change) = self.changes.iter().find(|c| c.path == path).cloned() else { return };
        let Some(workspace) = self.workspace.upgrade() else { return };
        window.defer(cx, move |window, cx| {
            workspace.update(cx, |ws, cx| {
                ws.open_comparison(change.path.clone(), super::diff_view::DiffBase::BeforeAgent, Some(change.before.clone().unwrap_or_default()), window, cx)
            })
        });
    }

    /// Reject: the file back as it was before the agent (or, if it created
    /// it, in the Trash).
    pub fn reject(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.changes.iter().position(|c| c.path == path) else { return };
        let change = self.changes.remove(ix);
        let Some(workspace) = self.workspace.upgrade() else { return };
        window.defer(cx, move |window, cx| workspace.update(cx, |ws, cx| ws.agent_restore(&change.path, change.before.as_deref(), window, cx)));
        cx.notify();
    }

    /// Keep: the changes stay, and the panel stops listing them.
    pub fn keep(&mut self, path: Option<&Path>, cx: &mut Context<Self>) {
        self.changes.retain(|c| path.is_some_and(|p| p != c.path));
        cx.notify();
    }

    pub fn toggle_expanded(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !self.expanded.remove(&ix) {
            self.expanded.insert(ix);
        }
        self.list_changed(Change::Updated(ix), cx);
    }

    fn list_changed(&mut self, change: Change, cx: &mut Context<Self>) {
        let count = self.thread.entries.len();
        self.scroller.update(cx, |scroller, cx| match change {
            Change::Appended => {
                let known = scroller.item_count();
                if count > known {
                    scroller.append(count - known, cx);
                }
            }
            Change::Updated(ix) => {
                scroller.remeasure_items(ix..ix + 1, cx);
            }
            Change::None | Change::Other => {}
        });
        if let Change::Updated(ix) = change {
            self.diff_cache.retain(|(entry, _), _| *entry != ix);
        }
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // Mentions

    /// The mention picker; `at` is where an "@" was typed, which the
    /// mention replaces.
    pub fn show_mentions(&mut self, at: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else { return };
        let panel = cx.entity().downgrade();
        window.defer(cx, move |window, cx| workspace.update(cx, |ws, cx| ws.show_mention_picker(panel, at, window, cx)));
    }

    /// Puts a mention of `uri` (shown as `@label`) where the "@" was typed,
    /// or at the cursor.
    pub fn insert_mention(&mut self, uri: String, label: String, at: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let token = gpui_kit::component::input::InlineToken::new(uri, format!("@{label}"));
        self.composer.update(cx, |state, cx| {
            let text = state.value();
            let range = match at {
                Some(at) if text.get(at..at + 1) == Some("@") => at..at + 1,
                _ => state.cursor()..state.cursor(),
            };
            if state.replace_range_with_token(range, token, window, cx).is_ok() {
                state.insert(" ", window, cx);
            }
            state.focus(window, cx);
        });
    }

    // ---------------------------------------------------------------------
    // The control channel

    /// `agent <command>` (see `debug.rs`).
    pub fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        let (verb, rest) = command.split_once(' ').unwrap_or((command, ""));
        let rest = rest.trim();
        match verb {
            "type" => {
                self.composer.update(cx, |state, cx| {
                    state.focus(window, cx);
                    state.insert(rest.replace("\\n", "\n"), window, cx);
                });
                Ok(String::new())
            }
            "send" => {
                if !rest.is_empty() {
                    self.composer.update(cx, |state, cx| state.insert(rest.to_string(), window, cx));
                }
                self.send(window, cx);
                Ok(String::new())
            }
            "mention" => {
                // `mention <path>[:<first line>-<last line>]`, as the picker inserts it.
                let (path, lines) = match rest.rsplit_once(':') {
                    Some((path, lines)) if lines.contains('-') => {
                        let (a, b) = lines.split_once('-').unwrap();
                        (path, a.parse::<u32>().ok().zip(b.parse::<u32>().ok()).map(|(a, b)| a.saturating_sub(1)..b.saturating_sub(1)))
                    }
                    _ => (rest, None),
                };
                let path = self.root.join(path);
                let label = mentions::label(&path, lines.as_ref());
                self.insert_mention(mentions::uri(&path, lines), label, None, window, cx);
                Ok(String::new())
            }
            "allow" | "reject" => {
                // The first pending request's first option of that kind.
                let (call, permission) = self
                    .thread
                    .entries
                    .iter()
                    .find_map(|e| match e {
                        Entry::Tool(call) => call.permission.as_ref().filter(|p| p.pending()).map(|p| (call.id.clone(), p.clone())),
                        _ => None,
                    })
                    .ok_or("no permission request is waiting")?;
                let allow = verb == "allow";
                let always = rest == "always";
                use crate::acp::schema::PermissionOptionKind as Kind;
                let option = permission
                    .options
                    .iter()
                    .find(|o| match (allow, always) {
                        (true, false) => o.kind == Kind::AllowOnce,
                        (true, true) => o.kind == Kind::AllowAlways,
                        (false, false) => o.kind == Kind::RejectOnce,
                        (false, true) => o.kind == Kind::RejectAlways,
                    })
                    .ok_or("no such option")?;
                self.answer_permission(&call, Some(option.option_id.0.to_string()), cx);
                Ok(String::new())
            }
            "cancel" => {
                self.cancel(cx);
                Ok(String::new())
            }
            "new" => {
                self.new_thread(window, cx);
                Ok(String::new())
            }
            "agent" => {
                self.select_agent(rest, window, cx);
                Ok(String::new())
            }
            "follow" => {
                self.set_follow(rest != "off", cx);
                Ok(String::new())
            }
            "mode" => {
                self.set_mode(rest.to_string(), cx);
                Ok(String::new())
            }
            "auth" => {
                self.authenticate(rest.to_string(), window, cx);
                Ok(String::new())
            }
            "review" | "reject-file" | "keep" => {
                let path = (!rest.is_empty()).then(|| self.root.join(rest));
                match (verb, path) {
                    ("keep", path) => self.keep(path.as_deref(), cx),
                    (_, None) => return Err("which file?".into()),
                    ("review", Some(path)) => self.review(&path.canonicalize().unwrap_or(path), window, cx),
                    (_, Some(path)) => self.reject(&path.canonicalize().unwrap_or(path), window, cx),
                }
                Ok(String::new())
            }
            "expand" => {
                let ix: usize = rest.parse().map_err(|_| "expand <entry>")?;
                self.toggle_expanded(ix, cx);
                Ok(String::new())
            }
            _ => Err(format!("unknown agent command {verb}")),
        }
    }

    pub fn summary(&self, window: &Window, cx: &App) -> Value {
        let entries: Vec<Value> = self
            .thread
            .entries
            .iter()
            .map(|entry| match entry {
                Entry::User(message) => json!({ "kind": "user", "text": message.text, "mentions": message.mentions }),
                Entry::Assistant(text) => json!({ "kind": "assistant", "text": text }),
                Entry::Thought(text) => json!({ "kind": "thought", "text": text }),
                Entry::Tool(call) => json!({
                    "kind": "tool",
                    "id": call.id,
                    "title": call.title,
                    "tool_kind": format!("{:?}", call.kind),
                    "status": format!("{:?}", call.status),
                    "locations": call.locations.iter().map(|(p, l)| format!("{}:{}", p.display(), l.unwrap_or(0))).collect::<Vec<_>>(),
                    "content": call.content.iter().map(|c| match c {
                        ToolContent::Text(text) => json!({ "text": text }),
                        ToolContent::Diff { path, old, new } => json!({ "diff": path, "old_lines": old.as_deref().map(|o| o.lines().count()), "new_lines": new.lines().count() }),
                        ToolContent::Terminal(id) => json!({ "terminal": id }),
                    }).collect::<Vec<_>>(),
                    "permission": call.permission.as_ref().map(|p| json!({
                        "pending": p.pending(),
                        "options": p.options.iter().map(|o| o.name.clone()).collect::<Vec<_>>(),
                        "outcome": p.outcome.clone(),
                    })),
                }),
            })
            .collect();
        json!({
            "agent": self.agent.id,
            "agent_name": self.agent.name,
            "agents": self.agents.iter().map(|a| a.id.clone()).collect::<Vec<_>>(),
            "status": match &self.status {
                Status::Idle => "idle".to_string(),
                Status::Starting => "starting".into(),
                Status::Generating => "generating".into(),
                Status::Error(message) => format!("error: {message}"),
                Status::AuthRequired(methods) => format!("auth: {}", methods.iter().map(|m| m.0.clone()).collect::<Vec<_>>().join(",")),
            },
            "running": self.running(),
            "pid": self.agent_pid(),
            "session": self.session.as_ref().and_then(|s| s.id.as_ref()).map(|id| id.0.to_string()),
            "follow": self.follow,
            "composer": self.composer.read(cx).value().to_string(),
            "composer_tokens": self.composer.read(cx).tokens().iter().map(|t| t.token().id().to_string()).collect::<Vec<_>>(),
            "composer_focused": self.composer.read(cx).focus_handle(cx).is_focused(window),
            "entries": entries,
            "list_items": self.scroller.read(cx).item_count(),
            "plan": self.thread.plan.iter().map(|p| format!("{:?} {}", p.status, p.content)).collect::<Vec<_>>(),
            "modes": self.thread.modes.as_ref().map(|m| json!({
                "current": m.current_mode_id.0.to_string(),
                "available": m.available_modes.iter().map(|mode| mode.id.0.to_string()).collect::<Vec<_>>(),
            })),
            "commands": self.thread.commands.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
            "changes": self.changes.iter().map(|c| json!({ "path": c.path, "created": c.before.is_none(), "added": c.added, "removed": c.removed })).collect::<Vec<_>>(),
            "opened_buffers": self.opened_buffers,
            "stop_reason": self.last_stop.map(|s| format!("{s:?}")),
        })
    }
}

impl Drop for AgentPanel {
    fn drop(&mut self) {
        self.stop_agent();
    }
}

fn placeholder(agent: &AgentServer) -> String {
    format!("Message {} — @ to mention a file or symbol", agent.name)
}

fn auth_methods(response: &InitializeResponse) -> Vec<(String, String, Option<String>)> {
    // Read from JSON so every kind of method is listed, whatever its shape.
    let value = serde_json::to_value(&response.auth_methods).unwrap_or_default();
    value
        .as_array()
        .map(|methods| {
            methods
                .iter()
                .filter_map(|m| {
                    let id = m.get("id")?.as_str()?.to_string();
                    let name = m.get("name").and_then(Value::as_str).unwrap_or(&id).to_string();
                    let description = m.get("description").and_then(Value::as_str).map(str::to_string);
                    Some((id, name, description))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The message's parts as content blocks: mentions embedded with their
/// text when the agent takes that, links to the files otherwise.
fn prompt_blocks(parts: &[Part], capabilities: &PromptCapabilities) -> Vec<ContentBlock> {
    parts
        .iter()
        .filter_map(|part| match part {
            Part::Text(text) if text.is_empty() => None,
            Part::Text(text) => Some(ContentBlock::Text(TextContent::new(text.clone()))),
            Part::Mention { uri, name, text: Some(text) } if capabilities.embedded_context => {
                let _ = name;
                Some(ContentBlock::Resource(EmbeddedResource::new(EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(text.clone(), uri.clone())))))
            }
            Part::Mention { uri, name, .. } => Some(ContentBlock::ResourceLink(ResourceLink::new(name.clone(), uri.clone()))),
        })
        .collect()
}

/// Lines added and removed from `before` to `now`.
fn line_stats(before: &str, now: &str) -> (usize, usize) {
    crate::git::diff::line_hunks(before, now).iter().fold((0, 0), |(a, r), h| (a + h.new.len(), r + h.old.len()))
}

// -------------------------------------------------------------------------
// The workspace's side

impl Workspace {
    /// View ▸ Show Coding Assistant (⌘0): shows the panel (made the first
    /// time) with the keyboard in its message field; from the field, hides it.
    pub fn toggle_agent_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.ensure_agent_panel(window, cx);
        let focused = panel.read(cx).composer.read(cx).focus_handle(cx).contains_focused(window, cx);
        if self.agent.shown && focused {
            self.agent.shown = false;
            match self.active_editor() {
                Some(editor) => editor.update(cx, |state, cx| state.focus(window, cx)),
                None => self.focus_handle.focus(window, cx),
            }
        } else {
            self.agent.shown = true;
            panel.update(cx, |panel, cx| panel.composer.update(cx, |state, cx| state.focus(window, cx)));
        }
        self.save_session(cx);
        cx.notify();
    }

    /// File ▸ New ▸ Conversation (⌃⌘H): a new thread in the panel, shown.
    pub fn new_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.ensure_agent_panel(window, cx);
        self.agent.shown = true;
        panel.update(cx, |panel, cx| panel.new_thread(window, cx));
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn ensure_agent_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<AgentPanel> {
        if let Some(panel) = &self.agent.panel {
            return panel.clone();
        }
        let settings = self.settings.current.clone();
        let restored = self.agent.restored.take();
        let workspace = cx.weak_entity();
        let root = self.project.root.clone();
        let panel = cx.new(|cx| AgentPanel::new(workspace, root, &settings, restored.as_ref(), window, cx));
        self._subscriptions.push(cx.observe(&panel, |_, _, cx| cx.notify()));
        self.agent.panel = Some(panel.clone());
        panel
    }

    /// A session that had the panel open shows it again (without starting
    /// an agent), after the first frame.
    pub(super) fn restore_agent_panel(&mut self, state: Option<crate::session::AgentPanelState>, shown: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.agent.restored = state;
        if !shown {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            this.update_in(cx, |this, window, cx| {
                this.ensure_agent_panel(window, cx);
                this.agent.shown = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn agent_settings_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = &self.agent.panel {
            let settings = self.settings.current.clone();
            panel.update(cx, |panel, cx| panel.set_settings(&settings, cx));
        }
    }

    pub(super) fn show_mention_picker(&mut self, panel: WeakEntity<AgentPanel>, at: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let server = self.lsp.read(cx).server();
        let delegate = MentionDelegate::new(cx.weak_entity(), panel, at, self.project.root.clone(), self.files.clone(), server);
        let picker = cx.new(|cx| crate::picker::Picker::new(delegate, window, cx));
        self.agent.mention_open = true;
        self.show_modal(picker, window, cx);
    }

    /// `agent <command>` from the control channel: `show`, `hide`, or one of
    /// the panel's (`AgentPanel::command`).
    pub fn agent_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        match command {
            "show" => {
                if !self.agent.shown {
                    self.toggle_agent_panel(window, cx);
                }
                Ok(String::new())
            }
            "hide" => {
                self.agent.shown = false;
                self.save_session(cx);
                cx.notify();
                Ok(String::new())
            }
            "mentions" => {
                let panel = self.agent.panel.clone().ok_or("the agent panel isn’t open")?;
                self.show_mention_picker(panel.downgrade(), None, window, cx);
                Ok(String::new())
            }
            _ => {
                let panel = self.agent.panel.clone().ok_or("the agent panel isn’t open")?;
                panel.update(cx, |panel, cx| panel.command(command, window, cx))
            }
        }
    }

    pub(super) fn agent_summary(&self, window: &Window, cx: &App) -> Value {
        match &self.agent.panel {
            Some(panel) => {
                let mut summary = panel.read(cx).summary(window, cx);
                summary["shown"] = self.agent.shown.into();
                summary
            }
            None => json!({ "shown": false, "loaded": false }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Part, PromptCapabilities, line_stats, prompt_blocks};
    use serde_json::json;

    #[test]
    fn mentions_are_embedded_when_the_agent_takes_them() {
        let parts = vec![
            Part::Text("Explain ".into()),
            Part::Mention { uri: "file:///p/A.swift".into(), name: "A.swift".into(), text: Some("struct A {}\n".into()) },
            Part::Text(String::new()),
        ];
        let embedded: PromptCapabilities = serde_json::from_value(json!({ "embeddedContext": true })).unwrap();
        let blocks = serde_json::to_value(prompt_blocks(&parts, &embedded)).unwrap();
        assert_eq!(blocks, json!([
            { "type": "text", "text": "Explain " },
            { "type": "resource", "resource": { "uri": "file:///p/A.swift", "text": "struct A {}\n" } },
        ]));
        let blocks = serde_json::to_value(prompt_blocks(&parts, &PromptCapabilities::default())).unwrap();
        assert_eq!(blocks[1], json!({ "type": "resource_link", "name": "A.swift", "uri": "file:///p/A.swift" }));
    }

    #[test]
    fn lines_added_and_removed() {
        assert_eq!(line_stats("a\nb\n", "a\nB\nc\n"), (2, 1));
        assert_eq!(line_stats("", "new\n"), (1, 0));
    }
}

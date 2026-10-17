//! A conversation with an agent as the panel shows it: the user's
//! messages, the agent's (streamed a chunk at a time), its thinking, its
//! tool calls with their output, diffs and permission requests, and its
//! plan. Built from `session/update` notifications; no UI here.

use std::path::PathBuf;

use agent_client_protocol_schema::v1::{
    AvailableCommand, ContentBlock, EmbeddedResourceResource, PermissionOption, PermissionOptionKind, PlanEntry, SessionModeState, SessionUpdate,
    ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolKind,
};
use agent_client_protocol_schema::MaybeUndefined;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    User(UserMessage),
    /// Markdown, chunks appended as they arrive.
    Assistant(String),
    Thought(String),
    Tool(ToolCallEntry),
}

/// What the user sent: the text as typed, with the files and symbols it mentions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserMessage {
    pub text: String,
    pub mentions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallEntry {
    pub id: String,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolCallStatus,
    pub content: Vec<ToolContent>,
    /// Files and lines it reads or changes, to follow.
    pub locations: Vec<(PathBuf, Option<u32>)>,
    pub raw_input: Option<Value>,
    pub permission: Option<Permission>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolContent {
    Text(String),
    Diff { path: PathBuf, old: Option<String>, new: String },
    Terminal(String),
}

/// A `session/request_permission` the user hasn't answered, or the answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Permission {
    /// The JSON-RPC request to answer.
    pub request: Value,
    pub options: Vec<PermissionOption>,
    /// The option chosen; `Some(None)` when the turn was cancelled first.
    pub outcome: Option<Option<String>>,
}

impl Permission {
    pub fn pending(&self) -> bool {
        self.outcome.is_none()
    }

    /// The option `always_allow_tool_actions` picks: allow once if offered.
    pub fn auto_allow(&self) -> Option<&PermissionOption> {
        let allow = |kind| self.options.iter().find(|o| o.kind == kind);
        allow(PermissionOptionKind::AllowOnce).or_else(|| allow(PermissionOptionKind::AllowAlways))
    }
}

/// Which entries an update changed, so the list re-lays out only those.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    None,
    Appended,
    Updated(usize),
    /// The plan, modes or commands, outside the entries.
    Other,
}

#[derive(Debug, Default)]
pub struct Thread {
    pub entries: Vec<Entry>,
    pub plan: Vec<PlanEntry>,
    pub modes: Option<SessionModeState>,
    pub commands: Vec<AvailableCommand>,
    pub title: Option<String>,
}

impl Thread {
    pub fn push_user(&mut self, message: UserMessage) -> Change {
        self.entries.push(Entry::User(message));
        Change::Appended
    }

    pub fn apply(&mut self, update: SessionUpdate) -> Change {
        match update {
            SessionUpdate::UserMessageChunk(chunk) => {
                let text = block_text(&chunk.content);
                match self.entries.last_mut() {
                    Some(Entry::User(message)) => {
                        message.text.push_str(&text);
                        Change::Updated(self.entries.len() - 1)
                    }
                    _ => self.push_user(UserMessage { text, mentions: vec![] }),
                }
            }
            SessionUpdate::AgentMessageChunk(chunk) => self.append_text(block_text(&chunk.content), false),
            SessionUpdate::AgentThoughtChunk(chunk) => self.append_text(block_text(&chunk.content), true),
            SessionUpdate::ToolCall(call) => self.upsert_call(call),
            SessionUpdate::ToolCallUpdate(update) => self.update_call(update),
            SessionUpdate::Plan(plan) => {
                self.plan = plan.entries;
                Change::Other
            }
            SessionUpdate::AvailableCommandsUpdate(update) => {
                self.commands = update.available_commands;
                Change::Other
            }
            SessionUpdate::CurrentModeUpdate(update) => match &mut self.modes {
                Some(modes) => {
                    modes.current_mode_id = update.current_mode_id;
                    Change::Other
                }
                None => Change::None,
            },
            SessionUpdate::SessionInfoUpdate(info) => match info.title {
                MaybeUndefined::Value(title) => {
                    self.title = Some(title);
                    Change::Other
                }
                _ => Change::None,
            },
            _ => Change::None,
        }
    }

    fn append_text(&mut self, text: String, thought: bool) -> Change {
        if text.is_empty() {
            return Change::None;
        }
        let last = self.entries.len().wrapping_sub(1);
        match self.entries.last_mut() {
            Some(Entry::Assistant(existing)) if !thought => existing.push_str(&text),
            Some(Entry::Thought(existing)) if thought => existing.push_str(&text),
            _ => {
                self.entries.push(if thought { Entry::Thought(text) } else { Entry::Assistant(text) });
                return Change::Appended;
            }
        }
        Change::Updated(last)
    }

    pub fn tool_call_index(&self, id: &str) -> Option<usize> {
        self.entries.iter().rposition(|e| matches!(e, Entry::Tool(call) if call.id == id))
    }

    pub fn tool_call(&self, id: &str) -> Option<&ToolCallEntry> {
        self.tool_call_index(id).and_then(|ix| match &self.entries[ix] {
            Entry::Tool(call) => Some(call),
            _ => None,
        })
    }

    fn upsert_call(&mut self, call: ToolCall) -> Change {
        let entry = ToolCallEntry {
            id: call.tool_call_id.0.to_string(),
            title: call.title,
            kind: call.kind,
            status: call.status,
            content: call.content.into_iter().map(tool_content).collect(),
            locations: call.locations.into_iter().map(|l| (l.path, l.line)).collect(),
            raw_input: call.raw_input,
            permission: None,
        };
        match self.tool_call_index(&entry.id) {
            Some(ix) => {
                if let Entry::Tool(existing) = &mut self.entries[ix] {
                    let permission = existing.permission.take();
                    *existing = ToolCallEntry { permission, ..entry };
                }
                Change::Updated(ix)
            }
            None => {
                self.entries.push(Entry::Tool(entry));
                Change::Appended
            }
        }
    }

    /// Applies a tool call's update; one for a call not seen yet adds it
    /// (the spec allows that, and permission requests carry one).
    pub fn update_call(&mut self, update: ToolCallUpdate) -> Change {
        let id = update.tool_call_id.0.to_string();
        let fields = update.fields;
        let (ix, change) = match self.tool_call_index(&id) {
            Some(ix) => (ix, Change::Updated(ix)),
            None => {
                self.entries.push(Entry::Tool(ToolCallEntry {
                    id: id.clone(),
                    title: fields.title.clone().unwrap_or_else(|| "Tool call".into()),
                    kind: ToolKind::Other,
                    status: ToolCallStatus::Pending,
                    content: vec![],
                    locations: vec![],
                    raw_input: None,
                    permission: None,
                }));
                (self.entries.len() - 1, Change::Appended)
            }
        };
        let Entry::Tool(call) = &mut self.entries[ix] else { return Change::None };
        if let Some(title) = fields.title {
            call.title = title;
        }
        if let Some(kind) = fields.kind {
            call.kind = kind;
        }
        if let Some(status) = fields.status {
            call.status = status;
        }
        if let Some(content) = fields.content {
            call.content = content.into_iter().map(tool_content).collect();
        }
        if let Some(locations) = fields.locations {
            call.locations = locations.into_iter().map(|l| (l.path, l.line)).collect();
        }
        if let Some(raw_input) = fields.raw_input {
            call.raw_input = Some(raw_input);
        }
        change
    }

    /// The agent asks to run a tool call: the call (added or updated from
    /// what the request carries) shows the options until one is chosen.
    pub fn request_permission(&mut self, request: Value, call: ToolCallUpdate, options: Vec<PermissionOption>) -> Change {
        let id = call.tool_call_id.0.to_string();
        let change = self.update_call(call);
        if let Some(ix) = self.tool_call_index(&id)
            && let Entry::Tool(call) = &mut self.entries[ix]
        {
            call.permission = Some(Permission { request, options, outcome: None });
        }
        change
    }

    /// Records the answer to tool call `id`'s permission request; the
    /// request to respond to, if it was still pending.
    pub fn resolve_permission(&mut self, id: &str, option: Option<String>) -> Option<(usize, Value)> {
        let ix = self.tool_call_index(id)?;
        let Entry::Tool(call) = &mut self.entries[ix] else { return None };
        let permission = call.permission.as_mut().filter(|p| p.pending())?;
        permission.outcome = Some(option);
        Some((ix, permission.request.clone()))
    }

    /// Every unanswered permission request, as (tool call id, request).
    pub fn pending_permissions(&self) -> Vec<(String, Value)> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Tool(call) => call.permission.as_ref().filter(|p| p.pending()).map(|p| (call.id.clone(), p.request.clone())),
                _ => None,
            })
            .collect()
    }

    /// Tool calls still running when the turn ended can't finish any more.
    pub fn end_turn(&mut self) -> Vec<usize> {
        let mut changed = vec![];
        for (ix, entry) in self.entries.iter_mut().enumerate() {
            if let Entry::Tool(call) = entry
                && matches!(call.status, ToolCallStatus::Pending | ToolCallStatus::InProgress)
                && call.permission.as_ref().is_none_or(|p| !p.pending())
            {
                call.status = ToolCallStatus::Failed;
                changed.push(ix);
            }
        }
        changed
    }
}

/// A content block as text: text as is, links and resources as Markdown links.
pub fn block_text(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text(text) => text.text.clone(),
        ContentBlock::ResourceLink(link) => format!("[{}]({})", link.title.as_deref().unwrap_or(&link.name), link.uri),
        ContentBlock::Resource(resource) => match &resource.resource {
            EmbeddedResourceResource::TextResourceContents(contents) => format!("\n```\n{}\n```\n", contents.text.trim_end()),
            EmbeddedResourceResource::BlobResourceContents(contents) => format!("[{}]({})", file_name(&contents.uri), contents.uri),
            _ => String::new(),
        },
        ContentBlock::Image(_) => "[image]".into(),
        ContentBlock::Audio(_) => "[audio]".into(),
        _ => String::new(),
    }
}

fn file_name(uri: &str) -> &str {
    uri.rsplit('/').next().unwrap_or(uri)
}

fn tool_content(content: ToolCallContent) -> ToolContent {
    match content {
        ToolCallContent::Content(content) => ToolContent::Text(block_text(&content.content)),
        ToolCallContent::Diff(diff) => ToolContent::Diff { path: diff.path, old: diff.old_text, new: diff.new_text },
        ToolCallContent::Terminal(terminal) => ToolContent::Terminal(terminal.terminal_id.0.to_string()),
        _ => ToolContent::Text(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn update(value: Value) -> SessionUpdate {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn chunks_join_into_messages_and_thoughts() {
        let mut thread = Thread::default();
        thread.push_user(UserMessage { text: "Hi".into(), mentions: vec![] });
        assert_eq!(thread.apply(update(json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "Hmm" } }))), Change::Appended);
        assert_eq!(thread.apply(update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Hel" } }))), Change::Appended);
        assert_eq!(thread.apply(update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "lo" } }))), Change::Updated(2));
        let link = json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "resource_link", "name": "A.swift", "uri": "file:///p/A.swift" } });
        thread.apply(update(link));
        assert_eq!(thread.entries[1], Entry::Thought("Hmm".into()));
        assert_eq!(thread.entries[2], Entry::Assistant("Hello[A.swift](file:///p/A.swift)".into()));
    }

    #[test]
    fn tool_calls_update_in_place_and_permissions_resolve_once() {
        let mut thread = Thread::default();
        let call = json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Edit A.swift", "kind": "edit", "status": "pending",
            "locations": [{ "path": "/p/A.swift", "line": 3 }] });
        assert_eq!(thread.apply(update(call)), Change::Appended);
        let request: agent_client_protocol_schema::v1::RequestPermissionRequest = serde_json::from_value(json!({
            "sessionId": "s", "toolCall": { "toolCallId": "t1", "status": "pending" },
            "options": [
                { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                { "optionId": "reject", "name": "Reject", "kind": "reject_once" },
            ],
        }))
        .unwrap();
        assert_eq!(thread.request_permission(json!(7), request.tool_call, request.options), Change::Updated(0));
        assert_eq!(thread.pending_permissions(), [("t1".to_string(), json!(7))]);
        let Entry::Tool(call) = &thread.entries[0] else { panic!() };
        assert_eq!(call.permission.as_ref().unwrap().auto_allow().unwrap().option_id.0.as_ref(), "allow");
        assert_eq!(thread.resolve_permission("t1", Some("allow".into())), Some((0, json!(7))));
        assert_eq!(thread.resolve_permission("t1", Some("reject".into())), None, "answered once");
        let diff = json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
            "content": [{ "type": "diff", "path": "/p/A.swift", "oldText": "a\n", "newText": "b\n" }] });
        assert_eq!(thread.apply(update(diff)), Change::Updated(0));
        let Entry::Tool(call) = &thread.entries[0] else { panic!() };
        assert_eq!(call.title, "Edit A.swift", "a title the update leaves out stays");
        assert_eq!(call.status, ToolCallStatus::Completed);
        assert_eq!(call.locations, [(PathBuf::from("/p/A.swift"), Some(3))]);
        assert_eq!(call.content, [ToolContent::Diff { path: "/p/A.swift".into(), old: Some("a\n".into()), new: "b\n".into() }]);
        assert!(call.permission.as_ref().is_some_and(|p| !p.pending()));
    }

    #[test]
    fn an_update_for_an_unknown_call_adds_it_and_unfinished_calls_fail_at_the_end() {
        let mut thread = Thread::default();
        let change = thread.apply(update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t9", "title": "Run tests", "status": "in_progress" })));
        assert_eq!(change, Change::Appended);
        assert_eq!(thread.end_turn(), [0]);
        assert!(matches!(&thread.entries[0], Entry::Tool(call) if call.status == ToolCallStatus::Failed && call.title == "Run tests"));
    }

    #[test]
    fn plan_modes_and_commands() {
        let mut thread = Thread::default();
        thread.modes = serde_json::from_value(json!({ "currentModeId": "default", "availableModes": [{ "id": "default", "name": "Default" }, { "id": "plan", "name": "Plan" }] })).unwrap();
        thread.apply(update(json!({ "sessionUpdate": "current_mode_update", "currentModeId": "plan" })));
        assert_eq!(thread.modes.as_ref().unwrap().current_mode_id.0.as_ref(), "plan");
        thread.apply(update(json!({ "sessionUpdate": "plan", "entries": [{ "content": "Read", "priority": "high", "status": "completed" }] })));
        assert_eq!(thread.plan.len(), 1);
        thread.apply(update(json!({ "sessionUpdate": "available_commands_update", "availableCommands": [{ "name": "init", "description": "Set up" }] })));
        assert_eq!(thread.commands[0].name, "init");
    }
}

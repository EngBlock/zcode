//! Adapters from the editor's provider traits to a document's language
//! servers.
//!
//! A document may have several servers (vtsls, ESLint and Tailwind for a
//! React component): completions, code actions and document colors are
//! everyone's; hover, definitions and signature help come from the first
//! server with an answer. Each completion and code action remembers which
//! server made it, for resolving and running it there.
//!
//! The editor addresses text by byte offset and (for LSP-shaped data it
//! applies itself) by *character* columns, while servers speak UTF-16, so
//! every range crossing this boundary is converted here.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use futures::FutureExt as _;
use gpui_kit::{App, AppContext as _, Entity, SharedString, Task, WeakEntity, Window};
use lsp_types::{CodeAction, CodeActionOrCommand, CompletionContext, CompletionItem, CompletionResponse, CompletionTextEdit, GotoDefinitionResponse, LocationLink};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde_json::{Value, json};
use zcode_editor::{
    CodeActionProvider, CompletionProvider, DefinitionProvider, DocumentColorProvider, EditorState, HoverProvider, Rope, RopeExt as _,
    SignatureHelpProvider,
};

use super::{LanguageServer, LspStore, lsp_range_to_offsets, offset_to_lsp, path_to_uri};

/// Applies a workspace edit through the workspace (may touch other files).
pub type ApplyEditFn = Rc<dyn Fn(lsp_types::WorkspaceEdit, &mut Window, &mut App)>;

/// Completions made without a server (Emmet's), in the editor's coordinates.
pub trait LocalCompletions {
    fn completions(&self, text: &Rope, offset: usize, invoked: bool) -> Vec<CompletionItem>;
    /// Characters that ask for them (`>`, `*` for an abbreviation).
    fn is_trigger(&self, ch: char) -> bool;
}

const MAX_COMPLETIONS: usize = 250;

/// Where a completion or code action came from, kept in its `data`.
const SERVER_KEY: &str = "zcode_server";

/// One instance per open document, shared by all provider slots.
#[derive(Clone)]
pub struct DocumentProviders {
    store: WeakEntity<LspStore>,
    path: PathBuf,
    apply_edit: ApplyEditFn,
    local: Option<Rc<dyn LocalCompletions>>,
}

impl DocumentProviders {
    pub fn new(store: WeakEntity<LspStore>, path: PathBuf, apply_edit: ApplyEditFn) -> Self {
        Self { store, path, apply_edit, local: None }
    }

    /// Adds completions of its own (Emmet's) to the servers'.
    pub fn with_local(mut self, local: Option<Rc<dyn LocalCompletions>>) -> Self {
        self.local = local;
        self
    }

    /// Installs completion, hover, definition, signature help, code-action
    /// and document color providers.
    pub fn install(self, editor: &Entity<EditorState>, cx: &mut App) {
        let this = Rc::new(self);
        editor.update(cx, |state, cx| {
            let lsp = state.lsp_mut();
            lsp.completion_provider = Some(this.clone());
            lsp.hover_provider = Some(this.clone());
            lsp.definition_provider = Some(this.clone());
            lsp.signature_help_provider = Some(this.clone());
            lsp.document_color_provider = Some(this.clone());
            lsp.code_action_providers = vec![this];
            cx.notify();
        });
    }

    /// The document's ready servers offering `capability`, synced with `text`.
    fn servers(&self, text: Option<&Rope>, capability: &str, cx: &mut App) -> Vec<(usize, Arc<LanguageServer>)> {
        let Some(store) = self.store.upgrade() else { return vec![] };
        store.update(cx, |store, cx| store.synced_servers(&self.path, text, Some(capability), cx))
    }

    fn server_by_id(&self, id: usize, cx: &App) -> Option<Arc<LanguageServer>> {
        self.store.upgrade()?.read(cx).server_by_id(id)
    }

    fn text_document(&self) -> Value {
        json!({ "uri": path_to_uri(&self.path).as_str() })
    }

    /// Asks each server in turn until one answers with something.
    fn first_answer<T: Send + 'static>(
        &self,
        servers: Vec<(usize, Arc<LanguageServer>)>,
        method: &'static str,
        params: Value,
        cx: &mut App,
        decode: impl Fn(Value) -> Option<T> + Send + 'static,
    ) -> Task<Result<Option<T>>> {
        let requests: Vec<_> = servers.iter().map(|(_, server)| server.request_raw(method, params.clone())).collect();
        cx.background_spawn(async move {
            let mut last_error = None;
            for request in requests {
                match request.await {
                    Ok(value) => {
                        if let Some(answer) = decode(value) {
                            return Ok(Some(answer));
                        }
                    }
                    Err(err) => last_error = Some(err),
                }
            }
            match last_error {
                Some(err) => Err(err),
                None => Ok(None),
            }
        })
    }
}

/// UTF-16 LSP range → the character-column range the editor expects.
fn to_editor_range(text: &Rope, range: lsp_types::Range) -> lsp_types::Range {
    let offsets = lsp_range_to_offsets(text, range);
    lsp_types::Range::new(text.offset_to_position(offsets.start), text.offset_to_position(offsets.end))
}

/// A server's completion, as the editor inserts it: ranges in its
/// coordinates (a snippet stays one; the editor expands it), and where it
/// came from in `data`, with the item as the server sent it (for
/// `completionItem/resolve`).
fn convert_completion(text: &Rope, server: usize, mut item: CompletionItem) -> CompletionItem {
    let original = serde_json::to_value(&item).unwrap_or(Value::Null);
    item.text_edit = item.text_edit.map(|edit| match edit {
        CompletionTextEdit::Edit(mut edit) => {
            edit.range = to_editor_range(text, edit.range);
            CompletionTextEdit::Edit(edit)
        }
        CompletionTextEdit::InsertAndReplace(mut edit) => {
            edit.insert = to_editor_range(text, edit.insert);
            edit.replace = to_editor_range(text, edit.replace);
            CompletionTextEdit::InsertAndReplace(edit)
        }
    });
    item.data = Some(json!({ SERVER_KEY: server, "item": original }));
    item
}

/// Where the text a completion replaces starts: its edit's range, else the
/// identifier before the cursor.
fn replaced_start(text: &Rope, offset: usize, item: &CompletionItem) -> usize {
    let start = match &item.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => Some(edit.range.start),
        Some(CompletionTextEdit::InsertAndReplace(edit)) => Some(edit.insert.start),
        None => None,
    };
    match start.map(|start| lsp_range_to_offsets(text, lsp_types::Range::new(start, start)).start) {
        Some(start) if start <= offset && text.offset_to_point(start).row == text.offset_to_point(offset).row => start,
        _ => word_start(text, offset),
    }
}

fn word_start(text: &Rope, offset: usize) -> usize {
    let len: usize = text
        .chars_at(offset)
        .reversed()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '$')
        .map(char::len_utf8)
        .sum();
    offset - len
}

/// Ranks completions from every server against what's been typed, as VS
/// Code does: the fuzzy score of each one's filter text against the text it
/// replaces, then the server's own order (`sortText`), servers in turn.
/// Items that don't match are dropped (some servers send everything in
/// scope and leave filtering to the client).
fn rank_completions(text: &Rope, offset: usize, items: Vec<(usize, CompletionItem)>) -> Vec<CompletionItem> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut patterns: HashMap<String, Option<Pattern>> = HashMap::new();
    let mut buf = Vec::new();
    let mut ranked: Vec<(u32, usize, CompletionItem)> = items
        .into_iter()
        .filter_map(|(order, item)| {
            let start = replaced_start(text, offset, &item);
            let query = text.slice(start..offset).to_string();
            // A number being typed isn't a name: nothing completes `1` but
            // what starts with it.
            if let Some(digit) = query.chars().next().filter(char::is_ascii_digit)
                && !item.filter_text.as_deref().unwrap_or(&item.label).starts_with(digit)
            {
                return None;
            }
            let pattern = patterns
                .entry(query.clone())
                .or_insert_with(|| (!query.trim().is_empty()).then(|| Pattern::parse(&query, CaseMatching::Ignore, Normalization::Smart)));
            let score = match pattern {
                None => 0,
                Some(pattern) => {
                    let candidate = item.filter_text.as_deref().unwrap_or(&item.label);
                    pattern.score(Utf32Str::new(candidate, &mut buf), &mut matcher)?
                }
            };
            Some((score, order, item))
        })
        .collect();
    // Local completions (order 0) come above the servers' when they're
    // `preselect`, below them otherwise.
    let tier = |order: usize, item: &CompletionItem| match (order, item.preselect) {
        (0, Some(true)) => 0,
        (0, _) => 2,
        _ => 1,
    };
    ranked.sort_by(|(sa, oa, a), (sb, ob, b)| {
        tier(*oa, a)
            .cmp(&tier(*ob, b))
            .then(sb.cmp(sa))
            .then(oa.cmp(ob))
            .then_with(|| a.sort_text.as_deref().unwrap_or(&a.label).cmp(b.sort_text.as_deref().unwrap_or(&b.label)))
    });
    ranked.truncate(MAX_COMPLETIONS);
    ranked.into_iter().map(|(_, _, item)| item).collect()
}

/// The server a completion or code action came from, and the item as it
/// sent it.
fn origin(data: Option<&Value>) -> Option<(usize, Value)> {
    let data = data?;
    Some((data.get(SERVER_KEY)?.as_u64()? as usize, data.get("item").cloned().unwrap_or(Value::Null)))
}

impl CompletionProvider for DocumentProviders {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let invoked = trigger.trigger_kind == lsp_types::CompletionTriggerKind::INVOKED;
        let local = self.local.as_ref().map(|local| local.completions(text, offset, invoked)).unwrap_or_default();
        let servers = self.servers(Some(text), "/completionProvider", cx);
        let features: Vec<super::Features> =
            self.store.upgrade().map(|store| servers.iter().map(|(id, _)| store.read(cx).features_of(*id)).collect()).unwrap_or_default();
        let text = text.clone();
        let position = offset_to_lsp(&text, offset);
        let previous = offset.checked_sub(1).filter(|o| text.is_char_boundary(*o)).and_then(|o| text.slice(o..offset).chars().next());
        let requests: Vec<_> = servers
            .iter()
            .zip(&features)
            .map(|((id, server), features)| {
                let typed = previous.map(String::from).filter(|ch| features.completion_triggers.contains(ch));
                let context = match typed {
                    Some(ch) if !invoked => json!({ "triggerKind": 2, "triggerCharacter": ch }),
                    _ => json!({ "triggerKind": 1 }),
                };
                let request = server.request_raw(
                    "textDocument/completion",
                    json!({ "textDocument": self.text_document(), "position": position, "context": context }),
                );
                let id = *id;
                async move { (id, request.await) }
            })
            .collect();
        cx.background_spawn(async move {
            let mut items: Vec<(usize, CompletionItem)> = local.into_iter().map(|item| (0, item)).collect();
            for (order, (server, answer)) in futures::future::join_all(requests).await.into_iter().enumerate() {
                let value = match answer {
                    Ok(value) => value,
                    Err(err) => {
                        log::debug!("completion from server {server}: {err:#}");
                        continue;
                    }
                };
                log::debug!("completion from server {server}: {} bytes", value.to_string().len());
                let response: Option<CompletionResponse> = serde_json::from_value(value).unwrap_or(None);
                let list = match response {
                    Some(CompletionResponse::Array(items)) => items,
                    Some(CompletionResponse::List(list)) => list.items,
                    None => vec![],
                };
                items.extend(list.into_iter().map(|item| (order + 1, convert_completion(&text, server, item))));
            }
            Ok(CompletionResponse::Array(rank_completions(&text, offset, items)))
        })
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, cx: &mut App) -> bool {
        let mut chars = new_text.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return false;
        };
        if ch == '.' || ch == '_' || ch.is_alphanumeric() {
            return true;
        }
        if self.local.as_ref().is_some_and(|local| local.is_trigger(ch)) {
            return true;
        }
        // The servers' own (`<` and `/` in HTML, `:` in CSS); never a space,
        // which would open the menu all through a line, or `(`, where
        // sourcekit-lsp's argument labels would cover the signature help.
        let Some(store) = self.store.upgrade() else { return false };
        let store = store.read(cx);
        let typed = ch.to_string();
        ch != ' ' && ch != '(' && store.document_features(&self.path).iter().any(|f| f.completion_triggers.contains(&typed))
    }

    /// Resolves the item for its additional edits (an import vtsls adds
    /// with the completion) and applies them.
    fn completion_inserted(&self, item: &CompletionItem, window: &mut Window, cx: &mut App) {
        let apply_edit = self.apply_edit.clone();
        let uri = path_to_uri(&self.path);
        let apply = move |edits: Vec<lsp_types::TextEdit>, window: &mut Window, cx: &mut App| {
            if edits.is_empty() {
                return;
            }
            #[allow(clippy::mutable_key_type)]
            let changes = std::collections::HashMap::from([(uri.clone(), edits)]);
            let apply_edit = apply_edit.clone();
            // The editor is being updated: edit it after.
            window.defer(cx, move |window, cx| apply_edit(lsp_types::WorkspaceEdit { changes: Some(changes), ..Default::default() }, window, cx));
        };
        let Some((server_id, original)) = origin(item.data.as_ref()) else { return };
        if let Some(edits) = item.additional_text_edits.clone().filter(|e| !e.is_empty()) {
            apply(edits, window, cx);
            return;
        }
        let resolves = self.store.upgrade().is_some_and(|store| store.read(cx).features_of(server_id).completion_resolve);
        let Some(server) = self.server_by_id(server_id, cx).filter(|_| resolves) else { return };
        let request = server.request_raw("completionItem/resolve", original);
        window
            .spawn(cx, async move |cx| {
                let resolved: Option<CompletionItem> = request.await.ok().and_then(|value| serde_json::from_value(value).ok());
                if let Some(edits) = resolved.and_then(|item| item.additional_text_edits) {
                    cx.update(|window, cx| apply(edits, window, cx)).ok();
                }
            })
            .detach();
    }
}

impl HoverProvider for DocumentProviders {
    fn hover(&self, text: &Rope, offset: usize, _window: &mut Window, cx: &mut App) -> Task<Result<Option<lsp_types::Hover>>> {
        let servers = self.servers(Some(text), "/hoverProvider", cx);
        let text = text.clone();
        let params = json!({ "textDocument": self.text_document(), "position": offset_to_lsp(&text, offset) });
        self.first_answer(servers, "textDocument/hover", params, cx, move |value| {
            let hover: lsp_types::Hover = serde_json::from_value(value).ok()?;
            let empty = match &hover.contents {
                lsp_types::HoverContents::Markup(markup) => markup.value.trim().is_empty(),
                lsp_types::HoverContents::Array(parts) => parts.is_empty(),
                lsp_types::HoverContents::Scalar(lsp_types::MarkedString::String(s)) => s.trim().is_empty(),
                lsp_types::HoverContents::Scalar(lsp_types::MarkedString::LanguageString(s)) => s.value.trim().is_empty(),
            };
            (!empty).then(|| lsp_types::Hover { range: hover.range.map(|r| to_editor_range(&text, r)), ..hover })
        })
    }
}

impl DefinitionProvider for DocumentProviders {
    fn definitions(&self, text: &Rope, offset: usize, _window: &mut Window, cx: &mut App) -> Task<Result<Vec<LocationLink>>> {
        let servers = self.servers(Some(text), "/definitionProvider", cx);
        let text = text.clone();
        let origin = text
            .word_range(offset)
            .map(|r| lsp_types::Range::new(text.offset_to_position(r.start), text.offset_to_position(r.end)));
        let params = json!({ "textDocument": self.text_document(), "position": offset_to_lsp(&text, offset) });
        let task = self.first_answer(servers, "textDocument/definition", params, cx, move |value| {
            log::debug!("definition response: {value}");
            let response: Option<GotoDefinitionResponse> = serde_json::from_value(value).ok()?;
            Some(definition_links(response, origin)).filter(|links| !links.is_empty())
        });
        cx.background_spawn(async move { Ok(task.await?.unwrap_or_default()) })
    }
}

impl SignatureHelpProvider for DocumentProviders {
    fn trigger_characters(&self, cx: &App) -> (Vec<String>, Vec<String>) {
        let Some(store) = self.store.upgrade() else { return Default::default() };
        let store = store.read(cx);
        let mut triggers: (Vec<String>, Vec<String>) = Default::default();
        for features in store.document_features(&self.path) {
            if let Some(help) = features.signature_help {
                triggers.0.extend(help.triggers.into_iter().filter(|t| !triggers.0.contains(t)).collect::<Vec<_>>());
                triggers.1.extend(help.retriggers.into_iter().filter(|t| !triggers.1.contains(t)).collect::<Vec<_>>());
            }
        }
        triggers
    }

    fn signature_help(
        &self,
        text: &Rope,
        offset: usize,
        context: lsp_types::SignatureHelpContext,
        cx: &mut App,
    ) -> Task<Result<Option<lsp_types::SignatureHelp>>> {
        let servers = self.servers(Some(text), "/signatureHelpProvider", cx);
        let params = json!({ "textDocument": self.text_document(), "position": offset_to_lsp(text, offset), "context": context });
        self.first_answer(servers, "textDocument/signatureHelp", params, cx, |value| {
            serde_json::from_value::<lsp_types::SignatureHelp>(value).ok().filter(|help| !help.signatures.is_empty())
        })
    }
}

impl DocumentColorProvider for DocumentProviders {
    fn document_colors(&self, text: &Rope, _window: &mut Window, cx: &mut App) -> Task<Result<Vec<lsp_types::ColorInformation>>> {
        let servers = self.servers(Some(text), "/colorProvider", cx);
        let text = text.clone();
        let requests: Vec<_> = servers
            .iter()
            .map(|(_, server)| server.request_raw("textDocument/documentColor", json!({ "textDocument": self.text_document() })))
            .collect();
        cx.background_spawn(async move {
            let mut colors = vec![];
            for answer in futures::future::join_all(requests).await {
                let found: Vec<lsp_types::ColorInformation> = answer.ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
                colors.extend(found.into_iter().map(|mut c| {
                    c.range = to_editor_range(&text, c.range);
                    c
                }));
            }
            Ok(colors)
        })
    }
}

/// Normalizes every shape of a definition response into links. Target ranges
/// stay in the server's UTF-16 coordinates; the workspace converts them once
/// the target file is loaded.
pub fn definition_links(response: Option<GotoDefinitionResponse>, origin: Option<lsp_types::Range>) -> Vec<LocationLink> {
    let link = |location: lsp_types::Location| LocationLink {
        origin_selection_range: origin,
        target_uri: location.uri,
        target_range: location.range,
        target_selection_range: location.range,
    };
    match response {
        None => vec![],
        Some(GotoDefinitionResponse::Scalar(location)) => vec![link(location)],
        Some(GotoDefinitionResponse::Array(locations)) => locations.into_iter().map(link).collect(),
        Some(GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|mut l| {
                l.origin_selection_range = origin;
                l
            })
            .collect(),
    }
}

impl CodeActionProvider for DocumentProviders {
    fn id(&self) -> SharedString {
        "language-servers".into()
    }

    fn code_actions(&self, state: Entity<EditorState>, range: Range<usize>, _window: &mut Window, cx: &mut App) -> Task<Result<Vec<CodeAction>>> {
        // The editor is mid-update when it asks for actions; use the store's copy.
        let _ = state;
        let Some(store) = self.store.upgrade() else { return Task::ready(Ok(vec![])) };
        let Some(text) = store.read(cx).document_text(&self.path) else { return Task::ready(Ok(vec![])) };
        let servers = self.servers(None, "/codeActionProvider", cx);
        let lsp_range = lsp_types::Range::new(offset_to_lsp(&text, range.start), offset_to_lsp(&text, range.end));
        let requests: Vec<_> = servers
            .iter()
            .map(|(id, server)| {
                // Each server is given its own diagnostics (ESLint's fixes are for ESLint's).
                let diagnostics: Vec<_> = store
                    .read(cx)
                    .diagnostics_of(*id, &self.path)
                    .iter()
                    .filter(|d| d.range.start.line <= lsp_range.end.line && d.range.end.line >= lsp_range.start.line)
                    .cloned()
                    .collect();
                let request = server.request_raw(
                    "textDocument/codeAction",
                    json!({ "textDocument": self.text_document(), "range": lsp_range, "context": { "diagnostics": diagnostics } }),
                );
                let id = *id;
                request.map(move |answer| (id, answer))
            })
            .collect();
        cx.background_spawn(async move {
            let mut all = vec![];
            for (server, answer) in futures::future::join_all(requests).await {
                let actions: Option<Vec<CodeActionOrCommand>> = answer.ok().and_then(|v| serde_json::from_value(v).ok());
                all.extend(actions.unwrap_or_default().into_iter().map(|action| {
                    let mut action = match action {
                        CodeActionOrCommand::CodeAction(action) => action,
                        CodeActionOrCommand::Command(command) => CodeAction { title: command.title.clone(), command: Some(command), ..Default::default() },
                    };
                    let original = serde_json::to_value(&action).unwrap_or(Value::Null);
                    action.data = Some(json!({ SERVER_KEY: server, "item": original }));
                    action
                }));
            }
            Ok(all)
        })
    }

    fn perform_code_action(
        &self,
        _state: Entity<EditorState>,
        action: CodeAction,
        _push_to_history: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let Some((server_id, original)) = origin(action.data.as_ref()) else { return Task::ready(Ok(())) };
        let Some(server) = self.server_by_id(server_id, cx) else { return Task::ready(Ok(())) };
        let resolves = self.store.upgrade().is_some_and(|store| store.read(cx).features_of(server_id).code_action_resolve);
        let apply_edit = self.apply_edit.clone();
        // An action without its edit gets it from `codeAction/resolve`.
        let resolve = (action.edit.is_none() && resolves && original.get("data").is_some())
            .then(|| server.request_raw("codeAction/resolve", original));
        window.spawn(cx, async move |cx| {
            let action = match resolve {
                Some(request) => serde_json::from_value::<CodeAction>(request.await?).unwrap_or(action),
                None => action,
            };
            if let Some(edit) = action.edit {
                cx.update(|window, cx| apply_edit(edit, window, cx))?;
            }
            if let Some(command) = action.command {
                // Any resulting edit arrives as a `workspace/applyEdit` request.
                server
                    .request_raw("workspace/executeCommand", json!({ "command": command.command, "arguments": command.arguments }))
                    .await?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, sort: &str) -> CompletionItem {
        CompletionItem { label: label.into(), sort_text: Some(sort.into()), ..Default::default() }
    }

    #[test]
    fn completions_are_filtered_and_ranked_across_servers() {
        let text = Rope::from("const x = use");
        let offset = text.len();
        let items = vec![
            (1, item("useState", "11")),
            (1, item("Promise", "15")),
            (1, item("useEffect", "10")),
            (2, item("user-select", "0")),
            (1, item("unsafeUse", "12")),
        ];
        let ranked: Vec<String> = rank_completions(&text, offset, items).into_iter().map(|i| i.label).collect();
        assert!(!ranked.contains(&"Promise".to_string()), "{ranked:?}");
        assert_eq!(&ranked[..2], ["useEffect", "useState"], "same score: the server's order");
        // Nothing typed: every item, servers in turn.
        let text = Rope::from("x.");
        let ranked: Vec<String> = rank_completions(&text, 2, vec![(2, item("b", "0")), (1, item("a", "9"))]).into_iter().map(|i| i.label).collect();
        assert_eq!(ranked, ["a", "b"]);
        // Typing a number completes nothing that doesn't start with it.
        let text = Rope::from("const n = 1");
        assert!(rank_completions(&text, text.len(), vec![(1, item("Int16Array", "0"))]).is_empty());
    }

    #[test]
    fn a_completions_edit_says_what_it_replaces() {
        // Tailwind replaces the class name typed so far, dashes and all.
        let text = Rope::from("<div class=\"bg-re");
        let offset = text.len();
        let mut tw = item("bg-red-500", "0");
        tw.text_edit = Some(CompletionTextEdit::Edit(lsp_types::TextEdit {
            range: lsp_types::Range::new(lsp_types::Position::new(0, 12), lsp_types::Position::new(0, 17)),
            new_text: "bg-red-500".into(),
        }));
        assert_eq!(replaced_start(&text, offset, &tw), 12);
        assert_eq!(rank_completions(&text, offset, vec![(1, tw), (1, item("flex", "0"))]).len(), 1);
    }

    #[test]
    fn items_remember_their_server() {
        let text = Rope::from("a");
        let converted = convert_completion(&text, 3, CompletionItem { label: "x".into(), data: Some(json!({ "id": 7 })), ..Default::default() });
        let (server, original) = origin(converted.data.as_ref()).unwrap();
        assert_eq!(server, 3);
        assert_eq!(original["data"]["id"], 7);
    }
}

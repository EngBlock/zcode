//! Adapters from the editor's provider traits to sourcekit-lsp requests.
//!
//! The editor addresses text by byte offset and (for LSP-shaped data it
//! applies itself) by *character* columns, while the server speaks UTF-16, so
//! every range crossing this boundary is converted here.

use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use zcode_editor::{
    CodeActionProvider, CompletionProvider, DefinitionProvider, EditorState, HoverProvider, Rope, RopeExt as _,
    SignatureHelpProvider,
};
use gpui_kit::{App, AppContext as _, Entity, SharedString, Task, WeakEntity, Window};
use lsp_types::{
    CodeAction, CodeActionOrCommand, CompletionContext, CompletionItem, CompletionResponse,
    CompletionTextEdit, GotoDefinitionResponse, InsertTextFormat, LocationLink,
};
use serde_json::{Value, json};

use super::{
    LanguageServer, LspStore, lsp_range_to_offsets, offset_to_lsp, path_to_uri,
    snippet_to_placeholders,
};

/// Applies a workspace edit through the workspace (may touch other files).
pub type ApplyEditFn = Rc<dyn Fn(lsp_types::WorkspaceEdit, &mut Window, &mut App)>;

const MAX_COMPLETIONS: usize = 250;

/// One instance per open document, shared by all provider slots.
#[derive(Clone)]
pub struct DocumentProviders {
    store: WeakEntity<LspStore>,
    path: PathBuf,
    apply_edit: ApplyEditFn,
}

impl DocumentProviders {
    pub fn new(store: WeakEntity<LspStore>, path: PathBuf, apply_edit: ApplyEditFn) -> Self {
        Self {
            store,
            path,
            apply_edit,
        }
    }

    /// Installs completion, hover, definition, signature help and code-action
    /// providers.
    pub fn install(self, editor: &Entity<EditorState>, cx: &mut App) {
        let this = Rc::new(self);
        editor.update(cx, |state, cx| {
            let lsp = state.lsp_mut();
            lsp.completion_provider = Some(this.clone());
            lsp.hover_provider = Some(this.clone());
            lsp.definition_provider = Some(this.clone());
            lsp.signature_help_provider = Some(this.clone());
            lsp.code_action_providers = vec![this];
            cx.notify();
        });
    }

    fn server(&self, cx: &mut App) -> Option<Arc<LanguageServer>> {
        let store = self.store.upgrade()?;
        store.update(cx, |store, cx| store.synced_server(&self.path, cx))
    }

    fn server_for(&self, text: &Rope, cx: &mut App) -> Option<Arc<LanguageServer>> {
        let store = self.store.upgrade()?;
        store.update(cx, |store, cx| store.synced_server_with_text(&self.path, text, cx))
    }

    fn text_document(&self) -> Value {
        json!({ "uri": path_to_uri(&self.path).as_str() })
    }
}

/// UTF-16 LSP range → the character-column range the editor expects.
fn to_editor_range(text: &Rope, range: lsp_types::Range) -> lsp_types::Range {
    let offsets = lsp_range_to_offsets(text, range);
    lsp_types::Range::new(
        text.offset_to_position(offsets.start),
        text.offset_to_position(offsets.end),
    )
}

fn convert_completion(text: &Rope, mut item: CompletionItem) -> CompletionItem {
    let is_snippet = item.insert_text_format == Some(InsertTextFormat::SNIPPET);
    let fix = |s: &str| {
        if is_snippet {
            snippet_to_placeholders(s)
        } else {
            s.to_string()
        }
    };
    item.text_edit = item.text_edit.map(|edit| match edit {
        CompletionTextEdit::Edit(mut edit) => {
            edit.range = to_editor_range(text, edit.range);
            edit.new_text = fix(&edit.new_text);
            CompletionTextEdit::Edit(edit)
        }
        CompletionTextEdit::InsertAndReplace(mut edit) => {
            edit.insert = to_editor_range(text, edit.insert);
            edit.replace = to_editor_range(text, edit.replace);
            edit.new_text = fix(&edit.new_text);
            CompletionTextEdit::InsertAndReplace(edit)
        }
    });
    item.insert_text = item.insert_text.as_deref().map(fix);
    item.insert_text_format = Some(InsertTextFormat::PLAIN_TEXT);
    item
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
        let empty = || CompletionResponse::Array(vec![]);
        let Some(server) = self.server_for(text, cx) else {
            return Task::ready(Ok(empty()));
        };
        let text = text.clone();
        let position = offset_to_lsp(&text, offset);
        let previous = offset
            .checked_sub(1)
            .and_then(|o| text.slice(o..offset).chars().next());
        let context = match previous {
            Some('.') => json!({ "triggerKind": 2, "triggerCharacter": "." }),
            _ if trigger.trigger_kind == lsp_types::CompletionTriggerKind::INVOKED => {
                json!({ "triggerKind": 1 })
            }
            _ => json!({ "triggerKind": 1 }),
        };
        let request = server.request_raw(
            "textDocument/completion",
            json!({
                "textDocument": self.text_document(),
                "position": position,
                "context": context,
            }),
        );
        cx.background_spawn(async move {
            let value = request.await?;
            let response: Option<CompletionResponse> = serde_json::from_value(value)?;
            let mut items = match response {
                Some(CompletionResponse::Array(items)) => items,
                Some(CompletionResponse::List(list)) => list.items,
                None => vec![],
            };
            items.sort_by(|a, b| {
                a.sort_text
                    .as_deref()
                    .unwrap_or(&a.label)
                    .cmp(b.sort_text.as_deref().unwrap_or(&b.label))
            });
            items.truncate(MAX_COMPLETIONS);
            let items = items
                .into_iter()
                .map(|item| convert_completion(&text, item))
                .collect();
            Ok(CompletionResponse::Array(items))
        })
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        let Some(ch) = new_text.chars().last() else {
            return false;
        };
        new_text.chars().count() == 1 && (ch == '.' || ch == '_' || ch.is_alphanumeric())
    }
}

impl HoverProvider for DocumentProviders {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<lsp_types::Hover>>> {
        let Some(server) = self.server_for(text, cx) else {
            return Task::ready(Ok(None));
        };
        let text = text.clone();
        let request = server.request_raw(
            "textDocument/hover",
            json!({
                "textDocument": self.text_document(),
                "position": offset_to_lsp(&text, offset),
            }),
        );
        cx.background_spawn(async move {
            let hover: Option<lsp_types::Hover> = serde_json::from_value(request.await?)?;
            Ok(hover.map(|mut hover| {
                hover.range = hover.range.map(|r| to_editor_range(&text, r));
                hover
            }))
        })
    }
}

impl DefinitionProvider for DocumentProviders {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let Some(server) = self.server_for(text, cx) else {
            return Task::ready(Ok(vec![]));
        };
        let text = text.clone();
        let word = text.word_range(offset);
        let request = server.request_raw(
            "textDocument/definition",
            json!({
                "textDocument": self.text_document(),
                "position": offset_to_lsp(&text, offset),
            }),
        );
        cx.background_spawn(async move {
            let value = request.await?;
            log::debug!("definition response: {value}");
            let response: Option<GotoDefinitionResponse> = serde_json::from_value(value)?;
            let origin = word.map(|r| {
                lsp_types::Range::new(
                    text.offset_to_position(r.start),
                    text.offset_to_position(r.end),
                )
            });
            Ok(definition_links(response, origin))
        })
    }
}

impl SignatureHelpProvider for DocumentProviders {
    fn trigger_characters(&self, cx: &App) -> (Vec<String>, Vec<String>) {
        self.store
            .upgrade()
            .and_then(|store| store.read(cx).features().signature_help.clone())
            .map(|triggers| (triggers.triggers, triggers.retriggers))
            .unwrap_or_default()
    }

    fn signature_help(
        &self,
        text: &Rope,
        offset: usize,
        context: lsp_types::SignatureHelpContext,
        cx: &mut App,
    ) -> Task<Result<Option<lsp_types::SignatureHelp>>> {
        let supported = self.store.upgrade().is_some_and(|store| store.read(cx).features().signature_help.is_some());
        let Some(server) = self.server_for(text, cx).filter(|_| supported) else {
            return Task::ready(Ok(None));
        };
        let request = server.request_raw(
            "textDocument/signatureHelp",
            json!({
                "textDocument": self.text_document(),
                "position": offset_to_lsp(text, offset),
                "context": context,
            }),
        );
        cx.background_spawn(async move { Ok(serde_json::from_value(request.await?)?) })
    }
}

/// Normalizes every shape of a definition response into links. Target ranges
/// stay in the server's UTF-16 coordinates; the workspace converts them once
/// the target file is loaded.
pub fn definition_links(
    response: Option<GotoDefinitionResponse>,
    origin: Option<lsp_types::Range>,
) -> Vec<LocationLink> {
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
        "sourcekit-lsp".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<CodeAction>>> {
        let Some(server) = self.server(cx) else {
            return Task::ready(Ok(vec![]));
        };
        // The editor is mid-update when it asks for actions; use the store's copy.
        let _ = state;
        let Some(text) = self
            .store
            .upgrade()
            .and_then(|store| store.read(cx).document_text(&self.path))
        else {
            return Task::ready(Ok(vec![]));
        };
        let lsp_range = lsp_types::Range::new(
            offset_to_lsp(&text, range.start),
            offset_to_lsp(&text, range.end),
        );
        let diagnostics: Vec<_> = self
            .store
            .upgrade()
            .map(|store| {
                store
                    .read(cx)
                    .diagnostics_for(&self.path)
                    .iter()
                    .filter(|d| {
                        d.range.start.line <= lsp_range.end.line
                            && d.range.end.line >= lsp_range.start.line
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let request = server.request_raw(
            "textDocument/codeAction",
            json!({
                "textDocument": self.text_document(),
                "range": lsp_range,
                "context": { "diagnostics": diagnostics },
            }),
        );
        cx.background_spawn(async move {
            let actions: Option<Vec<CodeActionOrCommand>> =
                serde_json::from_value(request.await?)?;
            Ok(actions
                .unwrap_or_default()
                .into_iter()
                .map(|action| match action {
                    CodeActionOrCommand::CodeAction(action) => action,
                    CodeActionOrCommand::Command(command) => CodeAction {
                        title: command.title.clone(),
                        command: Some(command),
                        ..Default::default()
                    },
                })
                .collect())
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
        if let Some(edit) = action.edit.clone() {
            (self.apply_edit)(edit, window, cx);
        }
        if let Some(command) = action.command
            && let Some(server) = self.server(cx)
        {
            let request = server.request_raw(
                "workspace/executeCommand",
                json!({ "command": command.command, "arguments": command.arguments }),
            );
            // Any resulting edit arrives as a `workspace/applyEdit` request.
            return cx.background_spawn(async move {
                request.await?;
                Ok(())
            });
        }
        Task::ready(Ok(()))
    }
}


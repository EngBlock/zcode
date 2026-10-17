use crate::EditorMode;
use anyhow::Result;
use gpui::{App, Context, EntityInputHandler, Pixels, Task, Window, px};
use lsp_types::{CompletionContext, CompletionItem, CompletionResponse, request::Completion};
use ropey::Rope;
use std::{cell::RefCell, ops::Range, rc::Rc};

use crate::{InputBaseState, ShowCompletions};

/// Display options for the LSP completion popover.
///
/// Accessed through [`super::Lsp::completion_menu`] so embedders can tweak the
/// popover without growing the [`InputBaseState`] API.
#[derive(Debug, Clone, Copy)]
pub struct CompletionMenuOptions {
    /// Maximum width of the popover.
    ///
    /// Defaults to 320 px, which is fine for most identifiers but can
    /// truncate longer labels. Widen this when hosting an editor that
    /// surfaces long completion labels.
    pub max_width: Pixels,
}

impl Default for CompletionMenuOptions {
    fn default() -> Self {
        Self {
            max_width: px(320.),
        }
    }
}

/// A trait for providing code completions based on the current input state and context.
pub trait CompletionProvider {
    /// Fetches completions based on the given byte offset.
    ///
    /// - The `offset` is in bytes of current cursor.
    ///
    /// textDocument/completion
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_completion
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>>;

    fn resolve_completions(
        &self,
        _completion_indices: Vec<usize>,
        _completions: Rc<RefCell<Box<[Completion]>>>,
        _: &mut App,
    ) -> Task<Result<bool>> {
        Task::ready(Ok(false))
    }

    /// Determines if the completion should be triggered based on the given byte offset.
    ///
    /// This is called on the main thread.
    fn is_completion_trigger(&self, offset: usize, new_text: &str, cx: &mut App) -> bool;

    /// `item` was just inserted: the provider may resolve it for edits it
    /// makes elsewhere (an auto-import). Called while the editor is being
    /// updated, so edits to it must be deferred.
    fn completion_inserted(&self, _item: &CompletionItem, _window: &mut Window, _cx: &mut App) {}
}

impl InputBaseState<EditorMode> {
    pub(crate) fn handle_completion_trigger(
        &mut self,
        _range: &Range<usize>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.completion_inserting {
            return;
        }

        let Some(provider) = self.extras.lsp.completion_provider.clone() else {
            return;
        };

        let new_offset = self.cursor();
        // Measure the inserted text in the current document. The replaced range
        // uses pre-edit coordinates, which preceding multi-cursor edits can
        // shift. The active caret ends immediately after the normalized input,
        // including selection replacements and IME commits.
        let Some(start) = new_offset.checked_sub(new_text.len()) else {
            return;
        };

        if !provider.is_completion_trigger(start, new_text, cx) {
            return;
        }

        // `trigger_start_offset` latches where the word a menu was opened for
        // begins, so later keystrokes refine the same query instead of starting
        // over at each character. It only describes this edit while the edit
        // continues that word: the document between the latch and the edit
        // must still read as a prefix of the last query. Deleting back into
        // the word keeps it; typing somewhere else, or into a document that
        // has since been replaced, starts a new query at this edit instead of
        // handing the provider text the user never typed as a prefix.
        let completion = &self.extras.context_menu_content.completion;
        let latched = completion.trigger_start_offset.filter(|&latched| {
            latched <= start
                && start <= latched + completion.query.len()
                && self.text.is_char_boundary(latched)
                && self.text.is_char_boundary(start)
                && completion
                    .query
                    .starts_with(self.text.slice(latched..start).to_string().as_str())
        });
        let start_offset = latched.unwrap_or(start);
        if new_offset < start_offset {
            return;
        }

        let query = self
            .text_for_range(
                self.range_to_utf16(&(start_offset..new_offset)),
                &mut None,
                window,
                cx,
            )
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        self.request_completions(
            provider,
            start_offset,
            query.clone(),
            CompletionContext {
                trigger_kind: lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER,
                trigger_character: Some(query),
            },
            window,
            cx,
        );
    }

    pub(crate) fn on_action_show_completions(
        &mut self,
        _: &ShowCompletions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_completions(window, cx);
    }

    /// Request completions at the caret and focus the editor, without editing text.
    ///
    /// The current identifier prefix is used to refine the menu. An empty
    /// prefix still asks the provider for suggestions. There is no default
    /// keybinding; applications can dispatch [`ShowCompletions`] in the
    /// `Input && mode == editor` context or call this method from a command.
    ///
    /// Disabled/readonly editors, IME preedit, and an already-open completion
    /// menu are left alone. A new request cancels the previous overlay request
    /// and inline suggestion, and closes the code-action menu.
    pub fn show_completions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled
            || self.readonly
            || self.completion_inserting
            || self.ime_marked_range.is_some()
            || self.extras.context_menu_content.completion.open
        {
            return;
        }
        let Some(provider) = self.extras.lsp.completion_provider.clone() else {
            return;
        };

        let offset = self.cursor();
        let word_len: usize = self
            .text
            .chars_at(offset)
            .reversed()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .map(char::len_utf8)
            .sum();
        let start_offset = offset - word_len;
        let query = self.text.slice(start_offset..offset).to_string();

        self.hide_context_menu(cx);
        self.focus(window, cx);

        self.request_completions(
            provider,
            start_offset,
            query,
            CompletionContext {
                trigger_kind: lsp_types::CompletionTriggerKind::INVOKED,
                trigger_character: None,
            },
            window,
            cx,
        );
    }

    /// Record the word a menu is for and ask `provider` about the caret.
    fn request_completions(
        &mut self,
        provider: Rc<dyn CompletionProvider>,
        start_offset: usize,
        query: String,
        completion_context: CompletionContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_offset = self.cursor();
        let revision = self.document_revision;
        self.extras.context_menu_task = Task::ready(Ok(()));
        self.extras.context_menu_content.code_action.open = false;
        self.extras
            .context_menu_content
            .completion
            .trigger_start_offset = Some(start_offset);
        self.extras
            .context_menu_content
            .completion
            .query
            .clone_from(&query);

        let provider_responses =
            provider.completions(&self.text, new_offset, completion_context, window, cx);
        self.extras.context_menu_task = cx.spawn_in(window, async move |editor, cx| {
            let mut completions: Vec<CompletionItem> = vec![];
            if let Some(provider_responses) = provider_responses.await.ok() {
                match provider_responses {
                    CompletionResponse::Array(items) => completions.extend(items),
                    CompletionResponse::List(list) => completions.extend(list.items),
                }
            }

            editor
                .update_in(cx, |editor, window, cx| {
                    if !editor.focus_handle.is_focused(window)
                        || editor.document_revision != revision
                        || editor.cursor() != new_offset
                        || editor.ime_marked_range.is_some()
                        || editor.disabled
                        || editor.readonly
                    {
                        return;
                    }

                    editor.extras.context_menu_content.completion.open = !completions.is_empty();
                    editor.extras.context_menu_content.completion.items = completions;
                    editor.extras.context_menu_content.completion.bump();
                    cx.notify();
                })
                .ok();

            Ok(())
        });
    }

    pub(crate) fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.extras.context_menu_content.completion.open = false;
        self.extras.context_menu_content.code_action.open = false;
        self.extras.context_menu_task = Task::ready(Ok(()));
        cx.notify();
    }

    pub(crate) fn is_context_menu_open(&self, _cx: &gpui::App) -> bool {
        self.extras.context_menu_content.completion.open
            || self.extras.context_menu_content.code_action.open
    }

    pub(crate) fn handle_action_for_context_menu(
        &mut self,
        action: Box<dyn gpui::Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let closes_overlay =
            crate::Enter::is_primary(&*action) || action.partial_eq(&crate::Escape);
        let kind = if self.extras.context_menu_content.completion.open {
            Some(super::InputOverlayKind::Completion)
        } else if self.extras.context_menu_content.code_action.open {
            Some(super::InputOverlayKind::CodeAction)
        } else {
            None
        };
        // With no menu open, Escape closes signature help.
        if kind.is_none() && self.extras.signature_help.is_some() && action.partial_eq(&crate::Escape) {
            self.dismiss_signature_help(cx);
            return true;
        }
        let Some((kind, handler)) = kind.zip(self.overlay_action_handler.clone()) else {
            return false;
        };
        let handled = handler(kind, action, window, cx);
        if handled && closes_overlay {
            self.hide_context_menu(cx);
        }
        handled
    }
}

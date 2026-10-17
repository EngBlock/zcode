use super::*;
use crate::EditorMode;
use std::ops::Range;

use lsp_types::{CompletionItem, Hover};

use crate::{Anchor, AnchorBias};

/// A snippet's tab stops Tab hasn't visited, and the snippet's extent (Tab
/// outside it ends the session).
#[derive(Clone, Debug)]
pub struct SnippetSession {
    pub(crate) stops: Vec<(Anchor, Anchor)>,
    pub(crate) bounds: (Anchor, Anchor),
}

#[derive(Clone, Debug, Default)]
pub struct CompletionMenuState {
    pub open: bool,
    pub trigger_start_offset: Option<usize>,
    pub query: String,
    pub items: Vec<CompletionItem>,
    revision: u64,
}

impl CompletionMenuState {
    /// Bumped whenever the content changes.
    ///
    /// A renderer that mirrors this menu compares revisions to decide whether
    /// to rebuild, so it never has to compare the item list itself.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

#[derive(Clone, Debug, Default)]
pub struct CodeActionMenuState {
    pub open: bool,
    pub items: Vec<CodeActionItem>,
    revision: u64,
}

impl CodeActionMenuState {
    /// Bumped whenever the content changes. See [`CompletionMenuState::revision`].
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(super) fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

#[derive(Clone, Debug)]
pub struct HoverPopoverState {
    pub symbol_range: Range<usize>,
    pub hover: Hover,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ContextMenuContent {
    pub(crate) completion: CompletionMenuState,
    pub(crate) code_action: CodeActionMenuState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputOverlayKind {
    Completion,
    CodeAction,
}

impl InputBaseState<EditorMode> {
    pub fn present_completion_items(
        &mut self,
        trigger_start_offset: usize,
        query: impl Into<String>,
        items: Vec<CompletionItem>,
        cx: &mut Context<Self>,
    ) {
        self.extras
            .context_menu_content
            .completion
            .trigger_start_offset = Some(trigger_start_offset);
        self.extras.context_menu_content.completion.query = query.into();
        self.extras.context_menu_content.completion.items = items;
        self.extras.context_menu_content.completion.open =
            !self.extras.context_menu_content.completion.items.is_empty();
        self.extras.context_menu_content.completion.bump();
        cx.notify();
    }

    pub fn present_code_actions(&mut self, items: Vec<CodeActionItem>, cx: &mut Context<Self>) {
        self.extras.context_menu_content.code_action.items = items;
        self.extras.context_menu_content.code_action.open = !self
            .extras
            .context_menu_content
            .code_action
            .items
            .is_empty();
        self.extras.context_menu_content.code_action.bump();
        cx.notify();
    }

    pub fn present_hover(
        &mut self,
        symbol_range: Range<usize>,
        hover: Hover,
        cx: &mut Context<Self>,
    ) {
        self.extras.hover_popover = Some(HoverPopoverState {
            symbol_range,
            hover,
        });
        cx.notify();
    }

    pub fn present_diagnostic(
        &mut self,
        diagnostic: crate::DiagnosticEntry,
        cx: &mut Context<Self>,
    ) {
        self.diagnostic_popover = Some(Rc::new(diagnostic));
        cx.notify();
    }

    pub fn clear_diagnostic_popover(&mut self, cx: &mut Context<Self>) {
        if self.diagnostic_popover.take().is_some() {
            cx.notify();
        }
    }

    pub fn route_overlay_action(
        &mut self,
        action: Box<dyn gpui::Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.handle_action_for_context_menu(action, window, cx)
    }

    pub fn set_overlay_action_handler(
        &mut self,
        handler: impl Fn(
            InputOverlayKind,
            Box<dyn gpui::Action>,
            &mut Window,
            &mut Context<InputBaseState<EditorMode>>,
        ) -> bool
        + 'static,
    ) {
        self.overlay_action_handler = Some(Rc::new(handler));
    }

    pub fn has_overlay_action_handler(&self) -> bool {
        self.overlay_action_handler.is_some()
    }

    pub fn dismiss_completion_overlay(&mut self, cx: &mut Context<Self>) {
        if self.extras.context_menu_content.completion.open {
            self.extras.context_menu_content.completion.open = false;
            cx.notify();
        }
    }

    pub fn dismiss_code_action_overlay(&mut self, cx: &mut Context<Self>) {
        if self.extras.context_menu_content.code_action.open {
            self.extras.context_menu_content.code_action.open = false;
            cx.notify();
        }
    }

    pub fn insert_completion(
        &mut self,
        item: &CompletionItem,
        fallback_range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut range = fallback_range;
        let mut new_text = item.label.clone();
        if let Some(edit) = item.text_edit.as_ref() {
            match edit {
                lsp_types::CompletionTextEdit::Edit(edit) => {
                    new_text.clone_from(&edit.new_text);
                    range = self.text.position_to_offset(&edit.range.start)
                        ..self.text.position_to_offset(&edit.range.end);
                }
                lsp_types::CompletionTextEdit::InsertAndReplace(edit) => {
                    new_text.clone_from(&edit.new_text);
                    range = self.text.position_to_offset(&edit.replace.start)
                        ..self.text.position_to_offset(&edit.replace.end);
                }
            }
        } else if let Some(insert_text) = item.insert_text.as_ref() {
            new_text.clone_from(insert_text);
            range = range.end..range.end;
        }
        if item.insert_text_format == Some(lsp_types::InsertTextFormat::SNIPPET) {
            self.insert_snippet(range, &new_text, window, cx);
        } else {
            self.end_snippet();
            self.completion_inserting = true;
            let range = self.range_to_utf16(&range);
            self.replace_text_in_range_silent(Some(range), &new_text, window, cx);
            self.completion_inserting = false;
        }
        if let Some(provider) = self.extras.lsp.completion_provider.clone() {
            provider.completion_inserted(item, window, cx);
        }
        self.focus(window, cx);
    }

    /// Replaces `range` (byte offsets) with an LSP snippet
    /// ([`crate::snippet::expand`]): placeholders as `<#…#>` tokens, the
    /// first tab stop selected, Tab ([`Self::next_snippet_stop`]) visiting
    /// the rest.
    pub fn insert_snippet(&mut self, range: Range<usize>, snippet: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.end_snippet();
        let expansion = crate::snippet::expand(snippet);
        let start = range.start.min(self.text.len());
        self.completion_inserting = true;
        let utf16 = self.range_to_utf16(&range);
        self.replace_text_in_range_silent(Some(utf16), &expansion.text, window, cx);
        self.completion_inserting = false;
        let mut stops = expansion.stops.iter().map(|r| start + r.start..start + r.end);
        if let Some(first) = stops.next() {
            self.set_selected_range(first, cx);
        }
        let rest: Vec<(Anchor, Anchor)> =
            stops.map(|r| (self.anchor(r.start, AnchorBias::Left), self.anchor(r.end, AnchorBias::Right))).collect();
        if !rest.is_empty() {
            let bounds = (self.anchor(start, AnchorBias::Left), self.anchor(start + expansion.text.len(), AnchorBias::Right));
            self.extras.snippet = Some(SnippetSession { stops: rest, bounds });
        }
    }

    /// Selects the inserted snippet's next tab stop. `false` when there is
    /// none, or the cursor has left the snippet (which ends it).
    pub fn next_snippet_stop(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.extras.snippet.as_mut() else { return false };
        let bounds = session.bounds;
        let selected = self.selected_range();
        let cursor = selected.start.min(selected.end);
        let inside = match (self.anchor_offset(bounds.0), self.anchor_offset(bounds.1)) {
            (Some(start), Some(end)) => (start..=end).contains(&cursor),
            _ => false,
        };
        let Some(session) = self.extras.snippet.as_mut().filter(|s| inside && !s.stops.is_empty()) else {
            self.end_snippet();
            return false;
        };
        let (start, end) = session.stops.remove(0);
        let range = self.anchor_offset(start).zip(self.anchor_offset(end));
        for anchor in [start, end] {
            self.remove_anchor(anchor);
        }
        if self.extras.snippet.as_ref().is_some_and(|s| s.stops.is_empty()) {
            self.end_snippet();
        }
        match range {
            Some((start, end)) => {
                self.set_selected_range(start..end.max(start), cx);
                true
            }
            None => self.next_snippet_stop(cx),
        }
    }

    /// Whether Tab would move to a snippet's next tab stop.
    pub fn has_snippet_stops(&self) -> bool {
        self.extras.snippet.as_ref().is_some_and(|s| !s.stops.is_empty())
    }

    /// Forgets the snippet's remaining tab stops.
    pub fn end_snippet(&mut self) {
        if let Some(session) = self.extras.snippet.take() {
            for (start, end) in session.stops {
                self.remove_anchor(start);
                self.remove_anchor(end);
            }
            self.remove_anchor(session.bounds.0);
            self.remove_anchor(session.bounds.1);
        }
    }

    #[doc(hidden)]
    pub fn completion_menu_state(&self) -> &CompletionMenuState {
        &self.extras.context_menu_content.completion
    }

    #[doc(hidden)]
    pub fn code_action_menu_state(&self) -> &CodeActionMenuState {
        &self.extras.context_menu_content.code_action
    }

    pub fn hover_popover(&self) -> Option<&HoverPopoverState> {
        self.extras.hover_popover.as_ref()
    }

    pub fn dismiss_lsp_overlays(&mut self, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        self.clear_hover_state(cx);
    }
}

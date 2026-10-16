//! Undo, redo, transactions and read-only mode, for callers outside the
//! keymap.
//!
//! The engine already records every edit as a transaction (typing coalesces,
//! a multi-cursor edit is one entry). These make that reachable: an outline
//! view that edits the text undoes through the same history without having
//! to dispatch an action to a focused editor, and a command made of several
//! edits ([`EditorState::transact`]) undoes in one step, or doesn't run at
//! all in a read-only editor.

use std::ops::Range;

use gpui::{Context, SharedString, Window};

use crate::EditorState;
use crate::cursor::CursorSelection;

impl EditorState {
    /// Undoes the last transaction, as ⌘Z does. Does nothing when the editor
    /// is read-only or there is nothing to undo.
    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replay_history(true, window, cx);
    }

    /// Redoes the last undone transaction, as ⇧⌘Z does.
    pub fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replay_history(false, window, cx);
    }

    pub fn can_undo(&self) -> bool {
        self.undo_manager.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo_manager.can_redo()
    }

    /// Runs `f` as one undoable transaction: every edit it makes (through
    /// [`Self::edit`], [`Self::replace`], [`Self::insert`] or any other
    /// editing method) undoes and redoes together, and undo puts the
    /// selections back where they were before `f`.
    ///
    /// Returns `None` without running `f` when the editor is read-only, so
    /// commands that change the text can be written once and simply do
    /// nothing in a read-only buffer. Transactions nest: an inner one joins
    /// the outer.
    pub fn transact<R>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) -> R,
    ) -> Option<R> {
        if !self.is_editable() {
            return None;
        }
        let before: Vec<CursorSelection> = self.selections.iter().copied().collect();
        self.undo_manager.begin_transaction();
        self.undo_manager
            .record_selections(before.clone(), before.clone());
        let result = f(self, window, cx);
        let after = self.selections.iter().copied().collect();
        self.undo_manager.record_selections(before, after);
        self.undo_manager.commit_transaction();
        Some(result)
    }

    /// Replaces each range (byte offsets in the current text, not
    /// overlapping) with its text, as one edit: one undo entry, one change
    /// event. Unlike the user's typing this works in a read-only editor; wrap
    /// it in [`Self::transact`] for a command that should respect read-only
    /// mode.
    pub fn edit(
        &mut self,
        edits: impl IntoIterator<Item = (Range<usize>, impl Into<SharedString>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut edits: Vec<(Range<usize>, String)> = edits
            .into_iter()
            .map(|(range, text)| (range, text.into().to_string()))
            .collect();
        if edits.is_empty() {
            return;
        }
        edits.sort_by_key(|(range, _)| range.start);
        self.with_edits_allowed(|this| {
            this.undo_manager.begin_transaction();
            this.replace_text_in_ranges(&edits, window, cx);
            this.undo_manager.commit_transaction();
        });
    }

    /// Whether the user's edits are refused. Read-only editors still select,
    /// copy, search, navigate and show language features.
    pub fn is_read_only(&self) -> bool {
        self.readonly
    }

    /// Makes the editor read-only, or editable again. While read-only, typing,
    /// paste, cut, undo, redo and [`Self::transact`] do nothing; the
    /// programmatic setters ([`Self::set_value`], [`Self::edit`]) still work,
    /// so a file can be reloaded from disk.
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        self.set_readonly(read_only, cx);
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use crate::test_support::open_editor;

    #[gpui::test]
    fn a_transaction_undoes_and_redoes_as_one_step(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "one\ntwo\n");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                assert!(!state.can_undo());
                state.set_selected_range(4..4, cx);
                let ran = state.transact(window, cx, |state, window, cx| {
                    state.edit([(0..3, "ONE")], window, cx);
                    state.edit([(4..7, "TWO"), (8..8, "three\n")], window, cx);
                    state.set_selected_range(0..0, cx);
                });
                assert_eq!(ran, Some(()));
                assert_eq!(state.text().to_string(), "ONE\nTWO\nthree\n");
                assert!(state.can_undo());
                state.undo(window, cx);
                assert_eq!(state.text().to_string(), "one\ntwo\n");
                assert_eq!(state.selected_range(), 4..4);
                assert!(!state.can_undo() && state.can_redo());
                state.redo(window, cx);
                assert_eq!(state.text().to_string(), "ONE\nTWO\nthree\n");
                assert_eq!(state.selected_range(), 0..0);
            });
        });
    }

    #[gpui::test]
    fn read_only_refuses_transactions_undo_and_redo(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "abc");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.edit([(3..3, "d")], window, cx);
                state.set_read_only(true, cx);
                assert!(state.is_read_only());
                assert_eq!(state.transact(window, cx, |_, _, _| ()), None);
                state.undo(window, cx);
                assert_eq!(state.text().to_string(), "abcd");
                // Programmatic edits still work, for reloads.
                state.edit([(0..1, "A")], window, cx);
                assert_eq!(state.text().to_string(), "Abcd");
                state.set_read_only(false, cx);
                state.undo(window, cx);
                assert_eq!(state.text().to_string(), "abcd");
            });
        });
    }
}

//! Undo, redo, transactions and read-only mode, for callers outside the
//! keymap.
//!
//! The engine already records every edit as a transaction (typing coalesces,
//! a multi-cursor edit is one entry). These make that reachable: an outline
//! view that edits the text undoes through the same history without having
//! to dispatch an action to a focused editor, and a command made of several
//! edits ([`EditorState::transact`]) undoes in one step, or doesn't run at
//! all in a read-only editor.
//!
//! Transactions have ids, so an editor can join an edit to one it made
//! earlier and undo exactly that one ([`EditorState::undo_transaction`]),
//! and an editor can hand its ⌘Z and ⇧⌘Z to someone else
//! ([`EditorState::set_external_history`]): a multibuffer keeps one undo
//! step per edit across the files its excerpts edit, made of the
//! transactions it added to each file's history.

use std::ops::Range;

use gpui::{Context, EventEmitter, SharedString, Window};

use crate::cursor::CursorSelection;
use crate::{EditorState, InputBaseState, InputModeKind};

/// A transaction in an editor's undo history; the same through undo and redo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransactionId(pub(crate) u64);

/// ⌘Z or ⇧⌘Z in an editor whose history is kept elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryRequest {
    Undo,
    Redo,
}

impl<M: InputModeKind> EventEmitter<HistoryRequest> for InputBaseState<M> {}

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

    /// The transaction ⌘Z would undo.
    pub fn last_transaction(&self) -> Option<TransactionId> {
        self.undo_manager.last_id().map(TransactionId)
    }

    /// The transaction ⇧⌘Z would redo.
    pub fn next_redo_transaction(&self) -> Option<TransactionId> {
        self.undo_manager.next_redo_id().map(TransactionId)
    }

    /// Like [`Self::edit`], but the edit joins `transaction` when that is
    /// still the last one, so they undo together. Returns the transaction
    /// the edit is in.
    pub fn edit_joining(
        &mut self,
        transaction: Option<TransactionId>,
        edits: impl IntoIterator<Item = (Range<usize>, impl Into<SharedString>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<TransactionId> {
        self.undo_manager.set_join(transaction.map(|t| t.0));
        self.edit(edits, window, cx);
        self.undo_manager.set_join(None);
        self.last_transaction()
    }

    /// Undoes `transaction` if it's the last one; whether it did.
    pub fn undo_transaction(&mut self, transaction: TransactionId, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.last_transaction() != Some(transaction) || self.external_history {
            return false;
        }
        self.undo(window, cx);
        self.next_redo_transaction() == Some(transaction)
    }

    /// Redoes `transaction` if it's the next to redo; whether it did.
    pub fn redo_transaction(&mut self, transaction: TransactionId, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.next_redo_transaction() != Some(transaction) || self.external_history {
            return false;
        }
        self.redo(window, cx);
        self.last_transaction() == Some(transaction)
    }

    /// Hands ⌘Z and ⇧⌘Z (and [`Self::undo`], [`Self::redo`]) to whoever
    /// keeps this editor's history: they emit [`HistoryRequest`] instead of
    /// replaying the editor's own. The editor still records its edits, so
    /// [`Self::last_transaction`] tells one edit from the next.
    pub fn set_external_history(&mut self, external: bool) {
        self.external_history = external;
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

    /// A number that changes with every change of the text, for telling
    /// whether an answer about it is stale.
    pub fn text_revision(&self) -> u64 {
        self.document_revision
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

    use super::HistoryRequest;
    use crate::test_support::open_editor;

    #[gpui::test]
    fn edits_join_a_transaction_and_undo_it_by_id(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "abc");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                let first = state.edit_joining(None, [(3..3, "d")], window, cx).unwrap();
                // Joined while it's the last transaction: one undo step.
                assert_eq!(state.edit_joining(Some(first), [(0..0, ">")], window, cx), Some(first));
                state.edit([(0..0, "!")], window, cx);
                let other = state.last_transaction().unwrap();
                assert_ne!(other, first);
                // Not the last one any more: a new transaction.
                let third = state.edit_joining(Some(first), [(0..0, "?")], window, cx).unwrap();
                assert_ne!(third, first);
                assert!(!state.undo_transaction(first, window, cx), "only the last transaction undoes");
                assert!(state.undo_transaction(third, window, cx));
                assert!(state.undo_transaction(other, window, cx));
                assert!(state.undo_transaction(first, window, cx));
                assert_eq!(state.text().to_string(), "abc");
                assert!(state.redo_transaction(first, window, cx));
                assert_eq!(state.text().to_string(), ">abcd");
                assert_eq!(state.last_transaction(), Some(first), "the id survives undo and redo");
            });
        });
    }

    #[gpui::test]
    fn an_external_history_is_asked_to_undo(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "abc");
        let requests = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
        let _subscription = cx.update(|_, cx| {
            let requests = requests.clone();
            cx.subscribe(&editor, move |_, request: &HistoryRequest, _| requests.borrow_mut().push(*request))
        });
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_external_history(true);
                state.focus(window, cx);
                state.edit([(3..3, "d")], window, cx);
                assert!(state.last_transaction().is_some(), "edits are still recorded");
            })
        });
        cx.simulate_keystrokes("cmd-z cmd-shift-z");
        cx.update(|window, cx| editor.update(cx, |state, cx| state.undo(window, cx)));
        cx.run_until_parked();
        assert_eq!(editor.read_with(&cx, |state, _| state.text().to_string()), "abcd");
        assert_eq!(*requests.borrow(), [HistoryRequest::Undo, HistoryRequest::Redo, HistoryRequest::Undo]);
    }

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

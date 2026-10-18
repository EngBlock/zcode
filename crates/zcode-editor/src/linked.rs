//! Linked editors: one buffer shown in several views.
//!
//! Two editor panes on the same file are two editors, each with its own
//! selections, scroll position, folds, soft wrap, search and popovers. Linking
//! them makes them one buffer:
//!
//! * every edit applied in one view (typing, commands, undo and redo,
//!   `set_value`) is applied to the others at once, inside the same update,
//!   so their texts are equal whenever anyone looks;
//! * they share one undo history, so ⌘Z in any view undoes the last edit
//!   made in any of them, and puts the cursors back in the view that undoes;
//! * they share one anchor table, so an anchor made in one view resolves in
//!   the others (a breakpoint anchored through one pane draws in both) and
//!   moves once per edit, whichever view made it.
//!
//! A view applying another's edit keeps its own cursors (moved past the
//! edit), records nothing in the history and leaves the anchors alone: the
//! view that made the edit did both. Views hold each other weakly, so closing
//! one leaves the others linked, and the last one is the buffer.

use std::ops::Range;

use gpui::{App, Context, Entity, Window};

use crate::cursor::CursorSelection;
use crate::{EditorState, InputBaseState, InputModeKind};

impl<M: InputModeKind> InputBaseState<M> {
    /// Reports an edit this editor just applied to its text, and applies it
    /// to the views linked to it.
    pub(crate) fn buffer_edited(&mut self, range: &Range<usize>, new_text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(journal) = &self.edit_journal {
            journal.record(range, new_text);
        }
        if !self.quiet_edits {
            cx.emit(crate::BufferEdit { range: range.clone(), new_text: new_text.to_string() });
        }
        if self.applying_linked_edit || self.linked_views.is_empty() {
            return;
        }
        self.linked_views.retain(|view| view.upgrade().is_some());
        for view in self.linked_views.clone() {
            view.update(cx, |view, cx| view.apply_linked_edit(range.clone(), new_text, window, cx)).ok();
        }
    }

    /// Applies an edit a linked view made: like any edit for this view's
    /// display (wrapping, folds, highlighting, decorations, search), but out
    /// of the history and the anchors, which are shared and already updated.
    fn apply_linked_edit(&mut self, range: Range<usize>, new_text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let selections: Vec<CursorSelection> = self.selections.iter().copied().collect();
        self.applying_linked_edit = true;
        let edits = [(range.clone(), new_text.to_string())];
        self.with_edits_allowed(|this| this.replace_text_in_ranges(&edits, window, cx));
        self.applying_linked_edit = false;
        // This view's cursors stay where they were in the text: before text
        // inserted at them, at the start of text replaced around them.
        let len = self.text.len();
        let moved = selections
            .into_iter()
            .map(|mut selection| {
                selection.start = map_offset(selection.start, &range, new_text.len()).min(len);
                selection.end = map_offset(selection.end, &range, new_text.len()).min(len);
                selection
            })
            .collect();
        self.selections.replace_all(moved);
        self.selections.merge_overlapping();
        cx.notify();
    }
}

impl EditorState {
    /// Applies an edit made for this editor rather than by its user (a
    /// multibuffer's excerpt following its file): like [`Self::edit`], but
    /// the cursors stay where they were in the text, as a linked view's do.
    pub(crate) fn edit_keeping_selections(&mut self, range: Range<usize>, new_text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let selections: Vec<CursorSelection> = self.selections.iter().copied().collect();
        self.edit([(range.clone(), new_text.to_string())], window, cx);
        let len = self.text.len();
        let moved = selections
            .into_iter()
            .map(|mut selection| {
                selection.start = map_offset(selection.start, &range, new_text.len()).min(len);
                selection.end = map_offset(selection.end, &range, new_text.len()).min(len);
                selection
            })
            .collect();
        self.selections.replace_all(moved);
        self.selections.merge_overlapping();
    }
}

pub(crate) fn map_offset(offset: usize, range: &Range<usize>, new_len: usize) -> usize {
    if offset <= range.start {
        offset
    } else if offset >= range.end {
        offset - range.len() + new_len
    } else {
        range.start
    }
}

impl EditorState {
    /// Makes `view` another view of `buffer`'s text: `view` takes the text,
    /// and from now on the two (and every view already linked to `buffer`)
    /// apply each other's edits and share undo history and anchors. `view`
    /// should be a fresh editor: its own history and anchors are dropped.
    pub fn link(buffer: &Entity<EditorState>, view: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
        let (text, history, anchors, mut group) = {
            let state = buffer.read(cx);
            (state.text.to_string(), state.undo_manager.clone(), state.anchors.clone(), state.linked_views.clone())
        };
        group.retain(|member| member.upgrade().is_some_and(|member| member != *view));
        group.push(buffer.downgrade());
        view.update(cx, |view, cx| {
            view.set_value(text, window, cx);
            view.undo_manager = history;
            view.anchors = anchors;
            view.linked_views = group.clone();
        });
        for member in group {
            member
                .update(cx, |member, _| {
                    if !member.linked_views.iter().any(|v| v == &view.downgrade()) {
                        member.linked_views.push(view.downgrade());
                    }
                })
                .ok();
        }
    }

    /// The other views of this editor's buffer that are still open.
    pub fn linked_views(&self) -> Vec<Entity<EditorState>> {
        self.linked_views.iter().filter_map(|view| view.upgrade()).collect()
    }

    /// Whether `other` shows the same buffer: one history, one anchor table.
    pub fn shares_buffer_with(&self, other: &EditorState) -> bool {
        self.undo_manager.shares(&other.undo_manager) && self.anchors.shares(&other.anchors)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};

    use crate::test_support::open_editor;
    use crate::{BufferEdit, EditorState};

    /// The editor of `open_editor` and a second view linked to it.
    fn two_views(cx: &mut TestAppContext, text: &str) -> (Entity<EditorState>, Entity<EditorState>, VisualTestContext) {
        let (a, mut cx) = open_editor(cx, text);
        let b = cx.update(|window, cx| {
            let b = cx.new(|cx| EditorState::new(window, cx).line_number(true));
            EditorState::link(&a, &b, window, cx);
            b
        });
        (a, b, cx)
    }

    fn text(editor: &Entity<EditorState>, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| editor.read(cx).text().to_string())
    }

    #[gpui::test]
    fn an_edit_in_one_view_reaches_the_other_and_undoes_from_either(cx: &mut TestAppContext) {
        let (a, b, mut cx) = two_views(cx, "one\ntwo\n");
        assert_eq!(text(&b, &mut cx), "one\ntwo\n");
        cx.update(|window, cx| {
            b.update(cx, |b, cx| b.set_selected_range(6..6, cx));
            a.update(cx, |a, cx| a.edit([(0..0, "zero\n")], window, cx));
        });
        assert_eq!(text(&b, &mut cx), "zero\none\ntwo\n");
        cx.update(|window, cx| {
            // b's cursor stayed on "two", moved past the insertion.
            assert_eq!(b.read(cx).selected_range(), 11..11);
            assert!(a.read(cx).shares_buffer_with(b.read(cx)));
            assert!(b.read(cx).can_undo());
            b.update(cx, |b, cx| b.undo(window, cx));
        });
        assert_eq!(text(&a, &mut cx), "one\ntwo\n");
        cx.update(|window, cx| {
            assert!(!a.read(cx).can_undo() && a.read(cx).can_redo());
            a.update(cx, |a, cx| a.redo(window, cx));
        });
        assert_eq!(text(&b, &mut cx), "zero\none\ntwo\n");
    }

    #[gpui::test]
    fn a_transaction_in_one_view_undoes_in_one_step_from_the_other(cx: &mut TestAppContext) {
        let (a, b, mut cx) = two_views(cx, "abc\ndef\n");
        cx.update(|window, cx| {
            a.update(cx, |a, cx| {
                a.transact(window, cx, |a, window, cx| {
                    a.edit([(0..3, "ABC")], window, cx);
                    a.edit([(4..7, "DEF")], window, cx);
                });
            });
        });
        assert_eq!(text(&b, &mut cx), "ABC\nDEF\n");
        cx.update(|window, cx| b.update(cx, |b, cx| b.undo(window, cx)));
        assert_eq!(text(&a, &mut cx), "abc\ndef\n");
        cx.update(|_, cx| assert!(!b.read(cx).can_undo()));
    }

    #[gpui::test]
    fn anchors_are_shared_and_come_back_with_undo_in_either_view(cx: &mut TestAppContext) {
        let (a, b, mut cx) = two_views(cx, "one\ntwo\nthree\n");
        cx.update(|window, cx| {
            let line = a.update(cx, |a, _| a.line_anchor(2));
            assert_eq!(b.read(cx).anchor_row(line), Some(2));
            // Deleting the line in b deletes the anchor; inserting above moves it once.
            b.update(cx, |b, cx| b.edit([(8..14, "")], window, cx));
            assert_eq!(a.read(cx).anchor_row(line), None);
            a.update(cx, |a, cx| a.undo(window, cx));
            assert_eq!(b.read(cx).anchor_row(line), Some(2));
            b.update(cx, |b, cx| b.edit([(0..0, "zero\n")], window, cx));
            assert_eq!(a.read(cx).anchor_row(line), Some(3));
            assert_eq!(b.read(cx).anchor_row(line), Some(3));
        });
    }

    #[gpui::test]
    fn typing_and_reloads_reach_every_view_and_each_reports_its_edits(cx: &mut TestAppContext) {
        let (a, b, mut cx) = two_views(cx, "let x = 1\n");
        let edits = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
        let _subscription = cx.update(|_, cx| {
            let edits = edits.clone();
            cx.subscribe(&b, move |_, edit: &BufferEdit, _| edits.borrow_mut().push(edit.clone()))
        });
        cx.update(|window, cx| {
            a.update(cx, |a, cx| {
                a.set_selected_range(9..9, cx);
                a.focus(window, cx);
            });
        });
        cx.simulate_input("0");
        assert_eq!(text(&b, &mut cx), "let x = 10\n");
        cx.update(|window, cx| a.update(cx, |a, cx| a.set_value("reloaded\n", window, cx)));
        assert_eq!(text(&b, &mut cx), "reloaded\n");
        cx.update(|_, cx| assert!(!b.read(cx).can_undo(), "a reload clears the shared history"));
        assert_eq!(edits.borrow().len(), 2, "{:?}", edits.borrow());
    }

    #[gpui::test]
    fn a_closed_view_drops_out_of_the_group(cx: &mut TestAppContext) {
        let (a, b, mut cx) = two_views(cx, "abc");
        let c = cx.update(|window, cx| {
            let c = cx.new(|cx| EditorState::new(window, cx));
            EditorState::link(&b, &c, window, cx);
            c
        });
        cx.update(|_, cx| {
            assert_eq!(a.read(cx).linked_views().len(), 2);
            assert_eq!(c.read(cx).linked_views().len(), 2);
        });
        drop(b);
        cx.run_until_parked();
        cx.update(|window, cx| {
            a.update(cx, |a, cx| a.edit([(3..3, "d")], window, cx));
            assert_eq!(c.read(cx).text().to_string(), "abcd");
            assert_eq!(a.read(cx).linked_views(), vec![c.clone()]);
        });
    }
}

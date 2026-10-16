//! Each change to the text, as an event, and ranges the user can't edit.
//!
//! [`BufferEdit`] reports every replacement the engine applies, typing,
//! commands, undo and redo, and `set_value` (as one whole-text edit), in
//! order and in the coordinates of the text just before it. A multibuffer
//! forwards them to the files its excerpts come from; a language client can
//! send them as incremental changes.
//!
//! Read-only ranges refuse the user's edits that touch them (a multibuffer's
//! excerpt separators, generated code), while text can still be inserted at
//! their edges. Like read-only mode, they don't limit the programmatic APIs
//! or undo. They follow edits.

use std::ops::Range;

use gpui::{Context, EventEmitter};

use crate::{EditorState, InputBaseState, InputModeKind};

/// One replacement of the text: `range` (in the text before it) became
/// `new_text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferEdit {
    pub range: Range<usize>,
    pub new_text: String,
}

impl<M: InputModeKind> EventEmitter<BufferEdit> for InputBaseState<M> {}

#[derive(Default)]
pub(crate) struct ReadOnlyRanges {
    ranges: Vec<Range<usize>>,
}

impl ReadOnlyRanges {
    /// Whether an edit of `range` touches a read-only range: replaces any of
    /// it, or inserts strictly inside it.
    pub(crate) fn refuses(&self, range: &Range<usize>) -> bool {
        self.ranges.iter().any(|protected| {
            if range.is_empty() {
                protected.start < range.start && range.start < protected.end
            } else {
                range.start < protected.end && protected.start < range.end
            }
        })
    }

    pub(crate) fn adjust_for_edit(&mut self, edited: &Range<usize>, new_len: usize) {
        if self.ranges.is_empty() {
            return;
        }
        self.ranges.retain_mut(|range| {
            *range = crate::decorations::adjust_range_for_edit(range, edited, new_len);
            range.start < range.end
        });
    }
}

impl EditorState {
    /// Makes `ranges` read-only for the user, replacing the previous ones.
    pub fn set_read_only_ranges(&mut self, ranges: Vec<Range<usize>>, cx: &mut Context<Self>) {
        let len = self.text.len();
        self.read_only_ranges.ranges = ranges
            .into_iter()
            .map(|range| range.start.min(len)..range.end.min(len))
            .filter(|range| range.start < range.end)
            .collect();
        cx.notify();
    }

    /// The read-only ranges as they stand after edits.
    pub fn read_only_ranges(&self) -> &[Range<usize>] {
        &self.read_only_ranges.ranges
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::TestAppContext;

    use super::*;
    use crate::test_support::open_editor;

    #[gpui::test]
    fn every_change_is_reported_in_order(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "abc\n");
        let edits = Rc::new(RefCell::new(vec![]));
        let _subscription = cx.update(|_, cx| {
            let edits = edits.clone();
            cx.subscribe(&editor, move |_, edit: &BufferEdit, _| edits.borrow_mut().push(edit.clone()))
        });
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.focus(window, cx);
                state.set_selected_range(1..1, cx);
            })
        });
        cx.simulate_keystrokes("x backspace");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.edit([(3..3, "!"), (0..0, ">")], window, cx);
                state.undo(window, cx);
                state.set_value("new", window, cx);
            })
        });
        cx.run_until_parked();
        let edit = |range: Range<usize>, text: &str| BufferEdit { range, new_text: text.into() };
        assert_eq!(
            *edits.borrow(),
            [
                edit(1..1, "x"),
                edit(1..2, ""),
                edit(3..3, "!"),
                edit(0..0, ">"),
                edit(0..1, ""),
                edit(3..4, ""),
                edit(0..4, "new"),
            ]
        );
    }

    #[gpui::test]
    fn read_only_ranges_refuse_the_users_edits_only(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "one\n---\ntwo\n");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_read_only_ranges(vec![4..8], cx);
                state.focus(window, cx);
                state.set_selected_range(8..8, cx);
            })
        });
        // Backspace at the start of "two" would delete the separator's newline.
        cx.simulate_keystrokes("backspace");
        assert_eq!(editor.read_with(&cx, |state, _| state.text().to_string()), "one\n---\ntwo\n");
        // Typing at its edge is fine, and the range moves with the text.
        cx.simulate_keystrokes("z");
        cx.update(|window, cx| editor.update(cx, |state, cx| {
            state.set_selected_range(0..0, cx);
            state.replace("> ", window, cx);
        }));
        editor.read_with(&cx, |state, _| {
            assert_eq!(state.text().to_string(), "> one\n---\nztwo\n");
            assert_eq!(state.read_only_ranges(), [6..10]);
        });
    }
}

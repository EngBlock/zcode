//! Each change to the text, as an event, and ranges the user can't edit.
//!
//! [`BufferEdit`] reports every replacement the engine applies, typing,
//! commands, undo and redo, and `set_value` (as one whole-text edit), in
//! order and in the coordinates of the text just before it. A multibuffer
//! forwards them to the files its excerpts come from; a language client can
//! send them as incremental changes.
//!
//! An [`EditJournal`] records the same edits as they're applied, for a
//! client that has to keep in step with the text from inside an update.
//!
//! Read-only ranges refuse the user's edits that touch them (a multibuffer's
//! excerpt separators, generated code), while text can still be inserted at
//! their edges. Like read-only mode, they don't limit the programmatic APIs
//! or undo. They follow edits.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ops::Range;
use std::rc::Rc;

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

/// Every edit applied to an editor's text, numbered and recorded as it's
/// applied ([`EditorState::edit_journal`]).
///
/// A [`BufferEdit`] event arrives after the update that made the edit, but a
/// language client syncs the document from inside the providers the editor
/// calls mid-update (completion as a key is typed), so it reads the edits from
/// here: those between the revision it last sent and the current one. The
/// journal keeps them until the reader forgets them, up to a limit past which
/// the oldest are dropped and [`Self::edits_between`] has nothing for them.
#[derive(Clone, Default)]
pub struct EditJournal(Rc<RefCell<JournalEntries>>);

#[derive(Default)]
struct JournalEntries {
    /// The revision before the first kept edit.
    first: u64,
    edits: VecDeque<BufferEdit>,
    /// The kept edits' inserted text, in bytes.
    bytes: usize,
}

/// What a journal keeps at most when nobody reads it (its language server
/// never starts, say): a reader falls back on comparing texts.
const JOURNAL_MAX_EDITS: usize = 4096;
const JOURNAL_MAX_BYTES: usize = 4 << 20;

impl EditJournal {
    /// The text's revision: how many edits it has had since the journal began.
    pub fn revision(&self) -> u64 {
        let entries = self.0.borrow();
        entries.first + entries.edits.len() as u64
    }

    /// The edits that took the text from revision `from` to `to`, in order,
    /// or `None` when some of them are no longer kept.
    pub fn edits_between(&self, from: u64, to: u64) -> Option<Vec<BufferEdit>> {
        let entries = self.0.borrow();
        let end = entries.first + entries.edits.len() as u64;
        if from < entries.first || to < from || to > end {
            return None;
        }
        let range = (from - entries.first) as usize..(to - entries.first) as usize;
        Some(entries.edits.range(range).cloned().collect())
    }

    /// Drops the edits up to revision `revision`: the reader has them.
    pub fn forget_through(&self, revision: u64) {
        let mut entries = self.0.borrow_mut();
        while entries.first < revision {
            let Some(edit) = entries.edits.pop_front() else { break };
            entries.first += 1;
            entries.bytes -= edit.new_text.len();
        }
    }

    pub(crate) fn record(&self, range: &Range<usize>, new_text: &str) {
        let mut entries = self.0.borrow_mut();
        if new_text.len() > JOURNAL_MAX_BYTES {
            // Too big to keep: the revision moves on with no edits for it.
            entries.first += entries.edits.len() as u64 + 1;
            entries.edits.clear();
            entries.bytes = 0;
            return;
        }
        entries.edits.push_back(BufferEdit { range: range.clone(), new_text: new_text.to_string() });
        entries.bytes += new_text.len();
        while entries.edits.len() > JOURNAL_MAX_EDITS || entries.bytes > JOURNAL_MAX_BYTES {
            let Some(edit) = entries.edits.pop_front() else { break };
            entries.first += 1;
            entries.bytes -= edit.new_text.len();
        }
    }
}

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
    /// The journal of this editor's edits, which starts recording when first
    /// asked for. A view's journal has every edit applied to its text,
    /// whichever linked view made it.
    pub fn edit_journal(&mut self) -> EditJournal {
        self.edit_journal.get_or_insert_with(EditJournal::default).clone()
    }

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
    fn the_journal_has_each_edit_as_it_is_applied(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "abc\n");
        let journal = editor.update(&mut cx, |state, _| state.edit_journal());
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.edit([(3..3, "!"), (0..0, ">")], window, cx);
                // Recorded inside the update, before any event is delivered.
                assert_eq!(journal.revision(), 2);
                state.undo(window, cx);
                state.set_value("one\ntwo", window, cx);
                state.focus(window, cx);
                state.set_selected_range(3..3, cx);
            })
        });
        cx.simulate_keystrokes("x backspace backspace");
        let replay = |from: u64, text: &str| {
            let mut text = text.to_string();
            for edit in journal.edits_between(from, journal.revision()).unwrap() {
                text.replace_range(edit.range, &edit.new_text);
            }
            text
        };
        assert_eq!(journal.revision(), 8);
        assert_eq!(replay(0, "abc\n"), "on\ntwo");
        assert_eq!(editor.read_with(&cx, |state, _| state.text().to_string()), "on\ntwo");

        // Forgotten edits are gone; the ones after them stay.
        journal.forget_through(5);
        assert_eq!(journal.edits_between(4, 8), None);
        assert_eq!(replay(5, "one\ntwo"), "on\ntwo");
        assert_eq!(journal.edits_between(8, 7), None);
    }

    #[gpui::test]
    fn the_journal_keeps_a_bounded_amount(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "");
        let journal = editor.update(&mut cx, |state, _| state.edit_journal());
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                for _ in 0..JOURNAL_MAX_EDITS + 10 {
                    state.edit([(0..0, "a")], window, cx);
                }
            })
        });
        let revision = journal.revision();
        assert_eq!(revision, JOURNAL_MAX_EDITS as u64 + 10);
        assert_eq!(journal.edits_between(0, revision), None);
        assert_eq!(journal.edits_between(10, revision).map(|edits| edits.len()), Some(JOURNAL_MAX_EDITS));
        // An edit too big to keep moves the revision on without one.
        let big = "b".repeat(JOURNAL_MAX_BYTES + 1);
        cx.update(|window, cx| editor.update(cx, |state, cx| state.set_value(big, window, cx)));
        assert_eq!(journal.revision(), revision + 1);
        assert_eq!(journal.edits_between(revision, revision + 1), None);
        assert_eq!(journal.edits_between(revision + 1, revision + 1), Some(vec![]));
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

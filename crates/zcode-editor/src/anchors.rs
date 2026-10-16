//! Anchors: positions in the buffer that follow its edits.
//!
//! A point anchor is a byte offset with a bias, deciding which side of text
//! inserted exactly at it the anchor ends up on. A line anchor follows a
//! whole line (breakpoints, bookmarks, gutter marks): it moves when lines are
//! inserted or removed above it, and reports its line as deleted once an
//! edit removes all of that line, newline included.
//!
//! Anchors survive undo and redo. Shifting an offset past an edit is
//! reversible, but an anchor inside replaced text collapses to its edge, and
//! a deleted line is gone. So each edit records the anchors it moved that
//! way in its undo history entry, and undoing the edit puts them back where
//! they were, deleted lines included; redo replays the edit, which moves
//! them again.
//!
//! Offsets are UTF-8 bytes. Anchors live in the editor state, so they are
//! dropped with it; [`EditorState::remove_anchor`] releases one earlier.

use std::collections::BTreeMap;
use std::ops::Range;

use ropey::Rope;

use crate::{EditorState, RopeExt as _};

/// A handle to an anchor in one editor. Copy it freely; it stays valid until
/// [`EditorState::remove_anchor`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Anchor(u64);

/// Which side of text inserted exactly at a point anchor the anchor ends up
/// on: `Left` stays before it, `Right` moves past it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnchorBias {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Point(AnchorBias),
    /// `start..end` is the line with its newline (or up to the end of the
    /// text, on the last line).
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    kind: Kind,
    start: usize,
    end: usize,
    deleted: bool,
}

/// An anchor as it stood before an edit moved it irreversibly; kept in the
/// edit's undo history entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AnchorSnapshot {
    id: u64,
    start: usize,
    end: usize,
    deleted: bool,
}

/// An anchor's row at a moment, as [`EditorState::anchor_rows`] reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchorRow {
    pub anchor: Anchor,
    /// The 0-based row, or `None` while its line is deleted.
    pub row: Option<u32>,
}

#[derive(Default)]
pub(crate) struct Anchors {
    entries: BTreeMap<u64, Entry>,
    next: u64,
    /// What the last edit moved irreversibly, for its history entry.
    moved: Vec<AnchorSnapshot>,
}

/// Maps `offset` through an edit that replaced `range` with `new_len` bytes.
/// An offset inside the range collapses to its start, or with `after` to the
/// end of the new text; `after` also moves an offset at an insertion past it.
fn map_offset(offset: usize, range: &Range<usize>, new_len: usize, after: bool) -> usize {
    if offset < range.start || (offset == range.start && !after) {
        offset
    } else if offset > range.end || (offset == range.end && !after) || (offset == range.end && range.is_empty()) {
        offset - range.len() + new_len
    } else if after {
        range.start + new_len
    } else {
        range.start
    }
}

impl Anchors {
    fn insert(&mut self, kind: Kind, start: usize, end: usize) -> Anchor {
        let id = self.next;
        self.next += 1;
        self.entries.insert(id, Entry { kind, start, end, deleted: false });
        Anchor(id)
    }

    /// Moves every anchor through an edit that replaced `range` with
    /// `new_len` bytes, noting the ones it moved irreversibly.
    pub(crate) fn adjust_for_edit(&mut self, range: &Range<usize>, new_len: usize) {
        self.moved.clear();
        for (&id, entry) in self.entries.iter_mut() {
            let (start, end) = (entry.start, entry.end);
            // Shifting past an edit and moving with an insertion can be
            // undone by arithmetic; landing inside replaced text can't.
            let touched = !range.is_empty()
                && ((range.start..=range.end).contains(&start) || (range.start..=range.end).contains(&end));
            if touched {
                self.moved.push(AnchorSnapshot { id, start, end, deleted: entry.deleted });
            }
            match entry.kind {
                Kind::Point(bias) => {
                    entry.start = map_offset(start, range, new_len, bias == AnchorBias::Right);
                    entry.end = entry.start;
                }
                // Text inserted at a line's start belongs to it (a newline
                // typed there pushes the line down); text at its end belongs
                // to the next line.
                Kind::Line => {
                    entry.start = map_offset(start, range, new_len, true);
                    entry.end = map_offset(end, range, new_len, false).max(entry.start);
                    // The whole line went, newline included (the last line:
                    // with the newline before it).
                    if !range.is_empty() && range.start <= start && end <= range.end && (start < range.end || range.start < start) {
                        entry.deleted = true;
                    }
                }
            }
        }
    }

    /// The anchors the last edit moved irreversibly.
    pub(crate) fn take_moved(&mut self) -> Vec<AnchorSnapshot> {
        std::mem::take(&mut self.moved)
    }

    /// Puts anchors back as an edit's history entry recorded them, after
    /// undo has reverted that edit.
    pub(crate) fn restore(&mut self, snapshots: &[AnchorSnapshot]) {
        for snapshot in snapshots {
            if let Some(entry) = self.entries.get_mut(&snapshot.id) {
                entry.start = snapshot.start;
                entry.end = snapshot.end;
                entry.deleted = snapshot.deleted;
            }
        }
    }

    /// Each anchor's row (lines) or offset (points), before the whole text
    /// is replaced without history.
    pub(crate) fn positions(&self, text: &Rope) -> Vec<(u64, usize)> {
        self.entries
            .iter()
            .map(|(&id, entry)| match entry.kind {
                Kind::Point(_) => (id, entry.start),
                Kind::Line => (id, text.offset_to_point(entry.start.min(text.len())).row),
            })
            .collect()
    }

    /// After a reload: line anchors keep their row and points their offset,
    /// both clamped to the new text.
    pub(crate) fn restore_positions(&mut self, positions: Vec<(u64, usize)>, text: &Rope) {
        self.moved.clear();
        for (id, position) in positions {
            let Some(entry) = self.entries.get_mut(&id) else { continue };
            entry.deleted = false;
            match entry.kind {
                Kind::Point(_) => {
                    entry.start = text.clip_offset(position.min(text.len()), sum_tree::Bias::Left);
                    entry.end = entry.start;
                }
                Kind::Line => {
                    (entry.start, entry.end) = line_range(text, position.min(text.lines_len().saturating_sub(1)));
                }
            }
        }
    }
}

/// The byte range of 0-based `row` with its newline.
fn line_range(text: &Rope, row: usize) -> (usize, usize) {
    let start = text.line_start_offset(row);
    let end = if row + 1 < text.lines_len() {
        text.line_start_offset(row + 1)
    } else {
        text.len()
    };
    (start, end)
}

impl EditorState {
    /// Anchors a position. `bias` decides which side of text inserted
    /// exactly there the anchor ends up on.
    pub fn anchor(&mut self, offset: usize, bias: AnchorBias) -> Anchor {
        let offset = self.text.clip_offset(offset.min(self.text.len()), sum_tree::Bias::Left);
        self.anchors.insert(Kind::Point(bias), offset, offset)
    }

    /// Anchors 0-based line `row` (clamped to the last line). The anchor's
    /// row follows lines inserted and removed above it, and is `None` while
    /// an edit has deleted the whole line; undo brings it back.
    pub fn line_anchor(&mut self, row: u32) -> Anchor {
        let row = (row as usize).min(self.text.lines_len().saturating_sub(1));
        let (start, end) = line_range(&self.text, row);
        self.anchors.insert(Kind::Line, start, end)
    }

    pub fn remove_anchor(&mut self, anchor: Anchor) {
        self.anchors.entries.remove(&anchor.0);
    }

    /// The anchor's offset (a line anchor's line start), or `None` while its
    /// line is deleted or after it was removed.
    pub fn anchor_offset(&self, anchor: Anchor) -> Option<usize> {
        let entry = self.anchors.entries.get(&anchor.0)?;
        (!entry.deleted).then_some(entry.start.min(self.text.len()))
    }

    /// The anchor's 0-based row, or `None` while its line is deleted.
    pub fn anchor_row(&self, anchor: Anchor) -> Option<u32> {
        let offset = self.anchor_offset(anchor)?;
        Some(self.text.offset_to_point(offset).row as u32)
    }

    /// Every anchor's row, in the order they were created.
    pub fn anchor_rows(&self) -> Vec<AnchorRow> {
        self.anchors
            .entries
            .keys()
            .map(|&id| AnchorRow {
                anchor: Anchor(id),
                row: self.anchor_row(Anchor(id)),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::open_editor;
    use gpui::TestAppContext;

    fn anchors_text() -> &'static str {
        "zero\none\ntwo\nthree\nfour\n"
    }

    #[test]
    fn offsets_map_through_edits_with_their_bias() {
        assert_eq!(map_offset(3, &(5..7), 4, false), 3);
        assert_eq!(map_offset(9, &(5..7), 4, false), 11);
        assert_eq!(map_offset(5, &(5..5), 2, false), 5);
        assert_eq!(map_offset(5, &(5..5), 2, true), 7);
        assert_eq!(map_offset(6, &(5..8), 1, false), 5);
        assert_eq!(map_offset(6, &(5..8), 1, true), 6);
        assert_eq!(map_offset(8, &(5..8), 1, false), 6);
    }

    #[gpui::test]
    fn line_anchors_follow_inserted_and_removed_lines(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, anchors_text());
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                let two = state.line_anchor(2);
                let point = state.anchor(state.text().line_start_offset(3) + 2, AnchorBias::Left);
                // A line inserted above moves both down.
                state.edit([(0..0, "new\n")], window, cx);
                assert_eq!(state.anchor_row(two), Some(3));
                assert_eq!(state.anchor_row(point), Some(4));
                // A newline typed at the line's start pushes it down too.
                let start = state.text().line_start_offset(3);
                state.edit([(start..start, "\n")], window, cx);
                assert_eq!(state.anchor_row(two), Some(4));
                // Typing on the line keeps it.
                let start = state.text().line_start_offset(4);
                state.edit([(start + 1..start + 1, "xx")], window, cx);
                assert_eq!(state.anchor_row(two), Some(4));
                // Removing lines above moves it up.
                state.edit([(0..4, "")], window, cx);
                assert_eq!(state.anchor_row(two), Some(3));
                // Emptying the line keeps it; deleting it with its newline doesn't.
                let (start, end) = line_range(state.text(), 3);
                state.edit([(start..end - 1, "")], window, cx);
                assert_eq!(state.anchor_row(two), Some(3));
                state.edit([(start..start + 1, "")], window, cx);
                assert_eq!(state.anchor_row(two), None);
                assert_eq!(state.anchor_rows().len(), 2);
                state.remove_anchor(two);
                assert_eq!(state.anchor_rows().len(), 1);
            });
        });
    }

    #[gpui::test]
    fn deleted_lines_come_back_with_undo_and_go_again_with_redo(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, anchors_text());
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                let one = state.line_anchor(1);
                let three = state.line_anchor(3);
                // Delete lines 1–3 as one edit, then type somewhere else.
                let (start, _) = line_range(state.text(), 1);
                let (_, end) = line_range(state.text(), 3);
                state.edit([(start..end, "")], window, cx);
                assert_eq!((state.anchor_row(one), state.anchor_row(three)), (None, None));
                state.edit([(0..0, "// header\n")], window, cx);
                state.undo(window, cx);
                state.undo(window, cx);
                assert_eq!(state.text().to_string(), anchors_text());
                assert_eq!((state.anchor_row(one), state.anchor_row(three)), (Some(1), Some(3)));
                state.redo(window, cx);
                assert_eq!((state.anchor_row(one), state.anchor_row(three)), (None, None));
                state.undo(window, cx);
                assert_eq!((state.anchor_row(one), state.anchor_row(three)), (Some(1), Some(3)));
            });
        });
    }

    #[gpui::test]
    fn a_replaced_block_puts_anchors_back_on_undo(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, anchors_text());
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                let two = state.line_anchor(2);
                let point = state.anchor(state.text().line_start_offset(2) + 1, AnchorBias::Right);
                let before = state.anchor_offset(point);
                // Rewrite lines 1–3 in one edit (a format, a paste over them).
                let (start, _) = line_range(state.text(), 1);
                let (_, end) = line_range(state.text(), 3);
                state.edit([(start..end, "rewritten\n")], window, cx);
                assert_eq!(state.anchor_row(two), None);
                // A right-biased point collapses to the end of the new text.
                assert_eq!(state.anchor_offset(point), Some(start + "rewritten\n".len()));
                state.undo(window, cx);
                assert_eq!(state.anchor_row(two), Some(2));
                assert_eq!(state.anchor_offset(point), before);
            });
        });
    }

    #[gpui::test]
    fn reloading_the_text_keeps_rows(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, anchors_text());
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                let three = state.line_anchor(3);
                let last = state.line_anchor(4);
                state.set_value("a\nb\nc\nd\n", window, cx);
                assert_eq!(state.anchor_row(three), Some(3));
                state.set_value("only\n", window, cx);
                assert_eq!(state.anchor_row(three), Some(1));
                assert_eq!(state.anchor_row(last), Some(1));
            });
        });
    }
}

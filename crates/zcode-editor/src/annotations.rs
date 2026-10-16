//! Inline annotations: inlays (text shown between characters, such as
//! inlay hints and inline blame) and blocks (whole lines of text shown below
//! a row, such as the rest of a multi-line edit prediction).
//!
//! Neither is part of the buffer: offsets, selections, copy and the language
//! server never see them, and the caret steps over an inlay in one move.
//! Both sit on anchors (`anchors.rs`), so they follow edits, undo and redo,
//! and come in keyed groups their owner replaces wholesale (hints after each
//! response, blame when the cursor moves).
//!
//! An inlay at an offset is laid out as a zero-length fragment of its row,
//! with its own width, which the soft wrapper counts. Which side of it the
//! caret is drawn on at that offset is the inlay's choice: a type hint
//! (`x: Int`) belongs to the text before it, a parameter name (`name: x`)
//! to the text after. A block adds rows below its line; the rows after it
//! move down, and so does everything measured from them.

use std::collections::BTreeMap;

use gpui::{Context, Hsla, SharedString};

use crate::anchors::{Anchor, AnchorBias, Anchors};
use crate::{EditorState, InputBaseState, InputModeKind, RopeExt as _};

/// Text shown at an offset without being part of the buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct Inlay {
    pub offset: usize,
    pub text: SharedString,
    /// The text color; the editor's muted color when `None`.
    pub color: Option<Hsla>,
    pub background: Option<Hsla>,
    /// Belongs to the text after it: the caret at `offset` is drawn after
    /// the inlay, and text typed there goes after it too.
    pub before_text: bool,
}

impl Inlay {
    /// An inlay that belongs to the text before `offset`, like a type hint.
    pub fn new(offset: usize, text: impl Into<SharedString>) -> Self {
        Self { offset, text: text.into(), color: None, background: None, before_text: false }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn background(mut self, background: Hsla) -> Self {
        self.background = Some(background);
        self
    }

    /// Belongs to the text after it, like a parameter name.
    pub fn before_text(mut self) -> Self {
        self.before_text = true;
        self
    }
}

/// Lines of text shown below a row without being part of the buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// The 0-based row the lines go below.
    pub row: u32,
    pub lines: Vec<SharedString>,
    /// The text color; the editor's muted color when `None`.
    pub color: Option<Hsla>,
    pub background: Option<Hsla>,
}

impl Block {
    pub fn new(row: u32, lines: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        Self { row, lines: lines.into_iter().map(Into::into).collect(), color: None, background: None }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn background(mut self, background: Hsla) -> Self {
        self.background = Some(background);
        self
    }
}

pub(crate) struct PlacedInlay {
    pub(crate) anchor: Anchor,
    pub(crate) inlay: Inlay,
}

pub(crate) struct PlacedBlock {
    pub(crate) anchor: Anchor,
    pub(crate) block: Block,
}

#[derive(Default)]
pub(crate) struct Annotations {
    inlays: BTreeMap<SharedString, Vec<PlacedInlay>>,
    blocks: BTreeMap<SharedString, Vec<PlacedBlock>>,
    /// Bumped whenever a group changes, so layout caches can tell.
    pub(crate) revision: u64,
    /// Whether the wrapper was last given inlay widths, so they are taken
    /// back once the last inlay goes.
    pub(crate) had_inlay_metrics: bool,
}

impl Annotations {
    pub(crate) fn has_inlays(&self) -> bool {
        self.inlays.values().any(|group| !group.is_empty())
    }

    pub(crate) fn has_blocks(&self) -> bool {
        self.blocks.values().any(|group| !group.is_empty())
    }

    /// Every inlay with its current offset, sorted by offset (the caret-after
    /// ones last at an offset, so they sit nearest the text after them).
    pub(crate) fn inlays(&self, anchors: &Anchors) -> Vec<(usize, &Inlay)> {
        let mut inlays: Vec<(usize, &Inlay)> = self
            .inlays
            .values()
            .flatten()
            .filter_map(|placed| Some((anchors.offset(placed.anchor)?, &placed.inlay)))
            .collect();
        inlays.sort_by_key(|(offset, inlay)| (*offset, inlay.before_text));
        inlays
    }

    /// The blocks below each row, by 0-based row; a block whose row is
    /// deleted is hidden until undo brings the row back.
    pub(crate) fn blocks_by_row(&self, anchors: &Anchors, text: &ropey::Rope) -> BTreeMap<usize, Vec<&Block>> {
        let mut rows: BTreeMap<usize, Vec<&Block>> = BTreeMap::new();
        for placed in self.blocks.values().flatten() {
            if let Some(row) = anchors.row(placed.anchor, text) {
                rows.entry(row as usize).or_default().push(&placed.block);
            }
        }
        rows
    }
}

impl EditorState {
    /// Sets the inlay group `key`, replacing what it had.
    pub fn set_inlays(&mut self, key: impl Into<SharedString>, inlays: Vec<Inlay>, cx: &mut Context<Self>) {
        let key = key.into();
        self.clear_inlay_group(&key);
        let placed = inlays
            .into_iter()
            .map(|inlay| {
                // Text typed at the inlay goes on the side it belongs to.
                let bias = if inlay.before_text { AnchorBias::Left } else { AnchorBias::Right };
                PlacedInlay { anchor: self.anchor(inlay.offset, bias), inlay }
            })
            .collect();
        self.annotations.inlays.insert(key, placed);
        self.annotations.revision += 1;
        cx.notify();
    }

    pub fn clear_inlays(&mut self, key: &str, cx: &mut Context<Self>) {
        self.clear_inlay_group(key);
        self.annotations.revision += 1;
        cx.notify();
    }

    fn clear_inlay_group(&mut self, key: &str) {
        if let Some(group) = self.annotations.inlays.remove(key) {
            for placed in group {
                self.remove_anchor(placed.anchor);
            }
        }
    }

    /// Group `key`'s inlays where they are now.
    pub fn inlays(&self, key: &str) -> Vec<Inlay> {
        self.annotations
            .inlays
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(|placed| {
                let offset = self.anchor_offset(placed.anchor)?;
                Some(Inlay { offset, ..placed.inlay.clone() })
            })
            .collect()
    }

    /// Sets the block group `key`, replacing what it had.
    pub fn set_blocks(&mut self, key: impl Into<SharedString>, blocks: Vec<Block>, cx: &mut Context<Self>) {
        let key = key.into();
        self.clear_block_group(&key);
        let placed = blocks
            .into_iter()
            .filter(|block| !block.lines.is_empty())
            .map(|block| PlacedBlock { anchor: self.line_anchor(block.row), block })
            .collect();
        self.annotations.blocks.insert(key, placed);
        self.annotations.revision += 1;
        cx.notify();
    }

    pub fn clear_blocks(&mut self, key: &str, cx: &mut Context<Self>) {
        self.clear_block_group(key);
        self.annotations.revision += 1;
        cx.notify();
    }

    fn clear_block_group(&mut self, key: &str) {
        if let Some(group) = self.annotations.blocks.remove(key) {
            for placed in group {
                self.remove_anchor(placed.anchor);
            }
        }
    }

    /// Group `key`'s blocks on their rows now (hidden ones left out).
    pub fn blocks(&self, key: &str) -> Vec<Block> {
        self.annotations
            .blocks
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(|placed| Some(Block { row: self.anchor_row(placed.anchor)?, ..placed.block.clone() }))
            .collect()
    }

}

impl<M: InputModeKind> InputBaseState<M> {
    /// How many block rows sit above 0-based `row` (below the rows before
    /// it), skipping folded rows.
    pub(crate) fn block_rows_before(&self, row: usize) -> usize {
        if !self.annotations.has_blocks() {
            return 0;
        }
        self.annotations
            .blocks_by_row(&self.anchors, &self.text)
            .range(..row)
            .filter(|(line, _)| !self.display_map.is_buffer_line_hidden(**line))
            .map(|(_, blocks)| blocks.iter().map(|b| b.lines.len()).sum::<usize>())
            .sum()
    }

    /// Every block row below a visible line.
    pub(crate) fn block_rows_total(&self) -> usize {
        self.block_rows_before(self.text.lines_len())
    }

    /// The display row (folds and wraps, without blocks) at visual row
    /// `visual` (blocks counted), and whether that visual row is in a block.
    pub(crate) fn visual_row_to_display_row(&self, visual: usize) -> usize {
        if !self.annotations.has_blocks() {
            return visual;
        }
        let mut before = 0;
        for (line, blocks) in self.annotations.blocks_by_row(&self.anchors, &self.text) {
            let Some(rows) = self.display_map.buffer_line_to_display_row_range(line) else { continue };
            let height: usize = blocks.iter().map(|b| b.lines.len()).sum();
            // The line's last display row, then its block rows.
            if visual < rows.end + before {
                break;
            }
            if visual < rows.end + before + height {
                return rows.end - 1;
            }
            before += height;
        }
        visual - before
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext, px};

    use super::*;
    use crate::test_support::open_editor;

    fn x_of(editor: &gpui::Entity<EditorState>, cx: &mut VisualTestContext, offset: usize) -> gpui::Pixels {
        editor.read_with(cx, |state, _| state.range_to_bounds(&(offset..offset)).unwrap().origin.x)
    }

    fn y_of(editor: &gpui::Entity<EditorState>, cx: &mut VisualTestContext, offset: usize) -> gpui::Pixels {
        editor.read_with(cx, |state, _| state.range_to_bounds(&(offset..offset)).unwrap().origin.y)
    }

    #[gpui::test]
    fn inlays_take_room_but_no_offsets(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "let x = f(y)\n");
        let (before_5, before_6, before_10, before_11) =
            (x_of(&editor, &mut cx, 5), x_of(&editor, &mut cx, 6), x_of(&editor, &mut cx, 10), x_of(&editor, &mut cx, 11));
        cx.update(|_, cx| {
            editor.update(cx, |state, cx| {
                state.set_inlays(
                    "hints",
                    vec![Inlay::new(5, ": Int"), Inlay::new(10, "value: ").before_text()],
                    cx,
                );
            })
        });
        cx.run_until_parked();
        assert_eq!(editor.read_with(&cx, |state, _| state.text().to_string()), "let x = f(y)\n");
        // A type hint: the caret at its offset stays before it, the text after moves.
        assert_eq!(x_of(&editor, &mut cx, 5), before_5);
        let hint = x_of(&editor, &mut cx, 6) - before_6;
        assert!(hint > px(20.), "{hint:?}");
        // A parameter name: the caret at its offset is drawn after it.
        assert!(x_of(&editor, &mut cx, 10) - before_10 > hint + px(20.));
        assert_eq!(x_of(&editor, &mut cx, 11) - before_11, x_of(&editor, &mut cx, 10) - before_10);

        // The caret steps over an inlay in one move.
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_selected_range(5..5, cx);
                state.focus(window, cx);
            })
        });
        cx.simulate_keystrokes("right");
        assert_eq!(editor.read_with(&cx, |state, _| state.selected_range()), 6..6);
    }

    #[gpui::test]
    fn inlays_follow_edits_on_the_side_they_belong_to(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "let x = f(y)\n");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_inlays("hints", vec![Inlay::new(5, ": Int"), Inlay::new(10, "value: ").before_text()], cx);
                // Typing at the end of `x` keeps the type hint after it; typing
                // at the start of `y` keeps the parameter name before it.
                state.edit([(10..10, "z")], window, cx);
                state.edit([(5..5, "x")], window, cx);
                let offsets: Vec<usize> = state.inlays("hints").iter().map(|i| i.offset).collect();
                assert_eq!(offsets, [6, 11]);
                state.undo(window, cx);
                state.undo(window, cx);
                let offsets: Vec<usize> = state.inlays("hints").iter().map(|i| i.offset).collect();
                assert_eq!(offsets, [5, 10]);
                state.clear_inlays("hints", cx);
                assert!(state.inlays("hints").is_empty() && state.anchor_rows().is_empty());
            })
        });
    }

    #[gpui::test]
    fn blocks_push_the_rows_below_them_down(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "zero\none\ntwo\n");
        let line_height = editor.read_with(&cx, |state, _| state.line_height().unwrap());
        let one = editor.read_with(&cx, |state, _| state.text().line_start_offset(1));
        let two = editor.read_with(&cx, |state, _| state.text().line_start_offset(2));
        let (y1, y2) = (y_of(&editor, &mut cx, one), y_of(&editor, &mut cx, two));
        cx.update(|_, cx| {
            editor.update(cx, |state, cx| {
                state.set_blocks("prediction", vec![Block::new(1, ["first", "second"])], cx);
            })
        });
        cx.run_until_parked();
        assert_eq!(y_of(&editor, &mut cx, one), y1);
        assert_eq!(y_of(&editor, &mut cx, two), y2 + line_height * 2.);
        let gutter_two = editor.read_with(&cx, |state, _| state.gutter_geometry().rows[2].1);
        assert_eq!(gutter_two, y_of(&editor, &mut cx, two));
        // A line inserted above moves the block with its row.
        cx.update(|window, cx| editor.update(cx, |state, cx| state.edit([(0..0, "new\n")], window, cx)));
        cx.run_until_parked();
        assert_eq!(editor.read_with(&cx, |state, _| state.blocks("prediction")[0].row), 2);
        cx.update(|_, cx| editor.update(cx, |state, cx| state.clear_blocks("prediction", cx)));
        cx.run_until_parked();
        let three = editor.read_with(&cx, |state, _| state.text().line_start_offset(3));
        assert_eq!(y_of(&editor, &mut cx, three), y2 + line_height);
    }

    #[gpui::test]
    fn soft_wrap_counts_inlay_widths(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "short line\n");
        cx.update(|window, cx| editor.update(cx, |state, cx| state.set_soft_wrap(true, window, cx)));
        cx.run_until_parked();
        let rows = |cx: &mut VisualTestContext| editor.read_with(cx, |state, _| state.display_map.wrap_row_count());
        let before = rows(&mut cx);
        cx.update(|_, cx| {
            editor.update(cx, |state, cx| state.set_inlays("long", vec![Inlay::new(5, "x".repeat(400))], cx))
        });
        cx.run_until_parked();
        assert!(rows(&mut cx) > before, "the inlay wraps its row");
    }
}

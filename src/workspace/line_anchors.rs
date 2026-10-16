//! Lines anchored in an open buffer, for breakpoints and bookmarks: each
//! line gets an invisible editor decoration, which the editor moves through
//! edits and undo. After an edit the owner reads where its anchors went and
//! moves its lines there; an anchor whose line was deleted reports `None`.

use zcode_editor::{EditorState, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle, RopeExt as _};
use gpui_kit::*;

use super::Workspace;

struct Anchor {
    id: u64,
    /// The 1-based line when anchored.
    line: u32,
    mark: Option<RangeDecorationCollection>,
}

#[derive(Default)]
pub(super) struct LineAnchors(Vec<Anchor>);

impl LineAnchors {
    /// Anchors `wanted` (id, 1-based line) in `editor`. Skipped when the
    /// anchors already match, unless `force` (the text was replaced).
    pub fn set(&mut self, editor: &Entity<EditorState>, wanted: Vec<(u64, u32)>, force: bool, cx: &mut App) {
        let current: Vec<(u64, u32)> = self.0.iter().map(|a| (a.id, a.line)).collect();
        if current == wanted && !force {
            return;
        }
        for anchor in self.0.drain(..) {
            if let Some(mark) = anchor.mark {
                mark.dispose(cx);
            }
        }
        let text = editor.read(cx).text().clone();
        self.0 = wanted
            .into_iter()
            .map(|(id, line)| {
                let mark = Workspace::line_range(&text, line).map(|range| {
                    // Invisible: a gutter overlay shows it.
                    let decoration = RangeDecoration::new(range)
                        .with_style(RangeDecorationStyle::Fill)
                        .with_color(gpui_kit::transparent_black());
                    editor.update(cx, |state, cx| state.create_range_decorations_collection(vec![decoration], cx))
                });
                Anchor { id, line, mark }
            })
            .collect();
    }

    /// The anchors whose line changed since they were set: the new 1-based
    /// line, or `None` when the line was deleted.
    pub fn moves(&self, editor: &Entity<EditorState>, cx: &App) -> Vec<(u64, Option<u32>)> {
        if self.0.is_empty() {
            return vec![];
        }
        let text = editor.read(cx).text().clone();
        let mut moves = vec![];
        for anchor in &self.0 {
            let Some(mark) = &anchor.mark else { continue };
            match mark.get_ranges(cx).first() {
                None => moves.push((anchor.id, None)),
                Some(range) => {
                    let line = text.offset_to_point(range.start).row as u32 + 1;
                    if line != anchor.line {
                        moves.push((anchor.id, Some(line)));
                    }
                }
            }
        }
        moves
    }
}

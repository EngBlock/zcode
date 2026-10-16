//! Highlight layers: styled ranges that override syntax highlighting, by
//! priority.
//!
//! Each layer has a key, so its owner replaces it wholesale (semantic tokens
//! after each response, the references to the symbol under the cursor), and
//! a priority. Over the tree-sitter styles, layers apply from the lowest priority to the highest: a field a higher
//! layer sets (color, weight, background, underline) wins, a field it leaves
//! unset shows through. Text decorations and diagnostics stay above every
//! layer. Ranges are UTF-8 bytes and follow edits; a range an edit empties
//! is dropped.

use std::collections::BTreeSet;
use std::ops::Range;

use gpui::{Context, HighlightStyle, SharedString};

use crate::EditorState;

/// Priorities of the layers zcode knows about; any `i32` works.
pub struct HighlightPriority;

impl HighlightPriority {
    /// Semantic tokens from the language server, over tree-sitter.
    pub const SEMANTIC_TOKENS: i32 = 100;
    /// Occurrences of the symbol under the cursor.
    pub const REFERENCES: i32 = 200;
    /// Find results and other transient marks.
    pub const SEARCH: i32 = 300;
}

struct Layer {
    key: SharedString,
    priority: i32,
    /// Sorted by start.
    ranges: Vec<(Range<usize>, HighlightStyle)>,
    /// The largest end among `ranges[..=i]`, to find the visible ones by
    /// binary search even when ranges overlap.
    max_ends: Vec<usize>,
}

impl Layer {
    fn reindex(&mut self) {
        self.ranges.sort_by_key(|(range, _)| range.start);
        self.max_ends.clear();
        self.reindex_ends(0);
    }

    /// Recomputes `max_ends` from `from` on.
    fn reindex_ends(&mut self, from: usize) {
        self.max_ends.truncate(from);
        let mut max = self.max_ends.last().copied().unwrap_or(0);
        self.max_ends.extend(self.ranges[from..].iter().map(|(range, _)| {
            max = max.max(range.end);
            max
        }));
    }

    fn visible(&self, range: &Range<usize>) -> impl Iterator<Item = &(Range<usize>, HighlightStyle)> {
        let first = self.max_ends.partition_point(|&end| end <= range.start);
        self.ranges[first..]
            .iter()
            .take_while(move |(r, _)| r.start < range.end)
            .filter(move |(r, _)| r.end > range.start)
    }
}

#[derive(Default)]
pub(crate) struct HighlightLayers {
    /// Sorted by priority, then by when they were first set.
    layers: Vec<Layer>,
}

impl HighlightLayers {
    pub(crate) fn is_empty(&self) -> bool {
        self.layers.iter().all(|layer| layer.ranges.is_empty())
    }

    /// Moves the ranges past an edit. Called on every keystroke, and a
    /// layer of semantic tokens has one range per identifier, so this
    /// leaves alone the ranges that end before the edit and doesn't sort:
    /// an edit maps offsets in order, so the ranges stay sorted by start.
    pub(crate) fn adjust_for_edit(&mut self, edited: &Range<usize>, new_len: usize) {
        for layer in &mut self.layers {
            let first = layer.max_ends.partition_point(|&end| end < edited.start);
            if first == layer.ranges.len() {
                continue;
            }
            let mut kept = first;
            for ix in first..layer.ranges.len() {
                let range = crate::decorations::adjust_range_for_edit(&layer.ranges[ix].0, edited, new_len);
                if range.start < range.end {
                    layer.ranges[ix].0 = range;
                    layer.ranges.swap(kept, ix);
                    kept += 1;
                }
            }
            layer.ranges.truncate(kept);
            layer.reindex_ends(first);
        }
    }

    /// Applies the layers to `base` (styles covering the visible text) over
    /// `visible`.
    pub(crate) fn compose(
        &self,
        base: Vec<(Range<usize>, HighlightStyle)>,
        visible: &Range<usize>,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let overrides: Vec<(Range<usize>, HighlightStyle)> = self
            .layers
            .iter()
            .flat_map(|layer| {
                layer.visible(visible).map(|(range, style)| {
                    (range.start.max(visible.start)..range.end.min(visible.end), *style)
                })
            })
            .collect();
        if overrides.is_empty() {
            return base;
        }
        compose_in_order(base.into_iter().chain(overrides))
    }
}

/// Merges styled ranges where each one refines the ones before it in the
/// input: where they overlap, a later range's set fields win. Covers the
/// union of the ranges, gaps excepted.
pub(crate) fn compose_in_order(
    styles: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let styles: Vec<(Range<usize>, HighlightStyle)> =
        styles.into_iter().filter(|(range, _)| !range.is_empty()).collect();
    let mut endpoints: Vec<(usize, bool, usize)> = Vec::with_capacity(styles.len() * 2);
    for (ix, (range, _)) in styles.iter().enumerate() {
        endpoints.push((range.start, true, ix));
        endpoints.push((range.end, false, ix));
    }
    // Ends before starts at the same offset, so touching ranges don't merge.
    endpoints.sort_unstable_by_key(|&(offset, start, ix)| (offset, start, ix));
    let mut active = BTreeSet::new();
    let mut out: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(styles.len());
    let mut at = 0;
    for (offset, start, ix) in endpoints {
        if offset > at && !active.is_empty() {
            let style = active
                .iter()
                .fold(HighlightStyle::default(), |acc, &ix: &usize| acc.highlight(styles[ix].1));
            match out.last_mut() {
                Some((range, last)) if range.end == at && *last == style => range.end = offset,
                _ => out.push((at..offset, style)),
            }
        }
        at = offset;
        if start {
            active.insert(ix);
        } else {
            active.remove(&ix);
        }
    }
    out
}

impl EditorState {
    /// Sets the highlight layer `key` to `highlights` at `priority`,
    /// replacing what it had. An empty list clears it.
    pub fn set_highlight_layer(
        &mut self,
        key: impl Into<SharedString>,
        priority: i32,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
        cx: &mut Context<Self>,
    ) {
        let key = key.into();
        let len = self.text.len();
        let ranges = highlights
            .into_iter()
            .map(|(range, style)| (range.start.min(len)..range.end.min(len), style))
            .filter(|(range, _)| !range.is_empty())
            .collect();
        let layers = &mut self.highlight_layers.layers;
        let mut layer = match layers.iter().position(|layer| layer.key == key) {
            Some(ix) => layers.remove(ix),
            None => Layer { key, priority, ranges: vec![], max_ends: vec![] },
        };
        layer.priority = priority;
        layer.ranges = ranges;
        layer.reindex();
        let ix = layers.partition_point(|other| other.priority <= priority);
        layers.insert(ix, layer);
        cx.notify();
    }

    pub fn clear_highlight_layer(&mut self, key: &str, cx: &mut Context<Self>) {
        self.highlight_layers.layers.retain(|layer| layer.key.as_ref() != key);
        cx.notify();
    }

    /// The ranges of layer `key` as they stand after edits.
    pub fn highlight_layer_ranges(&self, key: &str) -> Vec<Range<usize>> {
        self.highlight_layers
            .layers
            .iter()
            .find(|layer| layer.key.as_ref() == key)
            .map(|layer| layer.ranges.iter().map(|(range, _)| range.clone()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{FontWeight, TestAppContext, hsla};

    use super::*;
    use crate::test_support::open_editor;

    fn color(h: f32) -> HighlightStyle {
        HighlightStyle { color: Some(hsla(h, 1., 0.5, 1.)), ..Default::default() }
    }

    #[test]
    fn later_ranges_win_field_by_field_and_gaps_stay_gaps() {
        let bold = HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() };
        let composed = compose_in_order([(0..10, color(0.1)), (2..4, color(0.5)), (3..6, bold), (12..14, color(0.9))]);
        assert_eq!(
            composed,
            [
                (0..2, color(0.1)),
                (2..3, color(0.5)),
                (3..4, color(0.5).highlight(bold)),
                (4..6, color(0.1).highlight(bold)),
                (6..10, color(0.1)),
                (12..14, color(0.9)),
            ]
        );
    }

    #[test]
    fn edits_keep_layers_sorted_without_sorting_again() {
        // Overlapping and nested ranges, through edits before, inside,
        // across and after them, compared with adjusting every range and
        // sorting again.
        let ranges: Vec<Range<usize>> = (0..60).map(|i| (i * 7 % 50)..(i * 7 % 50 + 1 + i % 9)).collect();
        let mut layers = HighlightLayers::default();
        let mut layer = Layer {
            key: "tokens".into(),
            priority: 0,
            ranges: ranges.iter().map(|r| (r.clone(), color(0.1))).collect(),
            max_ends: vec![],
        };
        layer.reindex();
        layers.layers.push(layer);
        let mut expected = layers.layers[0].ranges.clone();
        for (edited, new_len) in [(10..10, 3), (20..26, 0), (0..2, 5), (55..70, 1), (30..31, 0), (5..40, 2), (200..200, 4)] {
            layers.adjust_for_edit(&edited, new_len);
            expected.retain_mut(|(range, _)| {
                *range = crate::decorations::adjust_range_for_edit(range, &edited, new_len);
                range.start < range.end
            });
            expected.sort_by_key(|(range, _)| range.start);
            let layer = &layers.layers[0];
            let starts = |list: &[(Range<usize>, HighlightStyle)]| list.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>();
            let mut got = starts(&layer.ranges);
            let mut want = starts(&expected);
            assert!(got.windows(2).all(|w| w[0].start <= w[1].start), "sorted after {edited:?}");
            got.sort_by_key(|r| (r.start, r.end));
            want.sort_by_key(|r| (r.start, r.end));
            assert_eq!(got, want, "after {edited:?}");
            let mut max = 0;
            let ends: Vec<usize> = layer.ranges.iter().map(|(r, _)| { max = max.max(r.end); max }).collect();
            assert_eq!(layer.max_ends, ends);
        }
    }

    #[gpui::test]
    fn layers_apply_by_priority_and_follow_edits(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "let value = other\n");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_highlight_layer("refs", HighlightPriority::REFERENCES, vec![(4..9, color(0.5))], cx);
                state.set_highlight_layer("tokens", HighlightPriority::SEMANTIC_TOKENS, vec![(4..9, color(0.2)), (12..17, color(0.3))], cx);
                let base = vec![(0..18, HighlightStyle::default())];
                let composed = state.highlight_layers.compose(base.clone(), &(0..18));
                assert_eq!(composed[1], (4..9, color(0.5)), "references over semantic tokens");
                assert_eq!(composed[3], (12..17, color(0.3)));
                // Only what's visible.
                assert_eq!(state.highlight_layers.compose(base, &(0..3)).len(), 1);

                state.edit([(0..0, "  ")], window, cx);
                assert_eq!(state.highlight_layer_ranges("tokens"), [6..11, 14..19]);
                state.edit([(14..19, "")], window, cx);
                assert_eq!(state.highlight_layer_ranges("tokens"), [6..11]);
                state.set_highlight_layer("refs", HighlightPriority::REFERENCES, vec![], cx);
                state.clear_highlight_layer("tokens", cx);
                assert!(state.highlight_layers.is_empty());
            });
        });
    }
}

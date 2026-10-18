//! Laying out long lines in pieces; `display_map/long_line.rs` measures them.
//!
//! Each frame a visible long line gets a [`LongPlan`]: with soft wrap off,
//! the [`TILE_LEN`]-byte tiles across the text area; with it on, the rows
//! on screen. Both also take the stretch around the active caret and its
//! selection's ends, where following the caret can scroll to this frame, so
//! a frame that scrolls to the caret still has its text. Only the planned
//! bytes are styled and shaped; the rest of the line is measured.

use std::{ops::Range, sync::Arc};

use gpui::{Bounds, Hsla, Pixels, ShapedLine, SharedString, TextRun, Window, px};
use smallvec::SmallVec;

use crate::{
    InputBaseState, InputModeKind, RopeExt as _,
    display_map::{InputLine, LineLayout, LineTile, LongLine, TILE_LEN},
    element::{align_runs_to_char_boundaries, has_background, runs_for_range, split_runs_by_bg_segments},
    layout::WhitespaceIndicators,
};

/// What to lay out of a long line this frame.
pub(crate) enum LongPlan {
    /// Soft wrap off: the tiles' byte ranges, line-relative and ascending.
    Tiles(Vec<Range<usize>>),
    /// Soft wrap on: the rows, ascending.
    Rows(Vec<usize>),
}

impl LongPlan {
    /// The planned bytes, line-relative, ascending and merged; `rows` are the
    /// line's rows.
    pub(crate) fn spans(&self, rows: &[Range<usize>]) -> Vec<Range<usize>> {
        let ranges: Box<dyn Iterator<Item = Range<usize>>> = match self {
            LongPlan::Tiles(tiles) => Box::new(tiles.iter().cloned()),
            LongPlan::Rows(laid_out) => Box::new(laid_out.iter().map(|&row| rows[row].clone())),
        };
        let mut spans: Vec<Range<usize>> = Vec::new();
        for range in ranges {
            match spans.last_mut() {
                Some(last) if last.end == range.start => last.end = range.end,
                _ => spans.push(range),
            }
        }
        spans
    }
}

/// Plans each visible long line, `None` for the others. `viewport` is the
/// text area in content coordinates, before this frame follows the caret.
pub(crate) fn plan_long_lines<M: InputModeKind>(
    state: &InputBaseState<M>,
    visible_buffer_lines: &[usize],
    line_height: Pixels,
    viewport: Bounds<Pixels>,
) -> Vec<Option<LongPlan>> {
    let mut plans = Vec::with_capacity(visible_buffer_lines.len());
    if state.tokens_visible() || state.masked || line_height <= px(0.) {
        plans.resize_with(visible_buffer_lines.len(), || None);
        return plans;
    }
    let selection = state.active_selection();
    let mut points = vec![selection.start, selection.end, selection.cursor_offset()];
    if let Some(marked) = &state.ime_marked_range {
        points.push(marked.end);
    }
    for &buffer_line in visible_buffer_lines {
        let Some(item) = state.display_map.line(buffer_line) else {
            plans.push(None);
            continue;
        };
        let Some(long) = &item.long else {
            plans.push(None);
            continue;
        };
        let line_start = state.text.line_start_offset(buffer_line);
        let near: Vec<usize> = points
            .iter()
            .filter(|&&point| point >= line_start && point <= line_start + long.len())
            .map(|&point| point - line_start)
            .collect();
        plans.push(Some(if state.soft_wrap {
            let top = line_height
                * (state.display_map.buffer_line_to_display_row(buffer_line)
                    + state.block_rows_before(buffer_line));
            LongPlan::Rows(plan_rows(&item.wrapped_lines, &near, top, line_height, viewport))
        } else {
            LongPlan::Tiles(plan_tiles(long, &near, viewport))
        }));
    }
    plans
}

/// The tiles across `viewport` and within a text area's width of each of
/// `near` (line-relative offsets).
fn plan_tiles(long: &LongLine, near: &[usize], viewport: Bounds<Pixels>) -> Vec<Range<usize>> {
    let width = viewport.size.width.as_f32() as f64;
    let left = viewport.left().as_f32() as f64;
    let mut spans = vec![(left, left + width)];
    for &point in near {
        let x = long.x(point);
        spans.push((x - width, x + width));
    }
    let mut tiles: Vec<usize> = Vec::new();
    for (from, to) in spans {
        let first = long.index_for_x(from) / TILE_LEN;
        let last = long.index_for_x(to) / TILE_LEN;
        tiles.extend(first..=last);
    }
    tiles.sort_unstable();
    tiles.dedup();
    let len = long.len();
    tiles
        .into_iter()
        .map(|tile| {
            let start = long.floor_boundary(tile * TILE_LEN);
            let end = if (tile + 1) * TILE_LEN >= len {
                len
            } else {
                long.floor_boundary((tile + 1) * TILE_LEN)
            };
            start..end
        })
        .filter(|range| !range.is_empty())
        .collect()
}

/// The rows of a line starting at `top` (content y) that are in `viewport`,
/// and those within a viewport's height of the rows of `near`.
fn plan_rows(
    rows: &[Range<usize>],
    near: &[usize],
    top: Pixels,
    line_height: Pixels,
    viewport: Bounds<Pixels>,
) -> Vec<usize> {
    let screen_rows = (viewport.size.height / line_height).ceil() as usize + 1;
    let first_on_screen = ((viewport.top() - top) / line_height).floor().max(0.) as usize;
    let mut windows = vec![first_on_screen.saturating_sub(1)..first_on_screen + screen_rows + 1];
    for &point in near {
        let row = rows.partition_point(|row| row.end < point).min(rows.len().saturating_sub(1));
        windows.push(row.saturating_sub(screen_rows + 1)..row + screen_rows + 2);
    }
    let mut laid_out: Vec<usize> = windows
        .into_iter()
        .flat_map(|window| window.start.min(rows.len())..window.end.min(rows.len()))
        .collect();
    laid_out.sort_unstable();
    laid_out.dedup();
    laid_out
}

impl<M: InputModeKind> InputBaseState<M> {
    /// The bytes the last frame shaped: every visible ordinary line's, a
    /// long line's laid-out pieces. For the control channel's checks.
    pub fn laid_out_bytes(&self) -> usize {
        let Some(layout) = &self.last_layout else {
            return 0;
        };
        layout
            .lines
            .iter()
            .map(|line| match line.laid_out_ranges() {
                Some(pieces) => pieces.iter().map(|piece| piece.len()).sum(),
                None => line.len(),
            })
            .sum()
    }
}

/// The runs a piece of a line (`range`, line-relative) is shaped with.
struct PieceRuns<'a> {
    runs: &'a [TextRun],
    /// Where the line starts in `runs`.
    run_offset: usize,
    bg_segments: &'a [(Range<usize>, Hsla)],
    /// Where the line starts in the text the background segments are in.
    display_line_start: usize,
    font_size: Pixels,
}

impl PieceRuns<'_> {
    fn shape(&self, long: &LongLine, range: Range<usize>, window: &mut Window) -> (ShapedLine, bool) {
        let runs = runs_for_range(self.runs, self.run_offset, &range);
        let runs = if self.bg_segments.is_empty() {
            runs
        } else {
            split_runs_by_bg_segments(self.display_line_start + range.start, &runs, self.bg_segments)
        };
        let text: SharedString = long.slice(range).to_string().into();
        let runs = align_runs_to_char_boundaries(&text, &runs).unwrap_or(runs);
        let background = has_background(&runs);
        (window.text_system().shape_line(text, self.font_size, &runs, None), background)
    }
}

/// Lays out a long line as `plan`. `runs` cover every visible line's bytes,
/// the line's from `run_offset`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_long_line<M: InputModeKind>(
    state: &InputBaseState<M>,
    buffer_line: usize,
    long: &Arc<LongLine>,
    plan: &LongPlan,
    runs: &[TextRun],
    run_offset: usize,
    bg_segments: &[(Range<usize>, Hsla)],
    display_line_start: usize,
    font_size: Pixels,
    whitespace_indicators: Option<WhitespaceIndicators>,
    block_height: Pixels,
    window: &mut Window,
) -> LineLayout {
    let pieces = PieceRuns {
        runs,
        run_offset,
        bg_segments,
        display_line_start,
        font_size,
    };
    let mut background = false;
    let layout = match plan {
        LongPlan::Tiles(tiles) => {
            let tiles = tiles
                .iter()
                .map(|range| {
                    let (line, has_background) = pieces.shape(long, range.clone(), window);
                    background |= has_background;
                    LineTile {
                        range: range.clone(),
                        x: px(long.x(range.start) as f32),
                        line,
                    }
                })
                .collect();
            LineLayout::new().inline_lines(smallvec::smallvec![InputLine::tiled(long.clone(), tiles)])
        }
        LongPlan::Rows(laid_out) => {
            let item = state
                .display_map
                .line(buffer_line)
                .expect("line should exists in wrapper");
            let lines: SmallVec<[InputLine; 1]> = laid_out
                .iter()
                .map(|&row| {
                    let (line, has_background) = pieces.shape(long, item.wrapped_lines[row].clone(), window);
                    background |= has_background;
                    InputLine::from(line)
                })
                .collect();
            // Continuation rows keep the first row's indentation, spaces.
            let wrap_indent = if item.indent > 0 && item.wrapped_lines.len() > 1 {
                px(long.measure(0..item.indent as usize) as f32)
            } else {
                px(0.)
            };
            LineLayout::new()
                .long_rows(long.clone(), item.wrapped_lines.clone(), laid_out.clone(), lines)
                .wrap_indent(wrap_indent)
        }
    };
    layout
        .with_background(background)
        .with_whitespaces(whitespace_indicators)
        .extra_height(block_height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    fn viewport(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[test]
    fn spans_merge_touching_pieces() {
        let plan = LongPlan::Tiles(vec![0..256, 256..512, 1024..1280]);
        assert_eq!(plan.spans(&[]), vec![0..512, 1024..1280]);
        let rows = vec![0..10, 10..20, 20..30, 30..40];
        assert_eq!(LongPlan::Rows(vec![0, 1, 3]).spans(&rows), vec![0..20, 30..40]);
    }

    #[test]
    fn rows_on_screen_and_around_the_caret_are_planned() {
        let rows: Vec<Range<usize>> = (0..1000).map(|row| row * 10..row * 10 + 10).collect();
        // The line starts 100 px down; the view shows y 300..500 at 20 px a row.
        let planned = plan_rows(&rows, &[], px(100.), px(20.), viewport(0., 300., 500., 200.));
        assert_eq!(planned, (9..22).collect::<Vec<_>>());
        // A caret far below brings the rows around it.
        let planned = plan_rows(&rows, &[9_005], px(100.), px(20.), viewport(0., 300., 500., 200.));
        assert!(planned.contains(&900) && planned.contains(&888) && planned.contains(&911));
        assert!(!planned.contains(&500));
        assert!(planned.windows(2).all(|pair| pair[0] < pair[1]));
        // Never past the line's rows.
        let planned = plan_rows(&rows, &[], px(0.), px(20.), viewport(0., 19_990., 500., 200.));
        assert_eq!(planned, vec![998, 999]);
    }
}

/// Editing a long line in a real editor: what's laid out, and that the
/// caret, selections, hit-testing, search highlights and several cursors
/// agree with the text.
#[cfg(test)]
mod editor_tests {
    use gpui::{Context, Entity, EntityInputHandler as _, TestAppContext, VisualTestContext};

    use crate::{
        EditorState, MoveDown, MoveEnd, MoveHome, RopeExt as _, element::TextElement,
        test_support::open_editor,
    };

    /// An editor, soft wrap off, on a short line, a ~53 KB line of minified
    /// code and a short line.
    fn open(cx: &mut TestAppContext) -> (Entity<EditorState>, VisualTestContext) {
        let (editor, mut cx) = open_editor(cx, &text());
        update(&mut cx, &editor, |state, window, cx| {
            state.set_soft_wrap(false, window, cx);
            state.focus(window, cx);
        });
        (editor, cx)
    }

    /// A short line, a ~53 KB line of minified code, a short line.
    fn text() -> String {
        let long: String = (0..1500).map(|ix| format!("var a{ix}=function(b){{return b*{ix}}};")).collect();
        format!("first\n{long}\nlast\n")
    }

    fn line_start(state: &EditorState) -> usize {
        state.text.line_start_offset(1)
    }

    fn update<R>(
        cx: &mut VisualTestContext,
        editor: &Entity<EditorState>,
        f: impl FnOnce(&mut EditorState, &mut gpui::Window, &mut Context<EditorState>) -> R,
    ) -> R {
        let editor = editor.clone();
        let result = cx.update(|window, cx| editor.update(cx, |state, cx| f(state, window, cx)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        result
    }

    /// The long line's laid-out bytes (line-relative).
    fn laid_out(cx: &mut VisualTestContext, editor: &Entity<EditorState>) -> Vec<std::ops::Range<usize>> {
        editor.read_with(cx, |state, _| {
            let layout = state.last_layout.as_ref().unwrap();
            layout.line(1).unwrap().laid_out_ranges().expect("laid out in pieces")
        })
    }

    #[gpui::test]
    fn only_the_view_and_the_caret_are_laid_out(cx: &mut TestAppContext) {
        let (editor, mut cx) = open(cx);
        let len = editor.read_with(&cx, |state, _| state.text.slice_line(1).len());
        let pieces = laid_out(&mut cx, &editor);
        assert_eq!(pieces.first().unwrap().start, 0);
        let shaped: usize = pieces.iter().map(|piece| piece.len()).sum();
        assert!(shaped < len / 10, "{shaped} of {len} bytes shaped");

        // The caret at the line's end scrolls there, and the text around it
        // is laid out in the same frame.
        update(&mut cx, &editor, |state, _, cx| {
            let end = line_start(state) + len;
            state.move_to(end, None, cx);
        });
        let pieces = laid_out(&mut cx, &editor);
        assert_eq!(pieces.last().unwrap().end, len);
        editor.read_with(&cx, |state, _| {
            let caret = state.last_layout.as_ref().unwrap().cursor_bounds.unwrap();
            let bounds = state.input_bounds;
            assert!(state.scroll_handle.offset().x < gpui::px(-1000.));
            assert!(caret.left() >= bounds.left() && caret.right() <= bounds.right(), "{caret:?} in {bounds:?}");
            // Its x is the line's measured x of the end.
            let layout = state.last_layout.as_ref().unwrap();
            let line = layout.line(1).unwrap();
            let long = state.display_map.line(1).unwrap().long.clone().unwrap();
            let x = line.position_for_index(len, layout, false).unwrap().x;
            assert!((x.as_f32() as f64 - long.x(len)).abs() < 1.);
        });
    }

    #[gpui::test]
    fn positions_and_hit_testing_round_trip_on_and_off_screen(cx: &mut TestAppContext) {
        let (editor, mut cx) = open(cx);
        update(&mut cx, &editor, |state, _, cx| {
            let middle = line_start(state) + 20_000;
            state.move_to(middle, None, cx);
        });
        editor.read_with(&cx, |state, _| {
            let layout = state.last_layout.as_ref().unwrap();
            let line = layout.line(1).unwrap();
            let pieces = line.laid_out_ranges().unwrap();
            // Inside a tile, beside it, and far from anything shaped.
            let shaped = pieces[0].start + 7;
            for ix in [shaped, 20_000, 20_003, 3, 40_001, line.len()] {
                let position = line.position_for_index(ix, layout, false).unwrap();
                let (back, _) = line.closest_index_for_position(position, layout).unwrap();
                assert_eq!(back, ix, "at {ix}");
            }
            // Later offsets are further right.
            let x = |ix| line.position_for_index(ix, layout, false).unwrap().x;
            assert!(x(10) < x(20_000) && x(20_000) < x(20_001) && x(20_001) < x(45_000));
        });
    }

    #[gpui::test]
    fn typing_with_several_cursors_on_a_long_line(cx: &mut TestAppContext) {
        let (editor, mut cx) = open(cx);
        let expected = update(&mut cx, &editor, |state, window, cx| {
            let start = line_start(state);
            state.set_selected_range(start + 30_000..start + 30_000, cx);
            state.add_selection(start + 10..start + 10, cx);
            state.add_selection(start + 45_000..start + 45_000, cx);
            let mut expected = state.value().to_string();
            for at in [start + 45_000, start + 30_000, start + 10] {
                expected.insert_str(at, "XY");
            }
            state.replace_text_in_range(None, "XY", window, cx);
            expected
        });
        let expected_len = expected.lines().nth(1).unwrap().len();
        editor.read_with(&cx, |state, _| {
            assert_eq!(state.value().to_string(), expected);
            let item = state.display_map.line(1).unwrap();
            assert_eq!(item.long.as_ref().unwrap().len(), expected_len);
            assert_eq!(item.len(), expected_len);
        });
        // Backspace removes each again.
        update(&mut cx, &editor, |state, window, cx| {
            state.backspace(&crate::Backspace, window, cx);
            state.backspace(&crate::Backspace, window, cx);
        });
        editor.read_with(&cx, |state, _| assert_eq!(state.value().to_string(), text()));
    }

    #[gpui::test]
    fn home_end_selection_and_search_on_a_long_line(cx: &mut TestAppContext) {
        let (editor, mut cx) = open(cx);
        update(&mut cx, &editor, |state, window, cx| {
            let start = line_start(state);
            state.move_to(start + 100, None, cx);
            state.end(&MoveEnd, window, cx);
        });
        let len = editor.read_with(&cx, |state, _| {
            let len = state.text.slice_line(1).len();
            assert_eq!(state.cursor(), line_start(state) + len);
            len
        });
        update(&mut cx, &editor, |state, window, cx| state.home(&MoveHome, window, cx));
        editor.read_with(&cx, |state, _| assert_eq!(state.cursor(), line_start(state)));

        // Selecting the whole line draws one row, within the view (plus a margin).
        update(&mut cx, &editor, |state, _, cx| {
            let start = line_start(state);
            state.set_selected_range(start..start + len, cx);
        });
        editor.read_with(&cx, |state, _| {
            let layout = state.last_layout.as_ref().unwrap();
            let start = line_start(state);
            let corners = TextElement::<crate::EditorMode>::layout_range_corners(&(start..start + len), layout).unwrap();
            assert_eq!(corners.len(), 1);
            let view = layout.viewport;
            assert!(corners[0].top_left.x >= view.left() - gpui::px(100.));
            assert!(corners[0].top_right.x <= view.right() + gpui::px(100.));
        });

        // Search highlights are laid out only where the line is.
        update(&mut cx, &editor, |state, _, cx| state.set_search_query("return", false, cx));
        editor.read_with(&cx, |state, _| assert_eq!(state.search_session().matcher.matched_ranges().len(), 1500));
        let painted = cx.update(|_, cx| {
            let state = editor.read(cx);
            let layout = state.last_layout.clone().unwrap();
            TextElement::new(editor.clone()).layout_search_matches(&layout, &state.input_bounds.clone(), cx).len()
        });
        assert!(painted > 0 && painted < 100, "{painted} of 1500 matches laid out");
    }

    #[gpui::test]
    fn soft_wrapped_long_line_lays_out_the_rows_on_screen(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, &text());
        update(&mut cx, &editor, |state, window, cx| {
            state.focus(window, cx);
            state.set_soft_wrap(true, window, cx);
        });
        let (rows, start) = editor.read_with(&cx, |state, _| {
            let layout = state.last_layout.as_ref().unwrap();
            let line = layout.line(1).unwrap();
            assert!(line.row_count() > 100, "{} rows", line.row_count());
            assert!(line.wrapped_lines.len() < 100);
            (line.row_count(), line_start(state))
        });

        // Down moves a row at a time within the line.
        update(&mut cx, &editor, |state, _, cx| state.move_to(start, None, cx));
        update(&mut cx, &editor, |state, window, cx| {
            state.down(&MoveDown, window, cx);
            state.down(&MoveDown, window, cx);
        });
        editor.read_with(&cx, |state, _| {
            let item = state.display_map.line(1).unwrap();
            let local = state.cursor() - start;
            assert!(item.wrapped_lines[2].contains(&local), "{local} in {:?}", item.wrapped_lines[2]);
        });

        // The last row is laid out when the caret goes there, and the caret
        // is drawn on screen.
        update(&mut cx, &editor, |state, window, cx| {
            state.end(&MoveEnd, window, cx);
            let end = start + state.text.slice_line(1).len();
            state.move_to(end, None, cx);
        });
        editor.read_with(&cx, |state, _| {
            let layout = state.last_layout.as_ref().unwrap();
            let line = layout.line(1).unwrap();
            let pieces = line.laid_out_ranges().unwrap();
            assert_eq!(pieces.last().unwrap().end, line.len());
            // Caret bounds are kept before vertical scrolling.
            let caret = layout.cursor_bounds.unwrap();
            let top = caret.top() - state.input_bounds.top();
            let view = layout.viewport;
            assert!(top >= view.top() && top + caret.size.height <= view.bottom(), "{caret:?} in {view:?}");
            assert_eq!(line.row_count(), rows);
        });

        // Typing near the start re-wraps the line as wrapping it again would.
        update(&mut cx, &editor, |state, window, cx| {
            state.move_to(start + 5, None, cx);
            state.replace_text_in_range(None, "inserted words ", window, cx);
        });
        editor.read_with(&cx, |state, _| {
            let item = state.display_map.line(1).unwrap();
            assert_eq!(item.wrapped_lines.last().unwrap().end, item.len());
            for pair in item.wrapped_lines.windows(2) {
                assert_eq!(pair[0].end, pair[1].start);
            }
        });
    }
}

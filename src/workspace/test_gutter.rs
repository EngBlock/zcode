//! Test diamonds in the editor gutter, as in Xcode: one on each test class,
//! suite and test function, showing the last result (outline: not run,
//! green check: passed, red cross: failed). Hovering shows a play button;
//! clicking runs that test or class.
//!
//! The diamonds are a gutter lane over the line numbers (`zcode-editor`'s
//! `gutter.rs`), so the editor lays them out with its lines and they follow
//! scrolling, folding and soft wrap.

use gpui_kit::component::ActiveTheme as _;
use zcode_editor::{EditorState, GutterItem, GutterLane};
use gpui_kit::*;

use super::Workspace;
use crate::build::log::TestStatus;

/// The gutter lane of test diamonds.
pub(super) const TEST_LANE: &str = "tests";
const DIAMOND_SIZE: f32 = 14.;

pub(super) struct DiamondColors {
    pub idle: Hsla,
    pub run: Hsla,
    pub passed: Hsla,
    pub failed: Hsla,
    pub skipped: Hsla,
    pub glyph: Hsla,
}

impl DiamondColors {
    pub fn new(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            idle: theme.muted_foreground,
            run: theme.primary,
            passed: theme.success,
            failed: theme.danger,
            skipped: theme.muted_foreground,
            glyph: gpui_kit::white(),
        }
    }
}

/// Paints one diamond centered in `bounds`: an outline until the test has
/// run, then filled with a check or cross; a play triangle while hovered.
pub(super) fn paint_diamond(window: &mut Window, bounds: Bounds<Pixels>, status: Option<TestStatus>, hovered: bool, colors: &DiamondColors) {
    let c = bounds.center();
    let r = bounds.size.width.min(bounds.size.height) / 2.;
    let corners = [point(c.x, c.y - r), point(c.x + r, c.y), point(c.x, c.y + r), point(c.x - r, c.y)];
    let fill = match (hovered, status) {
        (true, _) => Some(colors.run),
        (false, Some(TestStatus::Passed)) => Some(colors.passed),
        (false, Some(TestStatus::Failed)) => Some(colors.failed),
        (false, Some(TestStatus::Skipped)) => Some(colors.skipped),
        (false, None) => None,
    };
    match fill {
        Some(color) => {
            let mut path = PathBuilder::fill();
            path.add_polygon(&corners, true);
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        }
        None => {
            let mut path = PathBuilder::stroke(px(1.25));
            path.add_polygon(&corners, true);
            if let Ok(path) = path.build() {
                window.paint_path(path, colors.idle);
            }
            return;
        }
    }
    let u = r * 0.36;
    if hovered {
        let mut play = PathBuilder::fill();
        play.add_polygon(&[point(c.x - u * 0.7, c.y - u), point(c.x + u, c.y), point(c.x - u * 0.7, c.y + u)], true);
        if let Ok(path) = play.build() {
            window.paint_path(path, colors.glyph);
        }
        return;
    }
    let mut glyph = PathBuilder::stroke(px(1.5));
    match status {
        Some(TestStatus::Passed) => {
            glyph.move_to(point(c.x - u, c.y));
            glyph.line_to(point(c.x - u * 0.25, c.y + u * 0.75));
            glyph.line_to(point(c.x + u, c.y - u * 0.7));
        }
        Some(TestStatus::Failed) => {
            glyph.move_to(point(c.x - u * 0.75, c.y - u * 0.75));
            glyph.line_to(point(c.x + u * 0.75, c.y + u * 0.75));
            glyph.move_to(point(c.x + u * 0.75, c.y - u * 0.75));
            glyph.line_to(point(c.x - u * 0.75, c.y + u * 0.75));
        }
        _ => {
            glyph.move_to(point(c.x - u * 0.75, c.y));
            glyph.line_to(point(c.x + u * 0.75, c.y));
        }
    }
    if let Ok(path) = glyph.build() {
        window.paint_path(path, colors.glyph);
    }
}

/// A diamond as an inline element (the Test navigator's status icons).
pub(super) fn diamond_icon(status: Option<TestStatus>, cx: &App) -> impl IntoElement {
    let colors = DiamondColors::new(cx);
    canvas(|_, _, _| {}, move |bounds, _, window, _| paint_diamond(window, bounds, status, false, &colors))
        .size(px(12.))
        .flex_none()
}

impl Workspace {
    /// The diamonds in `editor`'s gutter, over the line numbers.
    pub(super) fn test_lane(&self, editor: &Entity<EditorState>, cx: &Context<Self>) -> GutterLane {
        let workspace = cx.weak_entity();
        let editor_id = editor.entity_id();
        GutterLane::new(TEST_LANE, move |rows, cx| {
            let Some(this) = workspace.upgrade() else { return vec![] };
            let this = this.read(cx);
            let Some(path) = this.buffer_path_of(editor_id) else { return vec![] };
            let statuses = this.current_test_statuses(cx);
            let colors = std::rc::Rc::new(DiamondColors::new(cx));
            let mut items: Vec<GutterItem> = vec![];
            let mut last_line = None;
            for (ix, item) in this.tests.index.items.iter().enumerate() {
                // One diamond per line; a class or suite sorts before its tests.
                if item.file != path || last_line == Some(item.line) || !rows.rows.contains(&item.line) {
                    continue;
                }
                last_line = Some(item.line);
                let status = statuses.get(ix).copied().flatten();
                let colors = colors.clone();
                let workspace = workspace.clone();
                items.push(
                    GutterItem::new(item.line, move |paint, window, _| {
                        // The diamond replaces the line number, as in Xcode.
                        let target = paint.bounds;
                        let cover = Bounds::new(
                            point(paint.cell.left(), target.origin.y - px(1.)),
                            size(target.right() - paint.cell.left() + px(2.), target.size.height + px(2.)),
                        );
                        window.paint_quad(fill(cover, paint.background));
                        paint_diamond(window, target, status, paint.hovered, &colors);
                    })
                    // At the right of the number column.
                    .bounds(|cell, line_height| {
                        let side = px(DIAMOND_SIZE).min(line_height - px(2.));
                        let right = cell.left() + cell.size.width.max(px(DIAMOND_SIZE));
                        Bounds::new(point(right - side - px(2.), cell.top() + (line_height - side) / 2.), size(side, side))
                    })
                    .cursor(CursorStyle::PointingHand)
                    .on_click(move |_, window, cx| workspace.update(cx, |this, cx| this.run_test_items(vec![ix], window, cx)).is_ok()),
                );
            }
            items
        })
        .order(2)
    }

    /// The test item each painted diamond stands for: (bounds, line, item).
    pub(super) fn painted_diamonds(&self, cx: &App) -> Vec<(Bounds<Pixels>, u32, usize)> {
        let Some(path) = self.active_buffer().map(|b| b.path.clone()) else { return vec![] };
        self.painted_gutter(TEST_LANE, cx)
            .into_iter()
            .filter_map(|(line, bounds)| {
                let ix = self.tests.index.items.iter().position(|item| item.file == path && item.line + 1 == line)?;
                Some((bounds, line, ix))
            })
            .collect()
    }
}

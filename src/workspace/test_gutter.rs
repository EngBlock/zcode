//! Test diamonds in the editor gutter, as in Xcode: one on each test class,
//! suite and test function, showing the last result (outline: not run,
//! green check: passed, red cross: failed). Hovering shows a play button;
//! clicking runs that test or class.
//!
//! The editor engine has no gutter-marker API, so the diamonds are a canvas
//! layered over the editor. It paints after the editor in the same frame
//! and asks it where each line landed, so the diamonds follow scrolling,
//! folding and soft wrap without lagging a frame.

use std::cell::{Cell, RefCell};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{EditorState, RopeExt as _};
use gpui_kit::*;

use super::Workspace;
use crate::build::log::TestStatus;

/// The editor gutter's fold-chevron column and the space before it, to the
/// right of the line numbers (`gpui-base`'s `FOLD_ICON_HITBOX_WIDTH` and
/// `LINE_NUMBER_RIGHT_MARGIN`).
const FOLD_COLUMN: f32 = 18. + 6.;
const DIAMOND_SIZE: f32 = 14.;

/// A diamond to draw: the 0-based line, the test item and its status.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Diamond {
    pub line: u32,
    pub item: usize,
    pub status: Option<TestStatus>,
}

/// Where diamonds were painted last frame (window coordinates), for clicks
/// and for the test harness.
#[derive(Default)]
pub(super) struct GutterState {
    pub painted: RefCell<Vec<(Bounds<Pixels>, Diamond)>>,
    pub hovered: Cell<Option<usize>>,
}

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

/// The width of the editor's gutter (line numbers and fold chevrons): how
/// far right of the text bounds a visible line's first character starts.
fn gutter_width(state: &EditorState) -> Option<Pixels> {
    let rows = state.visible_row_range()?;
    let text = state.text();
    let row = rows.start.min(text.lines_len().saturating_sub(1));
    let offset = text.line_start_offset(row);
    let line = state.range_to_bounds(&(offset..offset))?;
    Some(line.origin.x - state.text_bounds()?.origin.x)
}

impl Workspace {
    /// The diamonds overlay for `editor`, or `None` when the file has no tests.
    pub(super) fn render_test_gutter(&mut self, editor: &Entity<EditorState>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let path = self.active_buffer()?.path.clone();
        let statuses = self.test_statuses(cx);
        let mut diamonds: Vec<Diamond> = vec![];
        for (ix, item) in self.tests.index.items.iter().enumerate() {
            // One diamond per line; a class or suite sorts before its tests.
            if item.file == path && diamonds.last().is_none_or(|d| d.line != item.line) {
                diamonds.push(Diamond { line: item.line, item: ix, status: statuses.get(ix).copied().flatten() });
            }
        }
        if diamonds.is_empty() {
            self.gutter.painted.borrow_mut().clear();
            return None;
        }
        let editor = editor.clone();
        let gutter = self.gutter.clone();
        let workspace = cx.weak_entity();
        let colors = DiamondColors::new(cx);
        let background = cx
            .theme()
            .highlight_theme
            .style
            .editor_gutter_background
            .or(cx.theme().highlight_theme.style.editor_background)
            .unwrap_or(cx.theme().background);
        Some(
            canvas(
                {
                    let editor = editor.clone();
                    move |_, window, cx| {
                        // The gutter strip, from the editor's last layout.
                        let state = editor.read(cx);
                        let input = state.input_bounds();
                        let strip = Bounds::new(input.origin, size(gutter_width(state)?, input.size.height));
                        Some(window.insert_hitbox(strip, HitboxBehavior::Normal))
                    }
                },
                move |_, hitbox, window, cx| {
                    let Some(hitbox) = hitbox else { return };
                    let state = editor.read(cx);
                    let (Some(width), Some(line_height)) = (gutter_width(state), state.line_height()) else { return };
                    let input = state.input_bounds();
                    // The line numbers' column, left of the fold chevrons.
                    let numbers_right = input.origin.x + width - px(FOLD_COLUMN);
                    let rope = state.text().clone();
                    let mouse = window.mouse_position();
                    let mut painted = vec![];
                    for diamond in &diamonds {
                        let row = diamond.line as usize;
                        if row >= rope.lines_len() {
                            continue;
                        }
                        let offset = rope.line_start_offset(row);
                        let Some(line) = state.range_to_bounds(&(offset..offset)) else { continue };
                        let column = Bounds::new(point(input.origin.x, line.origin.y), size((numbers_right - input.origin.x).max(px(DIAMOND_SIZE)), line_height));
                        if !input.intersects(&column) {
                            continue;
                        }
                        let side = px(DIAMOND_SIZE).min(line_height - px(2.));
                        let target = Bounds::new(
                            point(column.right() - side - px(2.), column.origin.y + (line_height - side) / 2.),
                            size(side, side),
                        );
                        painted.push((target, *diamond));
                    }
                    let hovered = painted
                        .iter()
                        .find(|(b, _)| hitbox.is_hovered(window) && b.dilate(px(2.)).contains(&mouse))
                        .map(|(_, d)| d.item);
                    for (target, diamond) in &painted {
                        // The diamond replaces the line number, as in Xcode.
                        let cover = Bounds::new(
                            point(input.origin.x, target.origin.y - px(1.)),
                            size(target.right() - input.origin.x + px(2.), target.size.height + px(2.)),
                        );
                        window.paint_quad(fill(cover, background));
                        paint_diamond(window, *target, diamond.status, hovered == Some(diamond.item), &colors);
                    }
                    if hovered.is_some() {
                        window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
                    }
                    if gutter.hovered.get() != hovered {
                        gutter.hovered.set(hovered);
                    }
                    *gutter.painted.borrow_mut() = painted.clone();

                    let gutter_for_move = gutter.clone();
                    let move_hitbox = hitbox.clone();
                    let targets = painted.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        let now = targets
                            .iter()
                            .find(|(b, _)| move_hitbox.is_hovered(window) && b.dilate(px(2.)).contains(&event.position))
                            .map(|(_, d)| d.item);
                        if now != gutter_for_move.hovered.get() {
                            window.refresh();
                        }
                    });
                    let workspace = workspace.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !hitbox.is_hovered(window) {
                            return;
                        }
                        let Some((_, diamond)) = painted.iter().find(|(b, _)| b.dilate(px(2.)).contains(&event.position)) else { return };
                        cx.stop_propagation();
                        let item = diamond.item;
                        workspace.update(cx, |this, cx| this.run_test_items(vec![item], window, cx)).ok();
                    });
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into_any_element(),
        )
    }
}

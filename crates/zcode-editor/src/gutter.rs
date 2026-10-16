//! Gutter decorations: per-line items in lanes, painted by the editor and
//! hit-tested by it.
//!
//! A lane is one kind of mark (breakpoints, bookmarks, test diamonds, git
//! changes). It has a place in the gutter and an order: lanes before or after
//! the line numbers get columns of their own, laid out left to right by
//! order; lanes over the line numbers share the number column and stack by
//! order, the last on top (Xcode draws breakpoints and test diamonds over
//! the numbers). Each frame the editor asks every lane for the items on the
//! visible rows, so a lane reads its owner's model directly and is never
//! stale.
//!
//! An item paints itself in its cell and may take the pointer: click,
//! double-click, secondary click and hover. A click goes to the topmost item
//! under the pointer that handles it; when none does, to the topmost lane
//! with a handler for clicks on rows without an item (adding a breakpoint).

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, Bounds, CursorStyle, Font, Hsla, Modifiers, MouseButton, Pixels, Point, SharedString,
    TextAlign, TextRun, Window, point, px, size,
};

use ropey::Rope;

use crate::anchors::{Anchor, Anchors};
use crate::EditorState;

/// Where a lane sits in the gutter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GutterPlacement {
    /// A column of this width left of the line numbers.
    BeforeNumbers(Pixels),
    /// Over the line numbers, sharing their column.
    OverNumbers,
    /// A column of this width between the line numbers and the fold chevrons.
    AfterNumbers(Pixels),
}

/// A pointer event on a gutter row.
#[derive(Clone, Debug)]
pub struct GutterEvent {
    /// The 0-based buffer row.
    pub row: u32,
    pub position: Point<Pixels>,
    pub button: MouseButton,
    pub click_count: usize,
    pub modifiers: Modifiers,
    /// The item's bounds (or the lane cell's, for a row without an item).
    pub bounds: Bounds<Pixels>,
}

/// What an item paints with.
pub struct GutterPaint {
    /// The 0-based buffer row.
    pub row: u32,
    /// The item's bounds: its cell, or what [`GutterItem::bounds`] chose.
    pub bounds: Bounds<Pixels>,
    /// The lane's cell on this row (for a lane over the numbers, the number
    /// column).
    pub cell: Bounds<Pixels>,
    /// The line number's column on this row.
    pub numbers: Bounds<Pixels>,
    pub line_height: Pixels,
    pub hovered: bool,
    /// The gutter's background, to cover what an item replaces.
    pub background: Hsla,
    font: Font,
    font_size: Pixels,
}

impl GutterPaint {
    /// Paints this row's line number in `color`, right-aligned in the number
    /// column as the editor paints it.
    pub fn paint_line_number(&self, color: Hsla, window: &mut Window, cx: &mut App) {
        let number: SharedString = (self.row + 1).to_string().into();
        let run = TextRun {
            len: number.len(),
            font: self.font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window
            .text_system()
            .shape_line(number, self.font_size, &[run], None);
        let origin = point(self.numbers.right() - shaped.width, self.numbers.top());
        shaped
            .paint(origin, self.line_height, TextAlign::Left, None, window, cx)
            .ok();
    }

    /// Paints `text` on this row within `span` (an x range), left- or
    /// right-aligned, in the editor's font at `scale` of its size, cut short
    /// with an ellipsis to fit. Returns the painted width.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_label(
        &self,
        text: &str,
        span: Range<Pixels>,
        align_right: bool,
        scale: f32,
        color: Hsla,
        window: &mut Window,
        cx: &mut App,
    ) -> Pixels {
        let font_size = self.font_size * scale;
        let max_width = span.end - span.start;
        let shape = |text: SharedString, window: &mut Window| {
            let run = TextRun {
                len: text.len(),
                font: self.font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window.text_system().shape_line(text, font_size, &[run], None)
        };
        let mut shaped = shape(SharedString::from(text.to_string()), window);
        let mut chars: Vec<char> = text.chars().collect();
        while shaped.width > max_width && !chars.is_empty() {
            chars.pop();
            let cut: String = chars.iter().collect::<String>().trim_end().to_string() + "…";
            shaped = shape(cut.into(), window);
        }
        if text.is_empty() || shaped.width > max_width {
            return px(0.);
        }
        let x = if align_right { span.end - shaped.width } else { span.start };
        shaped
            .paint(point(x, self.cell.top()), self.line_height, TextAlign::Left, None, window, cx)
            .ok();
        shaped.width
    }
}

type PaintFn = Rc<dyn Fn(&GutterPaint, &mut Window, &mut App)>;
type HandlerFn = Rc<dyn Fn(&GutterEvent, &mut Window, &mut App) -> bool>;
type HoverFn = Rc<dyn Fn(bool, &mut Window, &mut App)>;
type BoundsFn = Rc<dyn Fn(Bounds<Pixels>, Pixels) -> Bounds<Pixels>>;
type ItemsFn = Rc<dyn Fn(&GutterRows, &App) -> Vec<GutterItem>>;

/// What a lane's items are computed from: the visible rows and the buffer.
pub struct GutterRows<'a> {
    /// The visible 0-based buffer rows.
    pub rows: Range<u32>,
    text: &'a Rope,
    anchors: &'a Anchors,
}

impl GutterRows<'_> {
    pub fn text(&self) -> &Rope {
        self.text
    }

    /// An anchor's 0-based row, or `None` while its line is deleted.
    pub fn anchor_row(&self, anchor: Anchor) -> Option<u32> {
        self.anchors.row(anchor, self.text)
    }
}

/// One mark on one row.
#[derive(Clone)]
pub struct GutterItem {
    row: u32,
    paint: PaintFn,
    bounds: Option<BoundsFn>,
    on_click: Option<HandlerFn>,
    on_double_click: Option<HandlerFn>,
    on_secondary_click: Option<HandlerFn>,
    pub(crate) on_hover: Option<HoverFn>,
    cursor: Option<CursorStyle>,
}

impl GutterItem {
    /// An item on 0-based buffer row `row`.
    pub fn new(row: u32, paint: impl Fn(&GutterPaint, &mut Window, &mut App) + 'static) -> Self {
        Self {
            row,
            paint: Rc::new(paint),
            bounds: None,
            on_click: None,
            on_double_click: None,
            on_secondary_click: None,
            on_hover: None,
            cursor: None,
        }
    }

    /// Narrows the item to part of its cell (`f(cell, line_height)`): what
    /// it paints in and what it hit-tests. The default is the whole cell.
    pub fn bounds(mut self, f: impl Fn(Bounds<Pixels>, Pixels) -> Bounds<Pixels> + 'static) -> Self {
        self.bounds = Some(Rc::new(f));
        self
    }

    /// A primary click (the first of a double-click too). Return `true` when
    /// handled; otherwise the click goes on to what's below.
    pub fn on_click(mut self, f: impl Fn(&GutterEvent, &mut Window, &mut App) -> bool + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    /// The second click of a double-click.
    pub fn on_double_click(mut self, f: impl Fn(&GutterEvent, &mut Window, &mut App) -> bool + 'static) -> Self {
        self.on_double_click = Some(Rc::new(f));
        self
    }

    /// A secondary (right or control) click, for a context menu.
    pub fn on_secondary_click(mut self, f: impl Fn(&GutterEvent, &mut Window, &mut App) -> bool + 'static) -> Self {
        self.on_secondary_click = Some(Rc::new(f));
        self
    }

    /// The pointer entering (`true`) or leaving the item. The item is
    /// repainted either way, with [`GutterPaint::hovered`].
    pub fn on_hover(mut self, f: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Rc::new(f));
        self
    }

    /// The pointer's shape over the item.
    pub fn cursor(mut self, cursor: CursorStyle) -> Self {
        self.cursor = Some(cursor);
        self
    }

    pub fn row(&self) -> u32 {
        self.row
    }
}

/// A kind of gutter mark.
#[derive(Clone)]
pub struct GutterLane {
    id: SharedString,
    order: i32,
    placement: GutterPlacement,
    items: ItemsFn,
    on_click: Option<HandlerFn>,
    on_secondary_click: Option<HandlerFn>,
}

impl GutterLane {
    /// A lane over the line numbers whose items come from `items(rows, cx)`,
    /// called each frame for the visible rows.
    pub fn new(
        id: impl Into<SharedString>,
        items: impl Fn(&GutterRows, &App) -> Vec<GutterItem> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            order: 0,
            placement: GutterPlacement::OverNumbers,
            items: Rc::new(items),
            on_click: None,
            on_secondary_click: None,
        }
    }

    /// Left to right among columns, bottom to top over the numbers.
    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub fn placement(mut self, placement: GutterPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// A primary click on a row where no item took it, in this lane's cell.
    pub fn on_click(mut self, f: impl Fn(&GutterEvent, &mut Window, &mut App) -> bool + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    /// A secondary click on a row where no item took it.
    pub fn on_secondary_click(mut self, f: impl Fn(&GutterEvent, &mut Window, &mut App) -> bool + 'static) -> Self {
        self.on_secondary_click = Some(Rc::new(f));
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }
}

/// An item as painted last frame, in window coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintedGutterItem {
    pub lane: SharedString,
    pub row: u32,
    pub bounds: Bounds<Pixels>,
}

/// The gutter's geometry last frame, in window coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GutterGeometry {
    /// The number column (x range of the whole gutter height).
    pub numbers: Bounds<Pixels>,
    /// Each visible row's 0-based buffer row, top and height.
    pub rows: Vec<(u32, Pixels, Pixels)>,
}

#[derive(Default)]
pub(crate) struct Gutter {
    lanes: Vec<GutterLane>,
    pub(crate) hovered: Option<(SharedString, u32)>,
    pub(crate) painted: Vec<PaintedGutterItem>,
    pub(crate) geometry: GutterGeometry,
}

impl Gutter {
    /// Width of the lanes placed before and after the numbers.
    pub(crate) fn column_widths(&self) -> (Pixels, Pixels) {
        let mut before = px(0.);
        let mut after = px(0.);
        for lane in &self.lanes {
            match lane.placement {
                GutterPlacement::BeforeNumbers(width) => before += width,
                GutterPlacement::AfterNumbers(width) => after += width,
                GutterPlacement::OverNumbers => {}
            }
        }
        (before, after)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lanes.is_empty()
    }
}

/// A lane's items on the visible rows, laid out for this frame.
pub(crate) struct LaidOutItem {
    pub(crate) lane: usize,
    pub(crate) item: GutterItem,
    pub(crate) cell: Bounds<Pixels>,
    pub(crate) numbers: Bounds<Pixels>,
    pub(crate) bounds: Bounds<Pixels>,
}

/// Everything the element needs to paint and hit-test the gutter.
pub(crate) struct GutterLayout {
    pub(crate) lanes: Vec<GutterLane>,
    /// Each lane's x range (over the numbers: the number column).
    pub(crate) lane_columns: Vec<Range<Pixels>>,
    /// In paint order (lane order, then row).
    pub(crate) items: Vec<LaidOutItem>,
    pub(crate) line_height: Pixels,
    pub(crate) rows: Vec<(u32, Pixels, Pixels)>,
    pub(crate) font: Font,
    pub(crate) font_size: Pixels,
    pub(crate) background: Hsla,
    pub(crate) bounds: Bounds<Pixels>,
}

impl GutterLayout {
    /// Lays out the lanes over `rows` (0-based row, top, height in window
    /// coordinates). `numbers` is the number column's x range; `before` and
    /// `after` start where the before- and after-number columns do.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        gutter: &Gutter,
        text: &Rope,
        anchors: &Anchors,
        bounds: Bounds<Pixels>,
        numbers: Range<Pixels>,
        before_start: Pixels,
        rows: Vec<(u32, Pixels, Pixels)>,
        line_height: Pixels,
        font: Font,
        font_size: Pixels,
        background: Hsla,
        cx: &App,
    ) -> Self {
        let mut lanes: Vec<GutterLane> = gutter.lanes.clone();
        lanes.sort_by_key(|lane| lane.order);
        let mut before_x = before_start;
        let mut after_x = numbers.end;
        let lane_columns: Vec<Range<Pixels>> = lanes
            .iter()
            .map(|lane| match lane.placement {
                GutterPlacement::BeforeNumbers(width) => {
                    before_x += width;
                    before_x - width..before_x
                }
                GutterPlacement::AfterNumbers(width) => {
                    after_x += width;
                    after_x - width..after_x
                }
                GutterPlacement::OverNumbers => numbers.clone(),
            })
            .collect();
        let visible = match (rows.first(), rows.last()) {
            (Some(first), Some(last)) => first.0..last.0 + 1,
            _ => 0..0,
        };
        let mut items = vec![];
        for (ix, lane) in lanes.iter().enumerate() {
            if visible.is_empty() {
                break;
            }
            let column = &lane_columns[ix];
            let query = GutterRows { rows: visible.clone(), text, anchors };
            for item in (lane.items)(&query, cx) {
                let Some(&(_, top, _)) = rows.iter().find(|(row, _, _)| *row == item.row) else {
                    continue;
                };
                let cell = Bounds::new(point(column.start, top), size(column.end - column.start, line_height));
                let numbers = Bounds::new(point(numbers.start, top), size(numbers.end - numbers.start, line_height));
                let item_bounds = item.bounds.as_ref().map_or(cell, |f| f(cell, line_height));
                items.push(LaidOutItem { lane: ix, item, cell, numbers, bounds: item_bounds });
            }
        }
        Self { lanes, lane_columns, items, line_height, rows, font, font_size, background, bounds }
    }

    /// The topmost item under `position`.
    pub(crate) fn item_at(&self, position: Point<Pixels>) -> Option<&LaidOutItem> {
        self.items.iter().rev().find(|item| item.bounds.contains(&position))
    }

    pub(crate) fn row_at(&self, y: Pixels) -> Option<(u32, Pixels)> {
        self.rows
            .iter()
            .find(|(_, top, height)| *top <= y && y < *top + *height)
            .map(|&(row, top, _)| (row, top))
    }

    pub(crate) fn paint(&self, hovered: &Option<(SharedString, u32)>, window: &mut Window, cx: &mut App) {
        for laid in &self.items {
            let lane = &self.lanes[laid.lane];
            let paint = GutterPaint {
                row: laid.item.row,
                bounds: laid.bounds,
                cell: laid.cell,
                numbers: laid.numbers,
                line_height: self.line_height,
                hovered: hovered.as_ref() == Some(&(lane.id.clone(), laid.item.row)),
                background: self.background,
                font: self.font.clone(),
                font_size: self.font_size,
            };
            (laid.item.paint)(&paint, window, cx);
        }
    }

    pub(crate) fn painted(&self) -> Vec<PaintedGutterItem> {
        self.items
            .iter()
            .map(|laid| PaintedGutterItem {
                lane: self.lanes[laid.lane].id.clone(),
                row: laid.item.row,
                bounds: laid.bounds,
            })
            .collect()
    }

    /// Delivers a mouse down: to the topmost item under the pointer that
    /// handles it, then to the topmost lane under it with a handler for
    /// rows without an item. Returns whether something handled it.
    pub(crate) fn dispatch_mouse_down(
        &self,
        position: Point<Pixels>,
        button: MouseButton,
        click_count: usize,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some((row, top)) = self.row_at(position.y) else { return false };
        let secondary = button == MouseButton::Right || (button == MouseButton::Left && modifiers.control);
        let event = |bounds| GutterEvent { row, position, button, click_count, modifiers, bounds };
        for laid in self.items.iter().rev().filter(|laid| laid.bounds.contains(&position)) {
            let item = &laid.item;
            let handler = if secondary {
                item.on_secondary_click.as_ref()
            } else if button != MouseButton::Left {
                None
            } else if click_count >= 2 {
                item.on_double_click.as_ref().or(item.on_click.as_ref())
            } else {
                item.on_click.as_ref()
            };
            if let Some(handler) = handler
                && handler(&event(laid.bounds), window, cx)
            {
                return true;
            }
        }
        for (ix, lane) in self.lanes.iter().enumerate().rev() {
            let column = &self.lane_columns[ix];
            if position.x < column.start || position.x >= column.end {
                continue;
            }
            let handler = if secondary {
                lane.on_secondary_click.as_ref()
            } else if button == MouseButton::Left {
                lane.on_click.as_ref()
            } else {
                None
            };
            let cell = Bounds::new(point(column.start, top), size(column.end - column.start, self.line_height));
            if let Some(handler) = handler
                && handler(&event(cell), window, cx)
            {
                return true;
            }
        }
        false
    }

    /// The hovered item's lane and row, its hover handler and its cursor.
    pub(crate) fn hover_at(
        &self,
        position: Point<Pixels>,
    ) -> Option<((SharedString, u32), Option<HoverFn>, Option<CursorStyle>)> {
        let laid = self.item_at(position)?;
        let lane = &self.lanes[laid.lane];
        Some(((lane.id.clone(), laid.item.row), laid.item.on_hover.clone(), laid.item.cursor))
    }
}

impl EditorState {
    /// Adds a gutter lane, or replaces the one with the same id.
    pub fn set_gutter_lane(&mut self, lane: GutterLane, cx: &mut gpui::Context<Self>) {
        let lanes = &mut self.gutter.lanes;
        match lanes.iter_mut().find(|l| l.id == lane.id) {
            Some(existing) => *existing = lane,
            None => lanes.push(lane),
        }
        cx.notify();
    }

    pub fn remove_gutter_lane(&mut self, id: &str, cx: &mut gpui::Context<Self>) {
        self.gutter.lanes.retain(|lane| lane.id.as_ref() != id);
        cx.notify();
    }

    /// The gutter items painted last frame, in window coordinates.
    pub fn painted_gutter_items(&self) -> &[PaintedGutterItem] {
        &self.gutter.painted
    }

    /// The gutter's number column and visible rows last frame.
    pub fn gutter_geometry(&self) -> &GutterGeometry {
        &self.gutter.geometry
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{MouseDownEvent, TestAppContext, VisualTestContext, px};

    use super::*;
    use crate::test_support::open_editor;

    type Log = Rc<RefCell<Vec<String>>>;

    fn press(cx: &mut VisualTestContext, position: Point<Pixels>, button: MouseButton, click_count: usize) {
        cx.simulate_event(MouseDownEvent {
            position,
            modifiers: Modifiers::none(),
            button,
            click_count,
            first_mouse: false,
        });
        cx.run_until_parked();
    }

    fn row_center(editor: &gpui::Entity<EditorState>, cx: &mut VisualTestContext, row: u32) -> Point<Pixels> {
        editor.read_with(cx, |state, _| {
            let geometry = state.gutter_geometry();
            let (_, top, _) = geometry.rows.iter().find(|(r, _, _)| *r == row).copied().unwrap();
            point(geometry.numbers.center().x, top + px(4.))
        })
    }

    /// A lane over the numbers with an item on row 1 that logs its clicks,
    /// and a click handler for rows without an item.
    fn marks(log: &Log) -> GutterLane {
        let items_log = log.clone();
        let lane_log = log.clone();
        GutterLane::new("marks", move |rows, _| {
            let log = items_log.clone();
            (rows.rows.start..rows.rows.end)
                .filter(|row| *row == 1)
                .map(|row| {
                    let (click, double, secondary, hover) = (log.clone(), log.clone(), log.clone(), log.clone());
                    GutterItem::new(row, |_, _, _| {})
                        .on_click(move |e, _, _| { click.borrow_mut().push(format!("item click {}", e.row)); true })
                        .on_double_click(move |e, _, _| { double.borrow_mut().push(format!("item double {}", e.row)); true })
                        .on_secondary_click(move |e, _, _| { secondary.borrow_mut().push(format!("item menu {}", e.row)); true })
                        .on_hover(move |hovered, _, _| hover.borrow_mut().push(format!("hover {hovered}")))
                        .cursor(CursorStyle::PointingHand)
                })
                .collect()
        })
        .on_click(move |e, _, _| {
            lane_log.borrow_mut().push(format!("lane click {}", e.row));
            true
        })
    }

    #[gpui::test]
    fn items_paint_on_their_rows_and_take_their_clicks(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "zero\none\ntwo\nthree\n");
        let log: Log = Default::default();
        cx.update(|_, cx| editor.update(cx, |state, cx| state.set_gutter_lane(marks(&log), cx)));
        cx.run_until_parked();
        let painted = editor.read_with(&cx, |state, _| state.painted_gutter_items().to_vec());
        assert_eq!(painted.iter().map(|p| (p.lane.to_string(), p.row)).collect::<Vec<_>>(), [("marks".to_string(), 1)]);

        let one = row_center(&editor, &mut cx, 1);
        let three = row_center(&editor, &mut cx, 3);
        press(&mut cx, one, MouseButton::Left, 1);
        press(&mut cx, one, MouseButton::Left, 2);
        press(&mut cx, one, MouseButton::Right, 1);
        press(&mut cx, three, MouseButton::Left, 1);
        // A secondary click on a row without an item isn't handled.
        press(&mut cx, three, MouseButton::Right, 1);
        cx.simulate_mouse_move(one, None, Modifiers::none());
        cx.run_until_parked();
        cx.simulate_mouse_move(three, None, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            *log.borrow(),
            ["item click 1", "item double 1", "item menu 1", "lane click 3", "hover true", "hover false"]
        );
    }

    #[gpui::test]
    fn later_lanes_sit_on_top_and_unhandled_clicks_fall_through(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "zero\none\ntwo\n");
        let log: Log = Default::default();
        let top_log = log.clone();
        // A narrow item on row 1 that only handles secondary clicks, over
        // the marks lane.
        let top = GutterLane::new("ribbons", move |_, _| {
            let log = top_log.clone();
            vec![GutterItem::new(1, |_, _, _| {})
                .on_secondary_click(move |_, _, _| { log.borrow_mut().push("ribbon menu".into()); true })]
        })
        .order(1);
        cx.update(|_, cx| {
            editor.update(cx, |state, cx| {
                state.set_gutter_lane(top, cx);
                state.set_gutter_lane(marks(&log), cx);
            })
        });
        cx.run_until_parked();
        let painted = editor.read_with(&cx, |state, _| state.painted_gutter_items().iter().map(|p| p.lane.to_string()).collect::<Vec<_>>());
        assert_eq!(painted, ["marks", "ribbons"], "painted in order, the later lane on top");
        let one = row_center(&editor, &mut cx, 1);
        press(&mut cx, one, MouseButton::Right, 1);
        press(&mut cx, one, MouseButton::Left, 1);
        assert_eq!(*log.borrow(), ["ribbon menu", "item click 1"]);
    }

    #[gpui::test]
    fn columns_move_the_numbers_over(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "zero\none\n");
        let before = editor.read_with(&cx, |state, _| state.gutter_geometry().numbers);
        cx.update(|_, cx| {
            editor.update(cx, |state, cx| {
                let lane = GutterLane::new("changes", |_, _| vec![GutterItem::new(0, |_, _, _| {})])
                    .placement(GutterPlacement::BeforeNumbers(px(4.)));
                state.set_gutter_lane(lane, cx);
            })
        });
        cx.run_until_parked();
        let (after, painted) = editor.read_with(&cx, |state, _| (state.gutter_geometry().numbers, state.painted_gutter_items().to_vec()));
        assert_eq!(after.origin.x, before.origin.x + px(4.));
        assert_eq!(painted[0].bounds.origin.x, before.origin.x);
        assert_eq!(painted[0].bounds.size.width, px(4.));
        cx.update(|_, cx| editor.update(cx, |state, cx| state.remove_gutter_lane("changes", cx)));
        cx.run_until_parked();
        assert_eq!(editor.read_with(&cx, |state, _| state.gutter_geometry().numbers.origin.x), before.origin.x);
    }

    #[gpui::test]
    fn items_on_anchored_lines_follow_edits(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "zero\none\ntwo\n");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                let anchor = state.line_anchor(1);
                state.set_gutter_lane(
                    GutterLane::new("anchored", move |rows, _| {
                        rows.anchor_row(anchor).map(|row| GutterItem::new(row, |_, _, _| {})).into_iter().collect()
                    }),
                    cx,
                );
                state.edit([(0..0, "new\n")], window, cx);
            })
        });
        cx.run_until_parked();
        let rows = editor.read_with(&cx, |state, _| state.painted_gutter_items().iter().map(|p| p.row).collect::<Vec<_>>());
        assert_eq!(rows, [2]);
    }
}

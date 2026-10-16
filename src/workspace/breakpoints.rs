//! Breakpoints in the editor and the Breakpoint navigator, as in Xcode: a
//! blue marker over the line number (click the gutter to add one, click a
//! marker to disable or enable it, double-click it to edit it, right-click
//! for its menu), the breakpoint editor popover, and the navigator's list
//! with its + menu (Swift Error, Exception and Symbolic breakpoints).
//!
//! Line breakpoints follow edits through their line anchors
//! (`line_anchors.rs`), so inserting lines above one moves it down.
//! Deleting a breakpoint's line deletes the breakpoint.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{EditorState, RopeExt as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::breakpoint_editor::BreakpointEditor;
use super::navigator::{empty, row, section_header};
use super::pickers::file_icon;
use super::test_gutter::{FOLD_COLUMN, gutter_width};
use super::{NavigatorTab, Target, Workspace};
use crate::debugger::Breakpoint;
use crate::debugger::breakpoints::{ExceptionLanguage, ExceptionPhase, Kind};

/// How far a marker's point reaches into the fold column.
const MARKER_TIP: f32 = 7.;

/// Where markers were painted last frame (window coordinates): for
/// opening the editor next to one, and for the test harness.
#[derive(Default)]
pub(super) struct BreakpointGutter {
    pub painted: RefCell<Vec<(Bounds<Pixels>, u64, u32)>>,
    /// The breakpoint the last click added or toggled, and whether it was
    /// enabled before, so a double-click (edit) can undo that click.
    pressed: Cell<Option<(u64, bool)>>,
}

/// An open popup menu (the breakpoint marker's menu).
pub(super) struct PopupMenuState {
    pub menu: Entity<PopupMenu>,
    pub position: Point<Pixels>,
    _subscription: Subscription,
}

/// Paints a breakpoint marker: a tag pointing at the line, as in Xcode.
fn paint_marker(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla) {
    let tip = px(MARKER_TIP).min(bounds.size.width / 2.);
    let (left, right, top, bottom) = (bounds.left(), bounds.right(), bounds.top(), bounds.bottom());
    let body = right - tip;
    let mut path = PathBuilder::fill();
    path.add_polygon(
        &[point(left, top), point(body, top), point(right, top + bounds.size.height / 2.), point(body, bottom), point(left, bottom)],
        true,
    );
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// Xcode's breakpoint blue; dimmed when the breakpoint (or all of them) is off.
fn marker_color(enabled: bool, active: bool) -> Hsla {
    let blue = hsla(0.585, 0.78, 0.52, 1.);
    match (enabled, active) {
        (true, true) => blue,
        (false, true) => blue.opacity(0.35),
        (true, false) => hsla(0.585, 0.12, 0.55, 0.85),
        (false, false) => hsla(0.585, 0.12, 0.55, 0.35),
    }
}

/// A marker as an inline element (the Breakpoint navigator's toggles).
fn marker_icon(enabled: bool, active: bool) -> impl IntoElement {
    let color = marker_color(enabled, active);
    canvas(|_, _, _| {}, move |bounds, _, window, _| paint_marker(window, bounds, color))
        .w(px(22.))
        .h(px(12.))
        .flex_none()
}

/// The 1-based line under window position `y` in the editor's gutter.
fn line_at(state: &EditorState, y: Pixels) -> Option<u32> {
    let rows = state.visible_row_range()?;
    let text = state.text();
    let mut found = None;
    for row in rows {
        if row >= text.lines_len() {
            break;
        }
        let offset = text.line_start_offset(row);
        let Some(bounds) = state.range_to_bounds(&(offset..offset)) else { continue };
        if bounds.origin.y > y {
            break;
        }
        found = Some(row as u32 + 1);
    }
    found
}

impl Workspace {
    // ------------------------------------------------------------------
    // Anchors

    /// Anchors `path`'s line breakpoints in its open buffer. Skipped when the
    /// anchors already match, unless `force` (the text was replaced).
    pub(super) fn anchor_breakpoints(&mut self, path: &Path, force: bool, cx: &mut Context<Self>) {
        let wanted: Vec<(u64, u32)> = self
            .debugger
            .read(cx)
            .line_breakpoints(path)
            .filter_map(|bp| Some((bp.id, bp.line()?)))
            .collect();
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else { return };
        buffer.breakpoint_anchors.set(&buffer.editor, wanted, force, cx);
    }

    /// After an edit, moves breakpoints to where their anchors went.
    pub(super) fn track_breakpoints(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter().find(|b| b.path == path) else { return };
        let moves = buffer.breakpoint_anchors.moves(&buffer.editor, cx);
        if !moves.is_empty() {
            let path = path.to_path_buf();
            self.debugger.update(cx, |d, cx| d.move_line_breakpoints(&path, moves, cx));
        }
    }

    // ------------------------------------------------------------------
    // Gutter

    /// The breakpoint markers over the active editor's line numbers, and
    /// the clicks on them.
    pub(super) fn render_breakpoint_gutter(&mut self, editor: &Entity<EditorState>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let path = self.active_buffer()?.path.clone();
        let debugger = self.debugger.read(cx);
        let active = debugger.breakpoints_active;
        let markers: Vec<(u64, u32, bool)> = debugger
            .line_breakpoints(&path)
            .filter_map(|bp| Some((bp.id, bp.line()?, bp.enabled)))
            .collect();
        let editor = editor.clone();
        let gutter = self.breakpoint_gutter.clone();
        let workspace = cx.weak_entity();
        let font = font(cx.theme().mono_font_family.clone());
        let font_size = px(self.font_size);
        Some(
            canvas(
                {
                    let editor = editor.clone();
                    move |_, window, cx| {
                        // The line numbers' column, from the editor's last layout.
                        let state = editor.read(cx);
                        let input = state.input_bounds();
                        let numbers = gutter_width(state)? - px(FOLD_COLUMN) + px(MARKER_TIP);
                        Some(window.insert_hitbox(Bounds::new(input.origin, size(numbers, input.size.height)), HitboxBehavior::Normal))
                    }
                },
                move |_, hitbox, window, cx| {
                    let Some(hitbox) = hitbox else { return };
                    let state = editor.read(cx);
                    let (Some(width), Some(line_height)) = (gutter_width(state), state.line_height()) else { return };
                    let input = state.input_bounds();
                    let numbers_right = input.origin.x + width - px(FOLD_COLUMN);
                    let rope = state.text().clone();
                    let mut visible = vec![];
                    for &(id, line, enabled) in &markers {
                        let row = line.saturating_sub(1) as usize;
                        if row >= rope.lines_len() {
                            continue;
                        }
                        let offset = rope.line_start_offset(row);
                        let Some(bounds) = state.range_to_bounds(&(offset..offset)) else { continue };
                        let marker = Bounds::new(
                            point(input.origin.x + px(2.), bounds.origin.y + px(1.)),
                            size(numbers_right + px(MARKER_TIP) - input.origin.x - px(2.), line_height - px(2.)),
                        );
                        if input.intersects(&marker) {
                            visible.push((marker, bounds.origin.y, id, line, enabled));
                        }
                    }
                    let mut painted = vec![];
                    for (marker, top, id, line, enabled) in visible {
                        paint_marker(window, marker, marker_color(enabled, active));
                        // The line number, white on the marker.
                        let number: SharedString = line.to_string().into();
                        let run = TextRun {
                            len: number.len(),
                            font: font.clone(),
                            color: white(),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let shaped = window.text_system().shape_line(number, font_size, &[run], None);
                        let origin = point(numbers_right - shaped.width, top);
                        shaped.paint(origin, line_height, TextAlign::Left, None, window, cx).ok();
                        painted.push((marker, id, line));
                    }
                    *gutter.painted.borrow_mut() = painted;

                    let editor = editor.clone();
                    let workspace = workspace.clone();
                    let path = path.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                            return;
                        }
                        let Some(line) = line_at(editor.read(cx), event.position.y) else { return };
                        let (button, clicks, position) = (event.button, event.click_count, event.position);
                        let path = path.clone();
                        let handled = workspace
                            .update(cx, |this, cx| match button {
                                MouseButton::Left => {
                                    this.gutter_click(path, line, clicks, position, window, cx);
                                    true
                                }
                                MouseButton::Right => this.breakpoint_menu_at(&path, line, position, window, cx),
                                _ => false,
                            })
                            .unwrap_or(false);
                        if handled {
                            cx.stop_propagation();
                        }
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

    /// A click on the line numbers: adds a breakpoint, or disables or
    /// enables the one there; a double-click edits it.
    pub(super) fn gutter_click(
        &mut self,
        path: PathBuf,
        line: u32,
        clicks: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self.debugger.read(cx).breakpoint_at(&path, line).map(|bp| (bp.id, bp.enabled));
        if clicks >= 2 {
            if let Some((id, _)) = existing {
                if let Some((pressed, before)) = self.breakpoint_gutter.pressed.take()
                    && pressed == id
                {
                    self.debugger.update(cx, |d, cx| d.set_breakpoint_enabled(id, before, cx));
                }
                // Below the marker, as Xcode's popover hangs from it.
                let marker = self.marker_anchor(id);
                self.edit_breakpoint(id, marker.or(Some(position)), window, cx);
            }
            return;
        }
        let pressed = match existing {
            Some((id, enabled)) => {
                self.debugger.update(cx, |d, cx| d.set_breakpoint_enabled(id, !enabled, cx));
                (id, enabled)
            }
            None => (self.debugger.update(cx, |d, cx| d.add_breakpoint(Breakpoint::new(Kind::Line { path, line }), cx)), true),
        };
        self.breakpoint_gutter.pressed.set(Some(pressed));
    }

    /// The marker's menu, when there is a breakpoint on `line`.
    fn breakpoint_menu_at(&mut self, path: &Path, line: u32, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(bp) = self.debugger.read(cx).breakpoint_at(path, line).cloned() else { return false };
        let workspace = cx.weak_entity();
        self.show_popup_menu(position, window, cx, move |menu, _, _| breakpoint_menu_items(menu, &bp, workspace, position, true));
        true
    }

    /// Shows a popup menu at a window position.
    pub(super) fn show_popup_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
        build: impl FnOnce(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu,
    ) {
        let menu = PopupMenu::build(window, cx, build);
        menu.focus_handle(cx).focus(window, cx);
        let subscription = cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, _, cx| {
            this.popup_menu = None;
            cx.notify();
        });
        self.popup_menu = Some(PopupMenuState { menu, position, _subscription: subscription });
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Editing

    /// Opens the breakpoint editor popover, next to `position` if given.
    pub(super) fn edit_breakpoint(&mut self, id: u64, position: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bp) = self.debugger.read(cx).breakpoint(id).cloned() else { return };
        // Below the marker when it's on screen.
        let position = position.or_else(|| self.marker_anchor(id));
        let workspace = cx.weak_entity();
        let editor = cx.new(|cx| BreakpointEditor::new(workspace, bp, window, cx));
        self.breakpoint_editor = Some(editor.downgrade());
        self.show_modal_at(editor, position, window, cx);
    }

    /// Where a popover for breakpoint `id` hangs from its marker.
    fn marker_anchor(&self, id: u64) -> Option<Point<Pixels>> {
        self.breakpoint_gutter
            .painted
            .borrow()
            .iter()
            .find(|(_, painted, _)| *painted == id)
            .map(|(bounds, _, _)| point(bounds.left(), bounds.bottom()))
    }

    /// Saves what the breakpoint editor changed (it closes however it closes).
    pub(super) fn apply_breakpoint_edit(&mut self, editor: &Entity<BreakpointEditor>, cx: &mut Context<Self>) {
        let edited = editor.read(cx).edited(cx);
        let id = edited.id;
        // A new symbolic breakpoint left without a symbol isn't kept.
        if matches!(&edited.kind, Kind::Symbolic { symbol, .. } if symbol.is_empty()) {
            self.debugger.update(cx, |d, cx| d.remove_breakpoint(id, cx));
        } else {
            self.debugger.update(cx, |d, cx| d.update_breakpoint(id, |bp| *bp = edited, cx));
        }
        self.save_session(cx);
    }

    /// Edit Breakpoint… for the breakpoint on the cursor's line.
    pub(super) fn edit_breakpoint_at_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.active_buffer() else { return };
        let line = buffer.editor.read(cx).cursor_position().line + 1;
        let id = self.debugger.read(cx).breakpoint_at(&buffer.path, line).map(|bp| bp.id);
        match id {
            Some(id) => self.edit_breakpoint(id, None, window, cx),
            None => self.notify_error(format!("There is no breakpoint on line {line}."), window, cx),
        }
    }

    /// Debug ▸ Breakpoints ▸ Create Symbolic Breakpoint… (⌥⌘\).
    pub(super) fn create_symbolic_breakpoint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bp = Breakpoint::new(Kind::Symbolic { symbol: String::new(), module: String::new() });
        self.create_breakpoint(bp, true, window, cx);
    }

    /// Debug ▸ Breakpoints ▸ Create Swift Error Breakpoint.
    pub(super) fn create_swift_error_breakpoint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create_breakpoint(Breakpoint::new(Kind::SwiftError), false, window, cx);
    }

    /// Debug ▸ Breakpoints ▸ Create Exception Breakpoint….
    pub(super) fn create_exception_breakpoint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bp = Breakpoint::new(Kind::Exception { language: ExceptionLanguage::All, on: ExceptionPhase::Throw });
        self.create_breakpoint(bp, true, window, cx);
    }

    /// Adds a breakpoint from the + menu and shows it in the navigator,
    /// opening its editor when `edit` (as Xcode does for symbolic and
    /// exception breakpoints).
    fn create_breakpoint(&mut self, bp: Breakpoint, edit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.debugger.update(cx, |d, cx| d.add_breakpoint(bp, cx));
        self.show_navigator_tab(NavigatorTab::Breakpoints, window, cx);
        if edit {
            self.edit_breakpoint(id, None, window, cx);
        }
    }

    // ------------------------------------------------------------------
    // Navigator

    pub(super) fn render_breakpoint_navigator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let debugger = self.debugger.read(cx);
        let active = debugger.breakpoints_active;
        let breakpoints = debugger.breakpoints.clone();
        let mut list = v_flex().id("breakpoints").flex_1().min_h_0().overflow_y_scroll();
        if breakpoints.is_empty() {
            list = list.child(empty("No breakpoints. Use ⌘\\ on a line to add one, or + below.", cx));
        }
        // Breakpoints that aren't in a file first, then files with their lines.
        for bp in breakpoints.iter().filter(|bp| bp.path().is_none()) {
            list = list.child(self.breakpoint_row(bp, active, false, cx));
        }
        let mut files: BTreeMap<PathBuf, Vec<&Breakpoint>> = BTreeMap::new();
        for bp in &breakpoints {
            if let Some(path) = bp.path() {
                files.entry(path.to_path_buf()).or_default().push(bp);
            }
        }
        for (file_ix, (path, mut lines)) in files.into_iter().enumerate() {
            lines.sort_by_key(|bp| bp.line());
            list = list.child(
                row(("bp-file", file_ix))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Icon::new(file_icon(&path)).small().text_color(theme.muted_foreground))
                    .child(path.file_name().unwrap_or_default().to_string_lossy().to_string()),
            );
            for bp in lines {
                list = list.child(self.breakpoint_row(bp, active, true, cx));
            }
        }
        let workspace = cx.weak_entity();
        let add = Button::new("bp-add")
            .icon(IconName::Plus)
            .ghost()
            .xsmall()
            .tooltip("Add a breakpoint")
            .dropdown_menu(move |menu, _, _| {
                let item = |label: &'static str, create: fn(&mut Workspace, &mut Window, &mut Context<Workspace>)| {
                    let workspace = workspace.clone();
                    PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        workspace.update(cx, |ws, cx| create(ws, window, cx)).ok();
                    })
                };
                menu.item(item("Swift Error Breakpoint", Workspace::create_swift_error_breakpoint))
                    .item(item("Exception Breakpoint…", Workspace::create_exception_breakpoint))
                    .item(item("Symbolic Breakpoint…", Workspace::create_symbolic_breakpoint))
            });
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .justify_between()
                    .child(section_header(if active { "Breakpoints" } else { "Breakpoints (deactivated)" }, cx))
                    .child(
                        Button::new("clear-breakpoints")
                            .icon(IconName::Trash)
                            .ghost()
                            .xsmall()
                            .tooltip("Delete All Breakpoints")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.debugger.update(cx, |d, cx| d.remove_all_breakpoints(cx));
                            })),
                    ),
            )
            .child(list)
            .child(h_flex().px_1().py_0p5().border_t_1().border_color(theme.border).child(add))
    }

    /// A breakpoint in the navigator: its marker toggles it, a click shows
    /// its line, a double-click edits it.
    fn breakpoint_row(&self, bp: &Breakpoint, active: bool, in_file: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (id, enabled) = (bp.id, bp.enabled);
        let label = match (&bp.kind, bp.name.is_empty()) {
            (Kind::Line { line, .. }, true) => format!("line {line}"),
            (_, true) => bp.title(),
            (_, false) => bp.name.clone(),
        };
        let mut details = vec![];
        if let Kind::Symbolic { module, .. } = &bp.kind
            && !module.is_empty()
        {
            details.push(module.clone());
        }
        if !bp.condition.is_empty() {
            details.push(format!("if {}", bp.condition));
        }
        if bp.ignore_count > 0 {
            details.push(format!("ignore {}", bp.ignore_count));
        }
        if !bp.actions.is_empty() {
            details.push(match bp.actions.len() {
                1 => "1 action".to_string(),
                n => format!("{n} actions"),
            });
        }
        let target = bp.path().map(Path::to_path_buf).zip(bp.line());
        let menu_bp = bp.clone();
        let workspace = cx.weak_entity();
        row(("bp", id as usize))
            .when(in_file, |el| el.pl_6())
            .text_xs()
            .hover(|el| el.bg(theme.sidebar_accent))
            .child(
                div()
                    .id(("bp-toggle", id as usize))
                    .child(marker_icon(enabled, active))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.debugger.update(cx, |d, cx| d.set_breakpoint_enabled(id, !enabled, cx));
                    })),
            )
            .child(div().truncate().when(!enabled, |el| el.text_color(theme.muted_foreground)).child(label))
            .child(div().flex_1().truncate().text_color(theme.muted_foreground).child(details.join(" · ")))
            .child(
                Button::new(("bp-remove", id as usize))
                    .icon(IconName::X)
                    .ghost()
                    .xsmall()
                    .tooltip("Delete Breakpoint")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.debugger.update(cx, |d, cx| d.remove_breakpoint(id, cx));
                    })),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    this.edit_breakpoint(id, Some(event.position()), window, cx);
                } else if let Some((path, line)) = target.clone() {
                    let target = Target::Line { line: line.saturating_sub(1), column: 0 };
                    this.open_file(path, Some(target), true, window, cx);
                }
            }))
            .context_menu(move |menu, _, _| breakpoint_menu_items(menu, &menu_bp, workspace.clone(), Point::default(), false))
    }

    /// The breakpoints for the test harness's `dump`.
    pub(super) fn breakpoints_summary(&self, cx: &App) -> serde_json::Value {
        let debugger = self.debugger.read(cx);
        let list: Vec<serde_json::Value> = debugger
            .breakpoints
            .iter()
            .map(|bp| {
                serde_json::json!({
                    "title": bp.title(),
                    "name": bp.name,
                    "line": bp.line(),
                    "enabled": bp.enabled,
                    "condition": bp.condition,
                    "ignore": bp.ignore_count,
                    "actions": bp.actions,
                    "auto_continue": bp.auto_continue,
                })
            })
            .collect();
        // Where to click in the gutter: the line numbers' column, and the
        // markers painted last frame.
        let gutter = self.active_buffer().and_then(|buffer| {
            let state = buffer.editor.read(cx);
            let input = state.input_bounds();
            let x = input.origin.x + (gutter_width(state)? - px(FOLD_COLUMN)) / 2.;
            let line_height = state.line_height()?;
            let text = state.text();
            let rows: Vec<serde_json::Value> = state
                .visible_row_range()?
                .filter(|&row| row < text.lines_len())
                .filter_map(|row| {
                    let offset = text.line_start_offset(row);
                    let bounds = state.range_to_bounds(&(offset..offset))?;
                    Some(serde_json::json!([row + 1, f32::from(bounds.origin.y + line_height / 2.)]))
                })
                .collect();
            Some(serde_json::json!({ "x": f32::from(x), "rows": rows }))
        });
        let markers: Vec<serde_json::Value> = self
            .breakpoint_gutter
            .painted
            .borrow()
            .iter()
            .map(|(bounds, _, line)| serde_json::json!({ "line": line, "x": f32::from(bounds.center().x), "y": f32::from(bounds.center().y) }))
            .collect();
        serde_json::json!({
            "list": list,
            "active": debugger.breakpoints_active,
            "gutter": gutter,
            "markers": markers,
            "editor": self.breakpoint_editor.as_ref().and_then(|e| e.upgrade()).map(|e| e.read(cx).summary(cx)),
            "menu_open": self.popup_menu.is_some(),
        })
    }
}

/// Edit Breakpoint…, Disable/Enable, Delete and (from the editor) Reveal in
/// Breakpoint Navigator: a breakpoint's menu.
fn breakpoint_menu_items(menu: PopupMenu, bp: &Breakpoint, workspace: WeakEntity<Workspace>, position: Point<Pixels>, reveal: bool) -> PopupMenu {
    let (id, enabled) = (bp.id, bp.enabled);
    let edit = {
        let workspace = workspace.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            let position = (position != Point::default()).then_some(position);
            workspace.update(cx, |ws, cx| ws.edit_breakpoint(id, position, window, cx)).ok();
        }
    };
    let toggle = {
        let workspace = workspace.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            workspace
                .update(cx, |ws, cx| ws.debugger.update(cx, |d, cx| d.set_breakpoint_enabled(id, !enabled, cx)))
                .ok();
        }
    };
    let delete = {
        let workspace = workspace.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            workspace.update(cx, |ws, cx| ws.debugger.update(cx, |d, cx| d.remove_breakpoint(id, cx))).ok();
        }
    };
    let menu = menu
        .item(PopupMenuItem::new("Edit Breakpoint…").on_click(edit))
        .item(PopupMenuItem::new(if enabled { "Disable Breakpoint" } else { "Enable Breakpoint" }).on_click(toggle))
        .item(PopupMenuItem::new("Delete Breakpoint").on_click(delete));
    if !reveal {
        return menu;
    }
    menu.separator().item(PopupMenuItem::new("Reveal in Breakpoint Navigator").on_click(move |_, window, cx| {
        workspace.update(cx, |ws, cx| ws.show_navigator_tab(NavigatorTab::Breakpoints, window, cx)).ok();
    }))
}

impl Workspace {
    /// Edits the open breakpoint editor (the test harness's `breakpoint`).
    /// `edit <n>` opens the editor for the nth breakpoint (in the order they
    /// were added) and `enable <n> on|off` toggles it as its marker does.
    pub fn breakpoint_editor_field(&mut self, field: &str, value: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        if field == "edit" || field == "enable" {
            let (index, on) = value.split_once(' ').unwrap_or((value, ""));
            let index: usize = index.parse().map_err(|_| format!("usage: breakpoint {field} <index>"))?;
            let id = self.debugger.read(cx).breakpoints.get(index).map(|bp| bp.id).ok_or(format!("no breakpoint {index}"))?;
            match field {
                "edit" => self.edit_breakpoint(id, None, window, cx),
                _ => self.debugger.update(cx, |d, cx| d.set_breakpoint_enabled(id, on == "on", cx)),
            }
            return Ok(());
        }
        let editor = self.breakpoint_editor.as_ref().and_then(|e| e.upgrade()).ok_or("no breakpoint editor is open")?;
        if field == "close" {
            self.dismiss_modal(window, cx);
            return Ok(());
        }
        editor.update(cx, |editor, cx| editor.set_field(field, value, window, cx))
    }

    /// The active editor's hover at its cursor (the test harness's `hover`).
    pub fn hover_at_cursor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Task<anyhow::Result<Option<lsp_types::Hover>>>> {
        let editor = self.active_editor()?;
        editor.update(cx, |state, cx| {
            let offset = state.cursor();
            let text = state.text().clone();
            let provider = state.lsp_mut().hover_provider.clone()?;
            Some(provider.hover(&text, offset, window, cx))
        })
    }
}

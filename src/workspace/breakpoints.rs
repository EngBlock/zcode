//! Breakpoints in the editor and the Breakpoint navigator, as in Xcode: a
//! blue marker over the line number (click the gutter to add one, click a
//! marker to disable or enable it, double-click it to edit it, right-click
//! for its menu), the breakpoint editor popover, and the navigator's list
//! with its + menu (Swift Error, Exception and Symbolic breakpoints).
//!
//! Line breakpoints follow edits through their line anchors
//! (`line_anchors.rs`), so inserting lines above one moves it down.
//! Deleting a breakpoint's line deletes the breakpoint.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use zcode_editor::{EditorState, GutterItem, GutterLane};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::breakpoint_editor::BreakpointEditor;
use super::line_anchors::LineChange;
use super::navigator::{empty, row, section_header};
use super::pickers::file_icon;
use super::{NavigatorTab, Target, Workspace};
use crate::debugger::Breakpoint;
use crate::debugger::breakpoints::{ExceptionLanguage, ExceptionPhase, Kind};

/// How far a marker's point reaches into the fold column.
const MARKER_TIP: f32 = 7.;

/// The gutter lane of breakpoint markers.
pub(super) const BREAKPOINT_LANE: &str = "breakpoints";

/// Gutter clicks on breakpoints.
#[derive(Default)]
pub(super) struct BreakpointGutter {
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

    /// After an edit, moves breakpoints to where their anchors went. A
    /// breakpoint deleted with its line comes back with the undo that
    /// restores the line.
    pub(super) fn track_breakpoints(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else { return };
        let changes = buffer.breakpoint_anchors.changes(&buffer.editor, cx);
        if changes.is_empty() {
            return;
        }
        let mut moves = vec![];
        let mut restored = vec![];
        for change in changes {
            match change {
                LineChange::Moved(id, line) => moves.push((id, Some(line))),
                LineChange::Deleted(id) => {
                    if let Some(bp) = self.debugger.read(cx).breakpoint(id).cloned() {
                        buffer.breakpoint_anchors.keep_deleted(id, bp);
                    }
                    moves.push((id, None));
                }
                LineChange::Restored(line, bp) => restored.push((bp, line)),
            }
        }
        let path = path.to_path_buf();
        self.debugger.update(cx, |d, cx| {
            if !moves.is_empty() {
                d.move_line_breakpoints(&path, moves, cx);
            }
            for (bp, line) in restored {
                d.restore_line_breakpoint(bp, line, cx);
            }
        });
    }

    // ------------------------------------------------------------------
    // Gutter

    /// The breakpoint markers over `editor`'s line numbers, and the clicks
    /// on them: a click on the numbers adds a breakpoint, a click on a
    /// marker disables or enables it, a double-click edits it, a right-click
    /// shows its menu.
    pub(super) fn breakpoint_lane(&self, editor: &Entity<EditorState>, cx: &Context<Self>) -> GutterLane {
        let workspace = cx.weak_entity();
        let editor_id = editor.entity_id();
        let lane_workspace = workspace.clone();
        GutterLane::new(BREAKPOINT_LANE, move |rows, cx| {
            let Some(this) = workspace.upgrade() else { return vec![] };
            let this = this.read(cx);
            let Some(path) = this.buffer_path_of(editor_id) else { return vec![] };
            let debugger = this.debugger.read(cx);
            let active = debugger.breakpoints_active;
            debugger
                .line_breakpoints(&path)
                .filter_map(|bp| Some((bp.line()?, bp.enabled)))
                .filter(|&(line, _)| rows.rows.contains(&line.saturating_sub(1)))
                .map(|(line, enabled)| {
                    let workspace = workspace.clone();
                    let click = workspace.clone();
                    let double = workspace.clone();
                    let path = path.clone();
                    let (click_path, double_path, menu_path) = (path.clone(), path.clone(), path);
                    GutterItem::new(line - 1, move |paint, window, cx| {
                        paint_marker(window, paint.bounds, marker_color(enabled, active));
                        // The line number, white on the marker.
                        paint.paint_line_number(white(), window, cx);
                    })
                    // From the gutter's edge to a point in the fold column.
                    .bounds(|cell, line_height| {
                        Bounds::new(
                            point(cell.left() + px(2.), cell.top() + px(1.)),
                            size(cell.size.width + px(MARKER_TIP) - px(2.), line_height - px(2.)),
                        )
                    })
                    .on_click(move |event, window, cx| {
                        let position = event.position;
                        click.update(cx, |this, cx| this.gutter_click(click_path.clone(), line, 1, position, window, cx)).is_ok()
                    })
                    .on_double_click(move |event, window, cx| {
                        let position = event.position;
                        double.update(cx, |this, cx| this.gutter_click(double_path.clone(), line, 2, position, window, cx)).is_ok()
                    })
                    .on_secondary_click(move |event, window, cx| {
                        let position = event.position;
                        workspace
                            .update(cx, |this, cx| this.breakpoint_menu_at(&menu_path, line, position, window, cx))
                            .unwrap_or(false)
                    })
                })
                .collect()
        })
        .order(0)
        .on_click(move |event, window, cx| {
            let (row, position, clicks) = (event.row, event.position, event.click_count);
            lane_workspace
                .update(cx, |this, cx| {
                    let Some(path) = this.buffer_path_of(editor_id) else { return false };
                    this.gutter_click(path, row + 1, clicks, position, window, cx);
                    true
                })
                .unwrap_or(false)
        })
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
                let marker = self.marker_anchor(id, cx);
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
        let subscription = cx.subscribe_in(&menu, window, |this, menu, _: &DismissEvent, window, cx| {
            this.popup_menu = None;
            // Give focus back, or shortcuts would go to the closed menu
            // (unless its item focused something, such as a popover).
            let focused = menu.focus_handle(cx);
            if focused.contains_focused(window, cx) || window.focused(cx).is_none() {
                match this.active_editor() {
                    Some(editor) => editor.update(cx, |state, cx| state.focus(window, cx)),
                    None => this.focus_handle.focus(window, cx),
                }
            }
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
        let position = position.or_else(|| self.marker_anchor(id, cx));
        let workspace = cx.weak_entity();
        let editor = cx.new(|cx| BreakpointEditor::new(workspace, bp, window, cx));
        self.breakpoint_editor = Some(editor.downgrade());
        self.show_modal_at(editor, position, window, cx);
    }

    /// Where a popover for breakpoint `id` hangs from its marker.
    fn marker_anchor(&self, id: u64, cx: &App) -> Option<Point<Pixels>> {
        let line = self.debugger.read(cx).breakpoint(id)?.line()?;
        self.painted_gutter(BREAKPOINT_LANE, cx)
            .into_iter()
            .find(|(painted, _)| *painted == line)
            .map(|(_, bounds)| point(bounds.left(), bounds.bottom()))
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
            let geometry = state.gutter_geometry();
            let line_height = state.line_height()?;
            let rows: Vec<serde_json::Value> = geometry
                .rows
                .iter()
                .map(|&(row, top, _)| serde_json::json!([row + 1, f32::from(top + line_height / 2.)]))
                .collect();
            Some(serde_json::json!({ "x": f32::from(geometry.numbers.center().x), "rows": rows }))
        });
        let markers: Vec<serde_json::Value> = self
            .painted_gutter(BREAKPOINT_LANE, cx)
            .into_iter()
            .map(|(line, bounds)| serde_json::json!({ "line": line, "x": f32::from(bounds.center().x), "y": f32::from(bounds.center().y) }))
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

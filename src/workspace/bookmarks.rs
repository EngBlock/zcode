//! Bookmarks in the editor and the Bookmarks navigator (⌘3), as in Xcode 27:
//! Edit ▸ Bookmark Line (⌃⌘') and Bookmark File (⌃⇧⌘'), their "With
//! Dialog" variants (⌥), Navigate ▸ Jump to Next / Previous Bookmark (⌃; ⌃:)
//! and their In Workspace variants (⌃⌥; ⌃⌥:), a ribbon at the left of the
//! line numbers, and the navigator's list grouped by file with editable
//! descriptions, completion marks, drag and drop, Sort By and a filter.
//!
//! From the Find navigator a result (its context menu), a file, or the
//! whole query (Bookmark Line while the Find navigator has focus, or its
//! bookmark button) can be bookmarked; a bookmarked query keeps its results
//! as a group that Find Again refreshes.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use zcode_editor::{EditorState, GutterItem, GutterLane, RopeExt as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::bookmark_dialog::BookmarkDialog;
use super::line_anchors::LineChange;
use super::navigator::{empty, row, section_header};
use super::pickers::file_icon;
use super::{NavigatorTab, Target, Workspace};
use crate::actions::PickerDismiss;
use crate::bookmarks::{Bookmark, Location, SectionKey, SortBy, file_name, matches_filter};

/// A ribbon's width, at the left edge of the gutter.
const RIBBON_WIDTH: f32 = 7.;

/// Rows and ribbons as painted: their bounds and what they show.
type PaintedRows = Rc<RefCell<Vec<(Bounds<Pixels>, RowId, String)>>>;
/// The gutter lane of bookmark ribbons.
pub(super) const BOOKMARK_LANE: &str = "bookmarks";
/// What a menu item does to the workspace.
type MenuAction = Box<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

/// A row of the navigator, for drag and drop and the test harness.
#[derive(Clone, Debug, PartialEq)]
enum RowId {
    Section(SectionKey),
    Bookmark(u64),
}

/// The navigator's state that isn't saved.
#[derive(Default)]
pub(super) struct BookmarksUi {
    /// Created the first time the navigator is shown, not at startup.
    filter: Option<Entity<InputState>>,
    incomplete_only: bool,
    editing: Option<TitleEdit>,
    collapsed: HashSet<SectionKey>,
    /// The row last revealed or clicked.
    selected: Option<RowId>,
    /// The bookmark last opened and the file and line it opened at, so
    /// In Workspace steps go on from it while the cursor stays there.
    last_opened: Option<(u64, PathBuf, u32)>,
    /// Where rows were painted last frame (window coordinates), for the
    /// test harness and drag and drop.
    rows: PaintedRows,
    pub dialog: Option<WeakEntity<BookmarkDialog>>,
    _subscriptions: Vec<Subscription>,
}

/// A description being edited in place (Edit Description, or a double-click).
struct TitleEdit {
    /// A bookmark, or a search group.
    id: u64,
    input: Entity<InputState>,
    _subscription: Subscription,
}

#[derive(Clone)]
struct DraggedBookmark {
    id: u64,
    label: SharedString,
}

#[derive(Clone)]
struct DraggedSection {
    key: SectionKey,
    label: SharedString,
}

fn drag_preview(label: SharedString, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .px_2()
        .py_0p5()
        .text_sm()
        .rounded(theme.radius)
        .bg(theme.popover)
        .border_1()
        .border_color(theme.border)
        .child(label)
}

impl Render for DraggedBookmark {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_preview(self.label.clone(), cx)
    }
}

impl Render for DraggedSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drag_preview(self.label.clone(), cx)
    }
}

/// Xcode's bookmark color; dimmed once marked complete.
fn ribbon_color(completed: bool) -> Hsla {
    let color = hsla(0.07, 0.85, 0.55, 1.);
    if completed { color.opacity(0.4) } else { color }
}

/// Paints a bookmark ribbon: a strip with a notch cut into its bottom.
fn paint_ribbon(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla, outline: Hsla) {
    let (left, right, top, bottom) = (bounds.left(), bounds.right(), bounds.top(), bounds.bottom());
    let notch = point(bounds.center().x, bottom - bounds.size.width / 2.);
    let corners = [point(left, top), point(right, top), point(right, bottom), notch, point(left, bottom)];
    // An outline in the gutter's color keeps it visible on a breakpoint.
    let mut edge = PathBuilder::stroke(px(1.5));
    edge.add_polygon(&corners, true);
    if let Ok(path) = edge.build() {
        window.paint_path(path, outline);
    }
    let mut body = PathBuilder::fill();
    body.add_polygon(&corners, true);
    if let Ok(path) = body.build() {
        window.paint_path(path, color);
    }
}

/// A line's text, trimmed and shortened, for a new bookmark's description.
fn describe(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(120) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

impl Workspace {
    // ------------------------------------------------------------------
    // Anchors

    /// Anchors `path`'s line bookmarks in its open buffer, as breakpoints are.
    pub(super) fn anchor_bookmarks(&mut self, path: &Path, force: bool, cx: &mut Context<Self>) {
        let wanted: Vec<(u64, u32)> = self.bookmarks.lines_in(path).collect();
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else { return };
        buffer.bookmark_anchors.set(&buffer.editor, wanted, force, cx);
    }

    /// After an edit, moves bookmarks to where their anchors went. A
    /// bookmark deleted with its line comes back, in its place in the list,
    /// with the undo that restores the line.
    pub(super) fn track_bookmarks(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else { return };
        let changes = buffer.bookmark_anchors.changes(&buffer.editor, cx);
        if changes.is_empty() {
            return;
        }
        let mut moves = vec![];
        let mut restored = vec![];
        for change in changes {
            match change {
                LineChange::Moved(id, line) => moves.push((id, Some(line))),
                LineChange::Deleted(id) => {
                    if let (Some(bookmark), Some(slot)) = (self.bookmarks.get(id).cloned(), self.bookmarks.slot_of(id)) {
                        buffer.bookmark_anchors.keep_deleted(id, (bookmark, slot));
                    }
                    moves.push((id, None));
                }
                LineChange::Restored(line, kept) => restored.push((kept, line)),
            }
        }
        self.bookmarks.move_lines(path, &moves);
        for ((mut bookmark, slot), line) in restored {
            if let Location::Line { line: current, .. } = &mut bookmark.location {
                *current = line;
            }
            self.bookmarks.put_back(bookmark, slot);
        }
        self.bookmarks_changed(false, cx);
    }

    /// Re-anchors open buffers after bookmarks changed, and saves them
    /// (line moves are saved with the session when the window closes).
    pub(super) fn bookmarks_changed(&mut self, save: bool, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.buffers.iter().map(|b| b.path.clone()).collect();
        for path in paths {
            self.anchor_bookmarks(&path, false, cx);
        }
        if save {
            self.save_session(cx);
        }
        cx.notify();
    }

    /// Drops saved bookmarks whose files are gone, checked off the main
    /// thread after the window opens.
    pub(super) fn prune_missing_bookmarks(&mut self, cx: &mut Context<Self>) {
        let mut paths: Vec<PathBuf> = self.bookmarks.all().map(|b| b.location.path().to_path_buf()).collect();
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let missing: Vec<PathBuf> = cx.background_spawn(async move { paths.into_iter().filter(|p| !p.exists()).collect() }).await;
            if missing.is_empty() {
                return;
            }
            this.update(cx, |this, cx| {
                for path in &missing {
                    this.bookmarks.remove_in(path);
                }
                this.bookmarks_changed(false, cx);
            })
            .ok();
        });
        self._tasks.push(task);
    }

    // ------------------------------------------------------------------
    // Commands

    /// Edit ▸ Bookmark Line (⌃⌘'), or With Dialog (⌃⌥⌘'). With the Find
    /// navigator focused it bookmarks the search, as Xcode does with the
    /// query selected there.
    pub(super) fn bookmark_line(&mut self, with_dialog: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_navigator && self.navigator == NavigatorTab::Find && self.find.input.read(cx).focus_handle(cx).is_focused(window) {
            self.bookmark_find_results(window, cx);
            return;
        }
        let Some(buffer) = self.active_buffer().filter(|b| b.shown_plist().is_none()) else {
            // A property list has no lines to bookmark; its file can be.
            if self.active_buffer().is_some() {
                self.bookmark_file(with_dialog, window, cx);
            }
            return;
        };
        let state = buffer.editor.read(cx);
        let line = state.cursor_position().line;
        let selected = state.selected_text().to_string();
        let title = if selected.trim().is_empty() || selected.contains('\n') {
            describe(&state.text().slice_line(line as usize).to_string())
        } else {
            describe(&selected)
        };
        let location = Location::Line { path: buffer.path.clone(), line: line + 1 };
        self.new_bookmark(location, title, with_dialog, window, cx);
    }

    /// Edit ▸ Bookmark File (⌃⇧⌘'), or With Dialog (⌃⌥⇧⌘').
    pub(super) fn bookmark_file(&mut self, with_dialog: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active_buffer().map(|b| b.path.clone()) else { return };
        let title = file_name(&path);
        self.new_bookmark(Location::File { path }, title, with_dialog, window, cx);
    }

    fn new_bookmark(&mut self, location: Location, title: String, with_dialog: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !with_dialog {
            self.add_bookmark(location, title, window, cx);
            return;
        }
        let workspace = cx.weak_entity();
        let dialog = cx.new(|cx| BookmarkDialog::new(workspace, location, title, window, cx));
        self.bookmarks_ui.dialog = Some(dialog.downgrade());
        self.show_modal(dialog, window, cx);
    }

    /// Adds a bookmark; one that's already there is revealed instead.
    pub fn add_bookmark(&mut self, location: Location, title: String, window: &mut Window, cx: &mut Context<Self>) {
        let (id, added) = self.bookmarks.add(location, describe(&title));
        if added {
            self.bookmarks_changed(true, cx);
        } else {
            self.reveal_bookmark(id, window, cx);
        }
    }

    /// Bookmark File from the project navigator's context menu.
    pub(super) fn bookmark_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let title = file_name(&path);
        self.add_bookmark(Location::File { path }, title, window, cx);
    }

    /// A Find navigator result's Bookmark Line (`line` is 0-based).
    pub(super) fn bookmark_find_result(&mut self, path: PathBuf, line: u32, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.add_bookmark(Location::Line { path, line: line + 1 }, text, window, cx);
    }

    /// Bookmarks the Find navigator's query with all its results.
    pub(super) fn bookmark_find_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.find.input.read(cx).value().to_string();
        if query.is_empty() || self.find.searching || self.find.results.is_empty() {
            return;
        }
        let results = find_results(&self.find.results);
        let count = results.len();
        let id = self.bookmarks.add_search(query.clone(), self.find.options.clone(), results);
        self.bookmarks_ui.selected = Some(RowId::Section(SectionKey::Search(id)));
        self.bookmarks_changed(true, cx);
        let lines = if count == 1 { "1 line".to_string() } else { format!("{count} lines") };
        window.push_notification(Notification::info(format!("Bookmarked “{query}” ({lines}) in the Bookmarks navigator (⌘3).")), cx);
    }

    /// Find Again on a bookmarked search: runs its query again and keeps
    /// what is still found.
    pub(super) fn find_again(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(group) = self.bookmarks.search(id) else { return };
        let Ok(regex) = crate::search::build_regex(&group.query, &group.options) else { return };
        let (root, files) = (self.project.root.clone(), self.files.clone());
        let task = cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { crate::search::search(&root, &files, &regex, &Arc::new(AtomicBool::new(false))) })
                .await;
            this.update(cx, |this, cx| {
                this.bookmarks.replace_search_results(id, find_results(&found));
                this.bookmarks_changed(true, cx);
            })
            .ok();
        });
        self._tasks.push(task);
    }

    /// Shows a bookmarked search's query in the Find navigator.
    fn show_search_in_find(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(group) = self.bookmarks.search(id).cloned() else { return };
        self.find.options = group.options;
        self.find.input.update(cx, |input, cx| input.set_value(group.query, window, cx));
        self.show_navigator_tab(NavigatorTab::Find, window, cx);
        self.run_find(window, cx);
    }

    /// Jump to Next / Previous Bookmark (in this file, or `in_workspace`).
    pub(super) fn step_bookmark(&mut self, forward: bool, in_workspace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.active_buffer() else {
            if in_workspace && let Some(first) = self.bookmarks.in_order().first().map(|b| b.id) {
                self.open_bookmark(first, window, cx);
            }
            return;
        };
        let path = buffer.path.clone();
        let line = buffer.editor.read(cx).cursor_position().line + 1;
        if in_workspace {
            let current = match &self.bookmarks_ui.last_opened {
                Some((id, at, at_line)) if *at == path && *at_line == line => Some(*id),
                _ => None,
            };
            if let Some(id) = self.bookmarks.step_in_workspace(&path, line, current, forward).map(|b| b.id) {
                self.open_bookmark(id, window, cx);
            }
        } else if let Some(line) = self.bookmarks.step_in_file(&path, line, forward) {
            self.open_file(path, Some(Target::Line { line: line - 1, column: 0 }), true, window, cx);
        }
    }

    /// Opens a bookmark's file at its line.
    pub(super) fn open_bookmark(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bookmark) = self.bookmarks.get(id).cloned() else { return };
        self.bookmarks_ui.selected = Some(RowId::Bookmark(id));
        let path = bookmark.location.path().to_path_buf();
        if !path.exists() {
            self.notify_error(format!("“{}” no longer exists.", file_name(&path)), window, cx);
            return;
        }
        let target = bookmark.location.line().map(|line| Target::Line { line: line - 1, column: 0 });
        self.open_file(path.clone(), target, true, window, cx);
        let line = self.active_buffer().map(|b| b.editor.read(cx).cursor_position().line + 1).unwrap_or(1);
        self.bookmarks_ui.last_opened = Some((id, path, line));
    }

    /// Shows a bookmark in the Bookmarks navigator.
    pub(super) fn reveal_bookmark(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.show_navigator_tab(NavigatorTab::Bookmarks, window, cx);
        if let Some(section) = self.bookmarks.sections().into_iter().find(|s| s.bookmarks.iter().any(|b| b.id == id)) {
            self.bookmarks_ui.collapsed.remove(&section.key);
        }
        self.bookmarks_ui.selected = Some(RowId::Bookmark(id));
        cx.notify();
    }

    fn remove_bookmark(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.bookmarks.remove(id) {
            self.bookmarks_changed(true, cx);
        }
    }

    fn set_bookmark_completed(&mut self, id: u64, completed: bool, cx: &mut Context<Self>) {
        self.bookmarks.set_completed(id, completed);
        self.bookmarks_changed(true, cx);
    }

    fn sort_bookmarks(&mut self, by: SortBy, cx: &mut Context<Self>) {
        self.bookmarks.sort(by);
        self.bookmarks_changed(true, cx);
    }

    fn remove_completed_bookmarks(&mut self, cx: &mut Context<Self>) {
        if self.bookmarks.remove_completed() > 0 {
            self.bookmarks_changed(true, cx);
        }
    }

    fn remove_bookmark_section(&mut self, key: &SectionKey, cx: &mut Context<Self>) {
        self.bookmarks.remove_section(key);
        self.bookmarks_changed(true, cx);
    }

    /// Edit Description: edits a bookmark's or search's description in place.
    fn begin_title_edit(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_title_edit(cx);
        let title = match (self.bookmarks.get(id), self.bookmarks.search(id)) {
            (Some(bookmark), _) => bookmark.display_title(),
            (_, Some(search)) => if search.title.is_empty() { search.query.clone() } else { search.title.clone() },
            _ => return,
        };
        self.show_navigator_tab(NavigatorTab::Bookmarks, window, cx);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_title_edit(cx),
            _ => {}
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.bookmarks_ui.editing = Some(TitleEdit { id, input, _subscription: subscription });
        cx.notify();
    }

    fn commit_title_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.bookmarks_ui.editing.take() else { return };
        let title = edit.input.read(cx).value().to_string();
        self.bookmarks.set_title(edit.id, &title);
        self.bookmarks_changed(true, cx);
    }

    fn cancel_title_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bookmarks_ui.editing.take().is_some() {
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    // ------------------------------------------------------------------
    // Gutter

    /// The ribbons at the left of `editor`'s line numbers, and their menu
    /// on a right-click (a left click goes on to the breakpoint below).
    pub(super) fn bookmark_lane(&self, editor: &Entity<EditorState>, cx: &Context<Self>) -> GutterLane {
        let workspace = cx.weak_entity();
        let editor_id = editor.entity_id();
        GutterLane::new(BOOKMARK_LANE, move |rows, cx| {
            let Some(this) = workspace.upgrade() else { return vec![] };
            let this = this.read(cx);
            let Some(path) = this.buffer_path_of(editor_id) else { return vec![] };
            let mut seen = HashSet::new();
            this.bookmarks
                .all()
                .filter(|b| b.location.path() == path)
                .filter_map(|b| Some((b.id, b.location.line()?, b.completed)))
                // One ribbon per line.
                .filter(|&(_, line, _)| rows.rows.contains(&line.saturating_sub(1)) && seen.insert(line))
                .map(|(id, line, completed)| {
                    let workspace = workspace.clone();
                    GutterItem::new(line - 1, move |paint, window, _| {
                        paint_ribbon(window, paint.bounds, ribbon_color(completed), paint.background);
                    })
                    .bounds(|cell, line_height| {
                        Bounds::new(point(cell.left() + px(1.), cell.top() + px(2.)), size(px(RIBBON_WIDTH), line_height - px(4.)))
                    })
                    .on_secondary_click(move |event, window, cx| {
                        let position = event.position;
                        workspace.update(cx, |this, cx| this.bookmark_menu_at(id, position, window, cx)).is_ok()
                    })
                })
                .collect()
        })
        .order(1)
    }

    fn bookmark_menu_at(&mut self, id: u64, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bookmark) = self.bookmarks.get(id).cloned() else { return };
        let workspace = cx.weak_entity();
        self.show_popup_menu(position, window, cx, move |menu, _, _| bookmark_menu_items(menu, &bookmark, workspace, true));
    }

    // ------------------------------------------------------------------
    // Navigator

    pub(super) fn render_bookmark_navigator(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let filter_input = match self.bookmarks_ui.filter.clone() {
            Some(input) => input,
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
                self.bookmarks_ui._subscriptions.push(cx.observe(&input, |_, _, cx| cx.notify()));
                self.bookmarks_ui.filter = Some(input.clone());
                input
            }
        };
        let filter = filter_input.read(cx).value().to_string();
        let incomplete_only = self.bookmarks_ui.incomplete_only;
        self.bookmarks_ui.rows.borrow_mut().clear();
        let shows = |b: &Bookmark| matches_filter(b, &filter) && !(incomplete_only && b.completed);

        let mut list = v_flex().id("bookmarks").flex_1().min_h_0().overflow_y_scroll();
        if self.bookmarks.is_empty() {
            list = list.child(empty("No bookmarks. Use Edit ▸ Bookmark Line (⌃⌘') or Bookmark File (⌃⇧⌘') to add one.", cx));
        }
        let sections: Vec<(SectionKey, String, Option<String>, Vec<Bookmark>)> = self
            .bookmarks
            .sections()
            .into_iter()
            .map(|s| {
                let detail = match (&s.key, s.search) {
                    (_, Some(search)) => Some(match search.results.len() {
                        1 => "1 result".to_string(),
                        n => format!("{n} results"),
                    }),
                    (SectionKey::File(path), _) => Some(self.relative(path.parent().unwrap_or(path))),
                    _ => None,
                };
                (s.key.clone(), s.title(), detail, s.bookmarks.into_iter().cloned().collect())
            })
            .collect();
        let mut shown_any = false;
        for (key, title, detail, bookmarks) in sections {
            let visible: Vec<&Bookmark> = bookmarks.iter().filter(|b| shows(b)).collect();
            let title_matches = !filter.trim().is_empty() && title.to_lowercase().contains(&filter.trim().to_lowercase());
            if visible.is_empty() && !title_matches {
                continue;
            }
            shown_any = true;
            let collapsed = self.bookmarks_ui.collapsed.contains(&key);
            list = list.child(self.section_row(&key, &title, detail, collapsed, bookmarks.first().map(|b| b.id), window, cx));
            if collapsed {
                continue;
            }
            let in_search = matches!(key, SectionKey::Search(_));
            for bookmark in visible {
                list = list.child(self.bookmark_row(bookmark, in_search, window, cx));
            }
        }
        if !self.bookmarks.is_empty() && !shown_any {
            list = list.child(empty("No bookmarks match.", cx));
        }

        let workspace = cx.weak_entity();
        let sort = Button::new("bookmarks-sort")
            .icon(IconName::ArrowUpDown)
            .ghost()
            .xsmall()
            .tooltip("Sort and clean up")
            .dropdown_menu(move |menu, _, _| {
                let item = |label: &'static str, run: fn(&mut Workspace, &mut Context<Workspace>)| {
                    let workspace = workspace.clone();
                    PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        workspace.update(cx, run).ok();
                    })
                };
                menu.item(item("Sort By Name", |ws, cx| ws.sort_bookmarks(SortBy::Name, cx)))
                    .item(item("Sort By Type", |ws, cx| ws.sort_bookmarks(SortBy::Type, cx)))
                    .item(item("Sort by Completion Status", |ws, cx| ws.sort_bookmarks(SortBy::CompletionStatus, cx)))
                    .separator()
                    .item(item("Delete All Completed Items", Workspace::remove_completed_bookmarks))
            });
        v_flex()
            .size_full()
            .child(h_flex().justify_between().child(section_header("Bookmarks", cx)).child(div().pr_1().child(sort)))
            .child(list)
            .child(
                h_flex()
                    .px_1()
                    .py_0p5()
                    .gap_1()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(div().flex_1().child(Input::new(&filter_input).xsmall().cleanable(true).prefix(Icon::new(IconName::Search).xsmall())))
                    .child(
                        Button::new("bookmarks-incomplete")
                            .icon(IconName::CircleCheck)
                            .ghost()
                            .xsmall()
                            .selected(incomplete_only)
                            .tooltip("Show only incomplete items")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.bookmarks_ui.incomplete_only = !this.bookmarks_ui.incomplete_only;
                                cx.notify();
                            })),
                    ),
            )
    }

    /// A file's or search's header: its disclosure, a drag handle for the
    /// whole section and a drop target for bookmarks dragged to its top.
    #[allow(clippy::too_many_arguments)]
    fn section_row(
        &self,
        key: &SectionKey,
        title: &str,
        detail: Option<String>,
        collapsed: bool,
        first: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let icon = match key {
            SectionKey::File(path) => file_icon(path),
            SectionKey::Search(_) => IconName::Search,
        };
        let selected = self.bookmarks_ui.selected.as_ref() == Some(&RowId::Section(key.clone()));
        let editing = match (key, &self.bookmarks_ui.editing) {
            (SectionKey::Search(id), Some(edit)) if edit.id == *id => Some(edit.input.clone()),
            _ => None,
        };
        let id = SharedString::from(format!("bookmark-section-{}", section_id(key)));
        let dragged = DraggedSection { key: key.clone(), label: title.to_string().into() };
        let (toggle_key, drop_key, menu_key, painted_key) = (key.clone(), key.clone(), key.clone(), key.clone());
        let workspace = cx.weak_entity();
        let rows = self.bookmarks_ui.rows.clone();
        let painted_title = title.to_string();
        let _ = window;
        row(id)
            .relative()
            .font_weight(FontWeight::SEMIBOLD)
            .when(selected, |el| el.bg(theme.sidebar_accent))
            .hover(|el| el.bg(theme.sidebar_accent))
            .child(record_bounds(rows, RowId::Section(painted_key), painted_title))
            .child(Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).xsmall().text_color(theme.muted_foreground))
            .child(Icon::new(icon).small().text_color(theme.muted_foreground))
            .map(|el| match editing {
                Some(input) => el.child(title_input(input, cx)),
                None => el.child(div().truncate().child(title.to_string())),
            })
            .when_some(detail, |el, detail| {
                el.child(div().flex_1().text_xs().font_weight(FontWeight::NORMAL).text_color(theme.muted_foreground).truncate().child(detail))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.bookmarks_ui.collapsed.remove(&toggle_key) {
                    this.bookmarks_ui.collapsed.insert(toggle_key.clone());
                }
                this.bookmarks_ui.selected = Some(RowId::Section(toggle_key.clone()));
                cx.notify();
            }))
            .on_drag(dragged, |section, _, _, cx| cx.new(|_| section.clone()))
            .drag_over::<DraggedSection>(|style, _, _, cx| style.bg(cx.theme().drop_target))
            .drag_over::<DraggedBookmark>(|style, _, _, cx| style.bg(cx.theme().drop_target))
            .on_drop(cx.listener(move |this, dragged: &DraggedSection, _, cx| {
                if this.bookmarks.move_section(&dragged.key, &drop_key) {
                    this.bookmarks_changed(true, cx);
                }
            }))
            .on_drop(cx.listener(move |this, dragged: &DraggedBookmark, _, cx| {
                // Onto its own section's header: to the top.
                if this.bookmarks.move_bookmark(dragged.id, first) {
                    this.bookmarks_changed(true, cx);
                }
            }))
            .context_menu(move |menu, _, _| section_menu_items(menu, &menu_key, workspace.clone()))
    }

    /// A bookmark: its completion box, description (double-click edits
    /// it), line, and delete button; a click opens it, dragging moves it.
    fn bookmark_row(&self, bookmark: &Bookmark, in_search: bool, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let id = bookmark.id;
        let completed = bookmark.completed;
        let title = bookmark.display_title();
        let detail = match (&bookmark.location, in_search) {
            (Location::Line { path, line }, true) => format!("{}:{line}", file_name(path)),
            (Location::Line { line, .. }, false) => format!("line {line}"),
            (Location::File { .. }, _) => "file".to_string(),
        };
        let icon = match &bookmark.location {
            Location::Line { .. } => IconName::Bookmark,
            Location::File { path } => file_icon(path),
        };
        let selected = self.bookmarks_ui.selected == Some(RowId::Bookmark(id));
        let editing = self.bookmarks_ui.editing.as_ref().filter(|e| e.id == id).map(|e| e.input.clone());
        let dragged = DraggedBookmark { id, label: title.clone().into() };
        let menu_bookmark = bookmark.clone();
        let workspace = cx.weak_entity();
        row(SharedString::from(format!("bookmark-{id}")))
            .relative()
            .pl_6()
            .text_xs()
            .when(selected, |el| el.bg(theme.sidebar_accent))
            .hover(|el| el.bg(theme.sidebar_accent))
            .child(record_bounds(self.bookmarks_ui.rows.clone(), RowId::Bookmark(id), title.clone()))
            .child(
                Checkbox::new(("bookmark-done", id as usize))
                    .xsmall()
                    .checked(completed)
                    .tooltip(if completed { "Mark as Incomplete" } else { "Mark as Complete" })
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        cx.stop_propagation();
                        this.set_bookmark_completed(id, *checked, cx);
                    })),
            )
            .child(Icon::new(icon).xsmall().text_color(ribbon_color(completed)))
            .map(|el| match editing {
                Some(input) => el.child(title_input(input, cx)),
                None => el.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(completed, |el| el.text_color(theme.muted_foreground).line_through())
                        .child(title),
                ),
            })
            .child(div().flex_none().text_color(theme.muted_foreground).child(detail))
            .child(
                Button::new(("bookmark-remove", id as usize))
                    .icon(IconName::X)
                    .ghost()
                    .xsmall()
                    .tooltip("Delete Bookmark")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.remove_bookmark(id, cx);
                    })),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    this.begin_title_edit(id, window, cx);
                } else {
                    this.open_bookmark(id, window, cx);
                }
            }))
            .on_drag(dragged, |bookmark, _, _, cx| cx.new(|_| bookmark.clone()))
            .drag_over::<DraggedBookmark>(|style, _, _, cx| style.bg(cx.theme().drop_target))
            .on_drop(cx.listener(move |this, dragged: &DraggedBookmark, _, cx| {
                if this.bookmarks.move_bookmark(dragged.id, Some(id)) {
                    this.bookmarks_changed(true, cx);
                }
            }))
            .context_menu(move |menu, _, _| bookmark_menu_items(menu, &menu_bookmark, workspace.clone(), false))
    }

    /// The bookmarks for the test harness's `dump`.
    pub(super) fn bookmarks_summary(&self, cx: &App) -> serde_json::Value {
        let sections: Vec<serde_json::Value> = self
            .bookmarks
            .sections()
            .iter()
            .map(|s| {
                serde_json::json!({
                    "title": s.title(),
                    "kind": if s.search.is_some() { "search" } else { "file" },
                    "query": s.search.map(|g| g.query.clone()),
                    "bookmarks": s.bookmarks.iter().map(|b| serde_json::json!({
                        "title": b.display_title(),
                        "file": file_name(b.location.path()),
                        "line": b.location.line(),
                        "completed": b.completed,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let point_json = |bounds: &Bounds<Pixels>| serde_json::json!({ "x": f32::from(bounds.center().x), "y": f32::from(bounds.center().y) });
        let rows: Vec<serde_json::Value> = self
            .bookmarks_ui
            .rows
            .borrow()
            .iter()
            .map(|(bounds, id, title)| {
                let mut row = point_json(bounds);
                row["kind"] = if matches!(id, RowId::Section(_)) { "section" } else { "bookmark" }.into();
                row["title"] = title.clone().into();
                row
            })
            .collect();
        let gutter: Vec<serde_json::Value> = self
            .painted_gutter(BOOKMARK_LANE, cx)
            .into_iter()
            .map(|(line, bounds)| {
                let mut ribbon = point_json(&bounds);
                ribbon["line"] = line.into();
                ribbon
            })
            .collect();
        let selected = match &self.bookmarks_ui.selected {
            Some(RowId::Bookmark(id)) => self.bookmarks.get(*id).map(|b| b.display_title()),
            Some(RowId::Section(SectionKey::Search(id))) => self.bookmarks.search(*id).map(|g| g.display_title()),
            Some(RowId::Section(SectionKey::File(path))) => Some(file_name(path)),
            None => None,
        };
        serde_json::json!({
            "sections": sections,
            "rows": if self.show_navigator && self.navigator == NavigatorTab::Bookmarks { rows } else { vec![] },
            "gutter": gutter,
            "selected": selected,
            "editing": self.bookmarks_ui.editing.as_ref().map(|e| e.input.read(cx).value().to_string()),
            "filter": self.bookmarks_ui.filter.as_ref().map(|f| f.read(cx).value().to_string()),
            "incomplete_only": self.bookmarks_ui.incomplete_only,
            "dialog": self.bookmarks_ui.dialog.as_ref().and_then(|d| d.upgrade()).map(|d| d.read(cx).summary(cx)),
        })
    }

    /// The test harness's `bookmark` command, for what has no keystroke:
    /// `complete <n> on|off` and `delete <n>` (the nth bookmark in the
    /// navigator's order), `menu <n>` (its gutter menu), `sort name|type|status`,
    /// `delete-completed`, `filter <text>`, `incomplete on|off`,
    /// `find-result <file> <match>` (a Find navigator result's Bookmark
    /// Line), `find-results` (its bookmark button), `find-again <search>`,
    /// `title <text>` (the open dialog's description) and `file <path>` (the
    /// project navigator's Bookmark File).
    pub fn bookmark_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
        let nth = |this: &Self, text: &str| -> Result<u64, String> {
            let index: usize = text.split_whitespace().next().unwrap_or("").parse().map_err(|_| format!("usage: bookmark {verb} <index> …"))?;
            this.bookmarks.in_order().get(index).map(|b| b.id).ok_or(format!("no bookmark {index}"))
        };
        let nth_search = |this: &Self, text: &str| -> Result<u64, String> {
            let index: usize = text.trim().parse().map_err(|_| "usage: bookmark find-again <index>".to_string())?;
            this.bookmarks.sections().iter().filter_map(|s| s.search).nth(index).map(|g| g.id).ok_or(format!("no search {index}"))
        };
        match verb {
            "complete" => {
                let id = nth(self, arg)?;
                self.set_bookmark_completed(id, arg.ends_with(" on"), cx);
            }
            "delete" => {
                let id = nth(self, arg)?;
                self.remove_bookmark(id, cx);
            }
            "menu" => {
                let id = nth(self, arg)?;
                let line = self.bookmarks.get(id).and_then(|b| b.location.line());
                let position = self
                    .painted_gutter(BOOKMARK_LANE, cx)
                    .into_iter()
                    .find(|(painted, _)| Some(*painted) == line)
                    .map(|(_, bounds)| bounds.center());
                self.bookmark_menu_at(id, position.ok_or("its ribbon isn't on screen")?, window, cx);
            }
            "sort" => self.sort_bookmarks(
                match arg {
                    "name" => SortBy::Name,
                    "type" => SortBy::Type,
                    "status" => SortBy::CompletionStatus,
                    _ => return Err("usage: bookmark sort name|type|status".into()),
                },
                cx,
            ),
            "delete-completed" => self.remove_completed_bookmarks(cx),
            "filter" => {
                let filter = self.bookmarks_ui.filter.clone().ok_or("the Bookmarks navigator hasn't been shown")?;
                filter.update(cx, |input, cx| input.set_value(arg.to_string(), window, cx));
            }
            "incomplete" => {
                self.bookmarks_ui.incomplete_only = arg == "on";
                cx.notify();
            }
            "find-result" => {
                let mut parts = arg.split_whitespace().filter_map(|p| p.parse::<usize>().ok());
                let (Some(file), Some(index)) = (parts.next(), parts.next()) else { return Err("usage: bookmark find-result <file> <match>".into()) };
                let found = self.find.results.get(file).ok_or(format!("no result file {file}"))?;
                let m = found.matches.get(index).ok_or(format!("no match {index}"))?;
                let (path, line, text) = (found.path.clone(), m.line, m.preview.clone());
                self.bookmark_find_result(path, line, text, window, cx);
            }
            "find-results" => self.bookmark_find_results(window, cx),
            "find-again" => {
                let id = nth_search(self, arg)?;
                self.find_again(id, cx);
            }
            "title" => {
                let dialog = self.bookmarks_ui.dialog.as_ref().and_then(|d| d.upgrade()).ok_or("no bookmark dialog is open")?;
                dialog.update(cx, |dialog, cx| dialog.set_title(arg, window, cx));
            }
            "file" => self.bookmark_path(PathBuf::from(arg), window, cx),
            other => return Err(format!("unknown bookmark command {other}")),
        }
        Ok(())
    }
}

/// A Find navigator's results as (file, 1-based line, line text).
fn find_results(results: &[crate::search::FileMatches]) -> Vec<(PathBuf, u32, String)> {
    results
        .iter()
        .flat_map(|file| file.matches.iter().map(|m| (file.path.clone(), m.line + 1, m.preview.clone())))
        .collect()
}

fn section_id(key: &SectionKey) -> String {
    match key {
        SectionKey::File(path) => path.to_string_lossy().to_string(),
        SectionKey::Search(id) => format!("search-{id}"),
    }
}

/// Records where a row was laid out, as an invisible child.
fn record_bounds(rows: PaintedRows, id: RowId, title: String) -> impl IntoElement {
    canvas(move |bounds, _, _| rows.borrow_mut().push((bounds, id, title)), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}

/// The description being edited; Return or a click elsewhere keeps it,
/// Escape cancels.
fn title_input(input: Entity<InputState>, cx: &mut Context<Workspace>) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .key_context("BookmarkTitle")
        .on_action(cx.listener(|this, _: &PickerDismiss, window, cx| this.cancel_title_edit(window, cx)))
        .child(Input::new(&input).xsmall())
}

/// A bookmark's menu (the navigator's context menu, or the ribbon's).
fn bookmark_menu_items(menu: PopupMenu, bookmark: &Bookmark, workspace: WeakEntity<Workspace>, in_gutter: bool) -> PopupMenu {
    let (id, completed) = (bookmark.id, bookmark.completed);
    let path = bookmark.location.path().to_path_buf();
    let with = |run: MenuAction| menu_action(workspace.clone(), run);
    let reveal_file = path.clone();
    let menu = menu
        .item(PopupMenuItem::new("Edit Description").on_click(with(Box::new(move |ws, window, cx| ws.begin_title_edit(id, window, cx)))))
        .item(PopupMenuItem::new(if completed { "Mark as Incomplete" } else { "Mark as Complete" }).on_click(with(Box::new(
            move |ws, _, cx| ws.set_bookmark_completed(id, !completed, cx),
        ))))
        .separator()
        .item(PopupMenuItem::new("Delete Bookmark").on_click(with(Box::new(move |ws, _, cx| ws.remove_bookmark(id, cx)))))
        .separator();
    let menu = if in_gutter {
        menu.item(PopupMenuItem::new("Reveal in Bookmark Navigator").on_click(with(Box::new(move |ws, window, cx| ws.reveal_bookmark(id, window, cx)))))
    } else {
        menu.item(PopupMenuItem::new("Reveal in Project Navigator").on_click(with(Box::new(move |ws, window, cx| {
            ws.reveal_in_navigator(&reveal_file, window, cx)
        }))))
    };
    menu.item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&path)))
}

/// A section's menu: Find Again and Show in Find Navigator for a search,
/// Reveal and Show in Finder for a file, and deleting it all.
fn section_menu_items(menu: PopupMenu, key: &SectionKey, workspace: WeakEntity<Workspace>) -> PopupMenu {
    let with = |run: MenuAction| menu_action(workspace.clone(), run);
    let delete_key = key.clone();
    match key {
        SectionKey::Search(id) => {
            let id = *id;
            menu.item(PopupMenuItem::new("Find Again").on_click(with(Box::new(move |ws, _, cx| ws.find_again(id, cx)))))
                .item(PopupMenuItem::new("Show in Find Navigator").on_click(with(Box::new(move |ws, window, cx| {
                    ws.show_search_in_find(id, window, cx)
                }))))
                .item(PopupMenuItem::new("Edit Description").on_click(with(Box::new(move |ws, window, cx| ws.begin_title_edit(id, window, cx)))))
                .separator()
                .item(PopupMenuItem::new("Delete Search").on_click(with(Box::new(move |ws, _, cx| ws.remove_bookmark_section(&delete_key, cx)))))
        }
        SectionKey::File(path) => {
            let (reveal, finder) = (path.clone(), path.clone());
            menu.item(PopupMenuItem::new("Reveal in Project Navigator").on_click(with(Box::new(move |ws, window, cx| {
                ws.reveal_in_navigator(&reveal, window, cx)
            }))))
            .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&finder)))
            .separator()
            .item(PopupMenuItem::new("Delete Bookmarks in File").on_click(with(Box::new(move |ws, _, cx| {
                ws.remove_bookmark_section(&delete_key, cx)
            }))))
        }
    }
}

fn menu_action(workspace: WeakEntity<Workspace>, run: MenuAction) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| {
        workspace.update(cx, |ws, cx| run(ws, window, cx)).ok();
    }
}

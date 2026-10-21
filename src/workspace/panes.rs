//! The editor area's panes: opening files in them, tabs, splits (File ▸ New
//! ▸ Editor Pane on Right / Below, Close Editor Pane), moving focus between
//! panes, and Xcode's assistant: counterparts that follow the focused
//! editor, and ⌥-click / ⌥-Return / ⌃⌥⌘J opening in the next pane.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::*;

use super::pane::{Assistant, AssistantMode, EditorItem, Item, Pane, PaneId};
use super::pane_group::{Member, PaneGroup, SplitAxis};
use super::{ProjectSettings, Target, Workspace};
use crate::lsp;
use crate::session::{PaneLayout, PaneState, ProjectSession, SavedPreview};

/// The narrowest (and lowest) a pane can be: the splits' minimum
/// (`PANEL_MIN_SIZE`) and the divider beside it.
const MIN_PANE_SIZE: f32 = 101.;

impl Workspace {
    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    pub(super) fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|p| p.id == id)
    }

    /// The focused pane's file tab.
    pub fn active_item(&self) -> Option<&EditorItem> {
        self.pane(self.active_pane)?.active_editor_item()
    }

    /// The focused pane's comparison, when it shows one.
    pub fn active_diff(&self) -> Option<&Entity<super::diff_view::DiffView>> {
        match self.pane(self.active_pane)?.active_item()? {
            Item::Diff(diff) => Some(&diff.view),
            _ => None,
        }
    }

    /// The file the focused tab edits: a file tab's, or a comparison's.
    pub fn active_file_path(&self) -> Option<PathBuf> {
        self.pane(self.active_pane)?.active_item()?.buffer_path().map(Path::to_path_buf)
    }

    /// Whether the focused pane shows the project editor.
    pub fn showing_project_settings(&self) -> bool {
        matches!(self.pane(self.active_pane).and_then(Pane::active_item), Some(Item::ProjectSettings(_)))
    }

    /// The pane and tab of an editor, property list editor or the project
    /// settings (by entity).
    pub(super) fn find_view(&self, view: EntityId) -> Option<(PaneId, usize)> {
        self.panes.iter().find_map(|pane| {
            let ix = pane.items.iter().position(|item| match item {
                Item::Editor(item) => {
                    item.editor.entity_id() == view || item.plist.as_ref().is_some_and(|p| p.editor.entity_id() == view)
                }
                other => other.view_id() == view,
            })?;
            Some((pane.id, ix))
        })
    }

    fn add_pane(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        self.panes.push(Pane::new(id));
        id
    }

    // ---------------------------------------------------------------------
    // Opening files and tabs

    /// Opens (or focuses) a file in the focused pane, optionally revealing
    /// a location.
    pub fn open_file(
        &mut self,
        path: PathBuf,
        target: Option<Target>,
        record_history: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file_in(self.active_pane, path, target, record_history, window, cx);
    }

    /// Opens (or focuses) a file in `pane` and focuses it there.
    pub fn open_file_in(
        &mut self,
        pane: PaneId,
        path: PathBuf,
        target: Option<Target>,
        record_history: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = path.canonicalize().unwrap_or(path);
        // An asset catalog, or a set in one, opens the catalog's tab.
        if let Some((catalog, selected)) = crate::xcassets::locate(&path) {
            self.open_asset_catalog_in(pane, catalog, selected, window, cx);
            return;
        }
        if record_history {
            self.push_history(pane, cx);
        }
        let Some((ix, created)) = self.add_item(pane, &path, window, cx) else { return };
        self.activate_item(pane, ix, window, cx);
        let Some(target) = target else { return };
        self.reveal_target(pane, ix, target.clone(), window, cx);
        if !created {
            return;
        }
        // Scrolling needs the editor's first layout; reveal again after it
        // unless the user has already moved the cursor.
        let Some(view) = self.pane(pane).and_then(|p| p.items[ix].editor()).map(|item| item.editor.clone()) else { return };
        let revealed_at = view.read(cx).selected_range();
        let view = view.entity_id();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(32)).await;
            this.update_in(cx, |this, window, cx| {
                if let Some((pane, ix)) = this.find_view(view)
                    && this.pane(pane).and_then(|p| p.items[ix].editor()).is_some_and(|item| item.editor.read(cx).selected_range() == revealed_at)
                {
                    this.reveal_target(pane, ix, target, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// `path`'s tab in `pane`, added (and its file read) if needed; whether
    /// it was added.
    fn add_item(&mut self, pane: PaneId, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Option<(usize, bool)> {
        if let Some(ix) = self.pane(pane)?.position_of(path) {
            return Some((ix, false));
        }
        if crate::xcassets::is_catalog(path) {
            return Some((self.add_asset_catalog_item(pane, path, None, window, cx)?, true));
        }
        if super::image_view::is_image(path) {
            return Some((self.add_image_item(pane, path, window, cx)?, true));
        }
        if self.buffer(path).is_none() && !self.open_buffer(path, window, cx) {
            return None;
        }
        let item = self.new_editor_item(path, window, cx)?;
        let pane = self.pane_mut(pane)?;
        pane.items.push(Item::Editor(item));
        let ix = pane.items.len() - 1;
        self.refresh_diagnostics_for(path, cx);
        if self.exec_location.as_ref().is_some_and(|(stop, _)| stop == path) {
            self.set_exec_mark(self.exec_location.clone(), cx);
        }
        Some((ix, true))
    }

    pub(super) fn reveal_target(&mut self, pane: PaneId, ix: usize, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.pane(pane).and_then(|p| p.items.get(ix)).and_then(Item::editor) else { return };
        item.editor.update(cx, |state, cx| {
            let text = state.text().clone();
            let range = match target {
                Target::Offset(offset) => offset..offset,
                Target::Line { line, column } => {
                    let offset = lsp::lsp_to_offset(&text, lsp_types::Position::new(line, column));
                    offset..offset
                }
                Target::LspRange(range) => lsp::lsp_range_to_offsets(&text, range),
            };
            // A target inside a fold (⌘L, a definition, an issue) opens it.
            state.unfold_at(zcode_editor::RopeExt::offset_to_position(&text, range.start), cx);
            state.unfold_at(zcode_editor::RopeExt::offset_to_position(&text, range.end), cx);
            state.set_selected_range(range.start..range.start, cx);
            // Selected, the symbol would be Vim's visual mode: its cursor
            // goes to the start instead.
            if range.end > range.start && !state.is_vim_enabled() {
                state.set_selected_range(range, cx);
            }
            state.focus(window, cx);
        });
        cx.notify();
    }

    /// Shows tab `ix` of `pane` and focuses it.
    pub fn activate_item(&mut self, pane: PaneId, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.pane_mut(pane) else { return };
        if ix >= p.items.len() {
            return;
        }
        p.active = ix;
        let focus = p.items[ix].focus_handle(cx);
        let settings = match &p.items[ix] {
            Item::ProjectSettings(settings) => Some(settings.clone()),
            Item::Editor(_)
            | Item::Diff(_)
            | Item::MultiBuffer(_)
            | Item::Terminal(_)
            | Item::Organizer(_)
            | Item::AssetCatalog(_)
            | Item::Image(_)
            | Item::Rendered(_) => None,
        };
        self.active_pane = pane;
        self.inspected = None;
        if let Some(settings) = settings {
            settings.update(cx, |s, cx| s.reload_if_changed(window, cx));
        }
        // A catalog shown again is read again (Finder, git or Xcode may
        // have changed it).
        if let Some(Item::AssetCatalog(item)) = self.pane(pane).and_then(|p| p.items.get(ix)) {
            item.view.update(cx, |view, cx| {
                if view.catalog.is_some() && !view.busy {
                    view.reload(window, cx);
                }
            });
        }
        if let Some(Item::Image(item)) = self.pane(pane).and_then(|p| p.items.get(ix)) {
            item.view.update(cx, |view, cx| view.reload_if_changed(cx));
        }
        focus.focus(window, cx);
        self.activated(window, cx);
    }

    /// After the focused pane, or the tab it shows, changed.
    fn activated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A menu (Show Callers', a context menu) was about what was shown.
        // Focus leaving it closes it too, but an inactive window gets no
        // focus events.
        self.popup_menu = None;
        window.set_window_title(&self.title());
        window.set_window_edited(self.buffers.iter().any(|b| b.dirty));
        self.release_hidden_layouts(cx);
        self.reveal_active_in_tree(cx);
        self.follow_counterparts(self.active_pane, window, cx);
        cx.notify();
    }

    /// Editors in tabs no pane shows let go of their last layout; the shown
    /// ones keep theirs. Deferred, as one of them may be what's updating.
    fn release_hidden_layouts(&self, cx: &mut Context<Self>) {
        let hidden: Vec<_> = self
            .panes
            .iter()
            .flat_map(|pane| pane.items.iter().enumerate().filter(move |(ix, _)| *ix != pane.active))
            .filter_map(|(_, item)| item.editor().map(|item| item.editor.clone()))
            .collect();
        cx.defer(move |cx| {
            for editor in hidden {
                editor.update(cx, |state, _| state.release_layout());
            }
        });
    }

    /// A pane's editor got focus (a click, Tab): its pane is the focused one.
    pub(super) fn view_focused(&mut self, view: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((pane, ix)) = self.find_view(view) else { return };
        if self.active_pane == pane && self.pane(pane).is_some_and(|p| p.active == ix) {
            return;
        }
        if let Some(p) = self.pane_mut(pane) {
            p.active = ix;
        }
        self.active_pane = pane;
        self.inspected = None;
        self.activated(window, cx);
    }

    /// Focuses the focused pane's tab, or the workspace when it has none.
    pub(super) fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.pane(self.active_pane).filter(|p| !p.items.is_empty()).map(|p| p.active) {
            Some(ix) => self.activate_item(self.active_pane, ix, window, cx),
            None => {
                self.focus_handle.focus(window, cx);
                self.activated(window, cx);
            }
        }
    }

    /// Focus was on something no longer drawn (a navigator, the debug
    /// area, the inspector or the coding assistant was hidden or switched
    /// away): it goes to the focused pane's tab, or the workspace.
    pub(super) fn restore_lost_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self.pane(self.active_pane).and_then(Pane::active_item).map(|item| item.focus_handle(cx));
        shown.unwrap_or_else(|| self.focus_handle.clone()).focus(window, cx);
        cx.notify();
    }

    /// Selects the shown file's row in the project navigator; nothing is
    /// selected when it has no row on show (its folder is collapsed, it's
    /// outside the project, or the tab isn't a file), rather than the file
    /// shown before. Reveal in Project Navigator (⇧⌘J) expands to it.
    pub(super) fn reveal_active_in_tree(&mut self, cx: &mut Context<Self>) {
        // A file's tab, or an asset catalog's.
        let shown = self.pane(self.active_pane).and_then(Pane::active_item).and_then(Item::path);
        let id: Option<SharedString> = shown.map(|path| path.to_string_lossy().to_string().into());
        self.tree.update(cx, |tree, cx| match id.and_then(|id| tree.index_of(&id)) {
            Some(ix) => {
                tree.set_selected_index(Some(ix), cx);
                tree.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
            None => tree.set_selected_index(None, cx),
        });
    }

    /// Closes a tab. The last tab showing a file with unsaved changes asks
    /// first; a pane left without tabs closes too, unless it's the only one.
    pub fn close_item(&mut self, pane: PaneId, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.pane(pane).and_then(|p| p.items.get(ix)) else { return };
        let view = item.view_id();
        let unsaved = self.unsaved_if_closed([item], cx);
        self.confirm_close(unsaved, window, cx, move |this, window, cx| {
            if let Some((pane, ix)) = this.find_view(view) {
                this.force_close_item(pane, ix, window, cx);
            }
        });
    }

    /// The files whose last tabs are among `items` and that have unsaved
    /// changes. A preview left open doesn't count (it doesn't edit the
    /// file), but the last preview of a file no tab edits does.
    pub(super) fn unsaved_if_closed<'a>(&self, items: impl IntoIterator<Item = &'a Item>, cx: &App) -> Vec<PathBuf> {
        // A comparison's local side is a view of the file too, and so is
        // each excerpted file's view in a multibuffer.
        let mut closing: Vec<PathBuf> = vec![];
        let mut previews: Vec<PathBuf> = vec![];
        for item in items {
            match item {
                Item::MultiBuffer(item) => closing.extend(item.view.read(cx).paths.values().cloned()),
                Item::Rendered(item) => previews.push(item.path.clone()),
                item => closing.extend(item.buffer_path().map(Path::to_path_buf)),
            }
        }
        let mut paths: Vec<PathBuf> = closing.iter().chain(&previews).cloned().collect();
        paths.sort();
        paths.dedup();
        paths.retain(|path| {
            let count = closing.iter().filter(|closed| *closed == path).count();
            let open = self.views_of(path).len() + self.diff_views_of(path).len() + self.multibuffer_views_of(path, cx).len();
            let last_preview = open > 0 || previews.iter().filter(|p| *p == path).count() == self.rendered_views_of(path).len();
            self.buffer(path).is_some_and(|b| b.dirty) && open == count && last_preview
        });
        paths
    }

    /// Runs `then` once the user chose to save or discard `unsaved` (right
    /// away when there is none); Save saves them first. Don't Save puts the
    /// saved text back in a buffer a preview keeps open.
    fn confirm_close(
        &mut self,
        unsaved: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if unsaved.is_empty() {
            then(self, window, cx);
            return;
        }
        let message = match &unsaved[..] {
            [path] => format!("Do you want to save the changes you made to “{}”?", super::pane::file_title(path)),
            paths => format!("Do you want to save the changes you made to {} files?", paths.len()),
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some("Your changes will be lost if you don’t save them."),
            &[PromptButton::ok("Save"), PromptButton::new("Don’t Save"), PromptButton::cancel("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await.unwrap_or(2);
            this.update_in(cx, |this, window, cx| {
                if choice == 0 {
                    for path in &unsaved {
                        this.save_path(path, window, cx);
                    }
                }
                if choice <= 1 {
                    then(this, window, cx);
                }
                if choice == 1 {
                    for path in &unsaved {
                        this.discard_changes(path, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn force_close_item(&mut self, pane: PaneId, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let closed = self.pane(pane).is_some_and(|p| p.active == ix).then(|| self.current_location(pane, cx)).flatten();
        let Some(p) = self.pane_mut(pane).filter(|p| ix < p.items.len()) else { return };
        let item = p.remove(ix);
        let emptied = p.items.is_empty();
        self.drop_items(vec![item], cx);
        if emptied && self.panes.len() > 1 && self.pane(pane).is_some_and(|p| p.assistant.is_none_or(|a| a.mode == AssistantMode::Manual)) {
            self.remove_pane(pane, window, cx);
            return;
        }
        if let Some(closed) = closed {
            self.shown_tab_closed(pane, closed, cx);
        }
        if pane == self.active_pane {
            self.focus_active(window, cx);
        }
        window.set_window_edited(self.buffers.iter().any(|b| b.dirty));
        cx.notify();
    }

    /// Lets go of tabs taken out of their pane.
    fn drop_items(&mut self, items: Vec<Item>, cx: &mut Context<Self>) {
        for item in items {
            match item {
                Item::Editor(item) => {
                    let (path, view) = (item.path.clone(), item.editor.entity_id());
                    drop(item);
                    self.view_closed(&path, view, cx);
                }
                Item::Diff(diff) => {
                    let path = diff.path.clone();
                    let view = diff.view.read(cx).current.entity_id();
                    drop(diff);
                    self.view_closed(&path, view, cx);
                }
                Item::MultiBuffer(item) => {
                    let views: Vec<(PathBuf, EntityId)> =
                        item.view.read(cx).source_views(cx).into_iter().map(|(path, view)| (path, view.entity_id())).collect();
                    drop(item);
                    for (path, view) in views {
                        self.view_closed(&path, view, cx);
                    }
                }
                // Closing a terminal's tab ends its processes.
                Item::Terminal(item) => item.view.update(cx, |view, _| view.stop()),
                Item::ProjectSettings(item) => {
                    // A value typed in a field that still has the keyboard.
                    item.update(cx, |settings, cx| settings.commit_pending(cx));
                    self.project_settings = None;
                    self.project_settings_subscriptions.clear();
                }
                Item::Organizer(_) => {
                    self.organizer = None;
                    self.organizer_subscriptions.clear();
                }
                Item::AssetCatalog(_) => {}
                Item::Image(item) => item.view.update(cx, |view, cx| view.release(cx)),
                Item::Rendered(item) => {
                    item.view.release(cx);
                    let path = item.path.clone();
                    drop(item);
                    self.rendered_closed(&path, cx);
                }
            }
        }
    }

    /// Closes every tab of `path`, its previews too, without asking (it was
    /// trashed or renamed).
    pub(super) fn close_file_everywhere(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let mut removed = vec![];
        for pane in &mut self.panes {
            while let Some(ix) = pane.items.iter().position(|item| item.path() == Some(path) || item.rendered_path() == Some(path)) {
                removed.push(pane.remove(ix));
            }
        }
        self.drop_items(removed, cx);
        self.focus_active(window, cx);
    }

    /// The tab before or after the shown one in the focused pane.
    pub(super) fn step_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.pane(self.active_pane).filter(|p| !p.items.is_empty()) else { return };
        let len = pane.items.len();
        let ix = if forward { (pane.active + 1) % len } else { (pane.active + len - 1) % len };
        self.activate_item(self.active_pane, ix, window, cx);
    }

    /// File ▸ Close Other Tabs (⌥⌘W).
    pub(super) fn close_other_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.active_pane;
        let Some(p) = self.pane(pane) else { return };
        let keep = p.active;
        let views: Vec<EntityId> = p
            .items
            .iter()
            .enumerate()
            .filter(|(ix, _)| *ix != keep)
            .map(|(_, item)| item.view_id())
            .collect();
        let unsaved = self.unsaved_if_closed(p.items.iter().enumerate().filter(|(ix, _)| *ix != keep).map(|(_, item)| item), cx);
        self.confirm_close(unsaved, window, cx, move |this, window, cx| {
            for view in views {
                if let Some((pane, ix)) = this.find_view(view) {
                    this.force_close_item(pane, ix, window, cx);
                }
            }
        });
    }

    // ---------------------------------------------------------------------
    // Splits

    /// File ▸ New ▸ Editor Pane on Right (⌃⌘T) / Below (⌃⌥⌘T): a new pane
    /// after the focused one, showing its file where it is, focused.
    pub fn split_pane(&mut self, axis: SplitAxis, window: &mut Window, cx: &mut Context<Self>) {
        let from = self.active_pane;
        if !self.split_fits(from, axis, window, cx) {
            return;
        }
        let shown = self.active_item().map(|item| (item.path.clone(), item.editor.read(cx).cursor(), item.plist.as_ref().map(|p| p.as_source)));
        let new = self.add_pane();
        self.pane_group.split(from, new, axis);
        self.zoomed = false;
        match shown {
            Some((path, cursor, as_source)) => {
                self.open_file_in(new, path, Some(Target::Offset(cursor)), false, window, cx);
                if let Some(as_source) = as_source
                    && let Some(ix) = self.pane(new).filter(|p| !p.items.is_empty()).map(|p| p.active)
                {
                    self.open_as(new, ix, as_source, window, cx);
                }
            }
            None => {
                self.active_pane = new;
                self.focus_active(window, cx);
            }
        }
    }

    /// Whether another pane fits beside (or below) `pane` with every pane
    /// at least the splits' minimum size; if not, the user is told. Panes
    /// past the editor area's edge would be off screen, the focused one too.
    fn split_fits(&self, pane: PaneId, axis: SplitAxis, window: &mut Window, cx: &mut App) -> bool {
        let size = self.editor_area_size.get();
        // Not laid out yet: nothing to go by.
        if size.width <= px(0.) {
            return true;
        }
        // Zoomed, the other panes come back as the split shows.
        let fits = self.pane_group.split_fits(pane, axis, (f32::from(size.width), f32::from(size.height)), MIN_PANE_SIZE);
        if !fits {
            let message = match axis {
                SplitAxis::Horizontal => "There’s no room for another editor pane on the right. Make the window wider, or close a pane.",
                SplitAxis::Vertical => "There’s no room for another editor pane below. Make the window taller, or close a pane.",
            };
            // One notice however many times it's tried.
            struct NoRoomForPane;
            let notice = gpui_kit::component::notification::Notification::warning(message).id::<NoRoomForPane>();
            gpui_kit::component::WindowExt::push_notification(window, notice, cx);
        }
        fits
    }

    /// File ▸ Close Editor Pane (⌃⇧⌘W). The only pane stays.
    pub fn close_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if self.panes.len() < 2 {
            return;
        }
        let Some(p) = self.pane(pane) else { return };
        let unsaved = self.unsaved_if_closed(&p.items, cx);
        self.confirm_close(unsaved, window, cx, move |this, window, cx| this.remove_pane(pane, window, cx));
    }

    /// File ▸ Close Other Editor Panes (⌃⌥⇧⌘W).
    pub(super) fn close_other_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keep = self.active_pane;
        let others: Vec<PaneId> = self.panes.iter().map(|p| p.id).filter(|id| *id != keep).collect();
        let unsaved = self.unsaved_if_closed(self.panes.iter().filter(|p| p.id != keep).flat_map(|p| p.items.iter()), cx);
        self.confirm_close(unsaved, window, cx, move |this, window, cx| {
            for pane in others {
                this.remove_pane(pane, window, cx);
            }
        });
    }

    /// Takes a pane out of the split tree with its tabs. Focus goes to the
    /// pane it assisted, or the one before it.
    fn remove_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.panes.iter().position(|p| p.id == pane) else { return };
        let assisted = self.panes[ix].assistant.map(|a| a.of).filter(|of| self.pane(*of).is_some());
        let before = self.pane_group.next(pane, false);
        if !self.pane_group.remove(pane) {
            return;
        }
        let removed = self.panes.remove(ix);
        self.drop_items(removed.items, cx);
        // Its assistants stay, as ordinary panes.
        for p in &mut self.panes {
            if p.assistant.is_some_and(|a| a.of == pane) {
                p.assistant = None;
            }
        }
        if self.panes.len() < 2 {
            self.zoomed = false;
        }
        if self.active_pane == pane || self.pane(self.active_pane).is_none() {
            self.active_pane = assisted.unwrap_or(before);
        }
        self.focus_active(window, cx);
    }

    /// Navigate ▸ Move Focus to Next / Previous Editor Pane (⌃` / ⌃⇧`).
    pub fn focus_next_pane(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.active_pane = self.pane_group.next(self.active_pane, forward);
        self.focus_active(window, cx);
    }

    /// Focuses a pane by its place in the layout (the control channel's
    /// `pane focus <n>`).
    pub fn focus_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane(pane).is_some() {
            self.active_pane = pane;
            self.focus_active(window, cx);
        }
    }

    /// View ▸ Editor ▸ Focus Editor Pane (⌃⇧⌘↩): the focused pane alone,
    /// or all of them again.
    pub(super) fn toggle_zoom(&mut self, cx: &mut Context<Self>) {
        self.zoomed = !self.zoomed && self.panes.len() > 1;
        cx.notify();
    }

    /// A click in a pane focuses it (its editor takes the click itself; an
    /// empty pane focuses the workspace).
    pub(super) fn pane_clicked(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_pane == pane {
            return;
        }
        self.active_pane = pane;
        if self.pane(pane).is_some_and(|p| p.items.is_empty()) {
            self.focus_handle.focus(window, cx);
        }
        self.activated(window, cx);
    }

    // ---------------------------------------------------------------------
    // The assistant

    /// The pane assisting `pane`.
    fn assistant_of(&self, pane: PaneId) -> Option<PaneId> {
        self.panes.iter().find(|p| p.assistant.is_some_and(|a| a.of == pane)).map(|p| p.id)
    }

    /// Editor ▸ Assistant (⌃⌥⌘↩): shows the focused editor's assistant on
    /// its right, in Counterparts, or hides it.
    pub fn toggle_assistant(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.active_pane;
        if self.pane(focused).is_some_and(|p| p.assistant.is_some()) {
            self.close_pane(focused, window, cx);
            return;
        }
        if let Some(assistant) = self.assistant_of(focused) {
            self.close_pane(assistant, window, cx);
            return;
        }
        if !self.split_fits(focused, SplitAxis::Horizontal, window, cx) {
            return;
        }
        let new = self.add_pane();
        self.pane_group.split(focused, new, SplitAxis::Horizontal);
        if let Some(p) = self.pane_mut(new) {
            p.assistant = Some(Assistant { of: focused, mode: AssistantMode::Counterparts });
        }
        self.zoomed = false;
        self.follow_counterparts(focused, window, cx);
        cx.notify();
    }

    /// Editor ▸ Show Editor Only (⌘↩): closes the focused editor's assistant.
    pub(super) fn show_editor_only(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let primary = self.pane(self.active_pane).and_then(|p| p.assistant).map_or(self.active_pane, |a| a.of);
        if let Some(assistant) = self.assistant_of(primary) {
            self.active_pane = primary;
            self.close_pane(assistant, window, cx);
        }
    }

    /// Where ⌥-click and ⌥-Return open: the focused editor's assistant (or
    /// the focused assistant itself), else the next pane, else a new
    /// assistant on the right.
    fn next_pane_for_open(&mut self) -> PaneId {
        let focused = self.active_pane;
        if let Some(assistant) = self.assistant_of(focused) {
            return assistant;
        }
        if self.pane(focused).is_some_and(|p| p.assistant.is_some()) {
            return focused;
        }
        if self.panes.len() > 1 {
            return self.pane_group.next(focused, true);
        }
        let new = self.add_pane();
        self.pane_group.split(focused, new, SplitAxis::Horizontal);
        if let Some(p) = self.pane_mut(new) {
            p.assistant = Some(Assistant { of: focused, mode: AssistantMode::Manual });
        }
        self.zoomed = false;
        new
    }

    /// Opens a file in the next pane (⌥-click in the navigator, ⌥-Return in
    /// Open Quickly). An assistant it opens in shows it from then on
    /// (Manual), as in Xcode.
    pub fn open_in_next_pane(&mut self, path: PathBuf, target: Option<Target>, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.pane_for_open_in_next();
        self.open_file_in(pane, path, target, true, window, cx);
    }

    /// `next_pane_for_open`, an assistant it is showing what's opened in
    /// it from then on.
    pub(super) fn pane_for_open_in_next(&mut self) -> PaneId {
        let pane = self.next_pane_for_open();
        if let Some(assistant) = self.pane_mut(pane).and_then(|p| p.assistant.as_mut()) {
            assistant.mode = AssistantMode::Manual;
        }
        pane
    }

    /// A header's implementation file or an implementation's header, next
    /// to it.
    pub(super) fn counterpart_of(path: &Path) -> Option<PathBuf> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let candidates: &[&str] = match ext {
            "h" | "hpp" | "hh" => &["m", "mm", "c", "cpp", "cc"],
            "m" | "mm" | "c" | "cpp" | "cc" => &["h", "hpp", "hh"],
            _ => &[],
        };
        candidates.iter().map(|candidate| path.with_extension(candidate)).find(|other| other.exists())
    }

    /// Counterparts: the assistants of `pane` show the counterpart of its
    /// file (none: an empty assistant says so), without taking focus.
    pub(super) fn follow_counterparts(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let assistants: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|p| p.assistant == Some(Assistant { of: pane, mode: AssistantMode::Counterparts }))
            .map(|p| p.id)
            .collect();
        if assistants.is_empty() {
            return;
        }
        let Some(path) = self.pane(pane).and_then(Pane::active_editor_item).map(|item| item.path.clone()) else { return };
        let counterpart = Self::counterpart_of(&path);
        for assistant in assistants {
            // What it showed goes, unless it's the last tab of unsaved changes.
            let stale: Vec<usize> = self
                .pane(assistant)
                .map(|p| {
                    let unsaved = self.unsaved_if_closed(&p.items, cx);
                    (0..p.items.len())
                        .rev()
                        .filter(|&ix| p.items[ix].path().is_none_or(|item| Some(item) != counterpart.as_deref() && !unsaved.iter().any(|u| u == item)))
                        .collect()
                })
                .unwrap_or_default();
            let mut removed = vec![];
            if let Some(p) = self.pane_mut(assistant) {
                for ix in stale {
                    removed.push(p.remove(ix));
                }
            }
            self.drop_items(removed, cx);
            if let Some(counterpart) = &counterpart
                && let Some((ix, _)) = self.add_item(assistant, counterpart, window, cx)
                && let Some(p) = self.pane_mut(assistant)
            {
                p.active = ix;
            }
        }
        cx.notify();
    }

    /// Navigate ▸ Jump to Next Counterpart with ⌥ (⌃⌥⌘↑): the counterpart
    /// in the next pane.
    pub(super) fn jump_to_counterpart_in_next_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(other) = self.active_item().and_then(|item| Self::counterpart_of(&item.path)) else { return };
        self.open_in_next_pane(other, None, window, cx);
    }

    /// Navigate ▸ Jump to Definition with ⌥ (⌃⌥⌘J): the definition in the
    /// next pane, the assistant when there is one.
    pub(super) fn jump_to_definition_in_next_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else { return };
        let request = editor.update(cx, |state, cx| {
            let provider = state.lsp_mut().definition_provider.clone()?;
            let text = state.text().clone();
            Some(provider.definitions(&text, state.cursor(), window, cx))
        });
        let Some(request) = request else {
            self.lsp_unavailable(window, cx);
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let Some(link) = request.await.ok().and_then(|links| links.into_iter().next()) else { return };
            let Some(path) = lsp::uri_to_path(&link.target_uri) else { return };
            let target = Some(Target::LspRange(link.target_selection_range));
            this.update_in(cx, |this, window, cx| this.open_in_next_pane(path, target, window, cx)).ok();
        })
        .detach();
    }

    // ---------------------------------------------------------------------
    // Project settings

    /// Shows the project editor (opening it if needed), like clicking the
    /// project in Xcode's navigator: where it is, or as the focused pane's
    /// first tab.
    pub fn open_project_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = match self.project_settings.clone() {
            Some(settings) => settings,
            None => {
                let projects = self.xcode_projects.clone();
                if projects.is_empty() {
                    return;
                }
                let settings = cx.new(|cx| ProjectSettings::new(projects, window, cx));
                let view = settings.entity_id();
                let focus = settings.read(cx).focus_handle(cx);
                self.project_settings_subscriptions = vec![
                    cx.subscribe_in(&settings, window, Self::on_project_settings_event),
                    cx.on_focus_in(&focus, window, move |this, window, cx| this.view_focused(view, window, cx)),
                ];
                self.project_settings = Some(settings.clone());
                settings
            }
        };
        let (pane, ix) = match self.find_view(settings.entity_id()) {
            Some(found) => found,
            None => {
                let pane = self.active_pane;
                let Some(p) = self.pane_mut(pane) else { return };
                if !p.items.is_empty() {
                    p.active += 1;
                }
                p.items.insert(0, Item::ProjectSettings(settings));
                (pane, 0)
            }
        };
        self.activate_item(pane, ix, window, cx);
    }

    pub fn close_project_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let found = self.project_settings.as_ref().and_then(|settings| self.find_view(settings.entity_id()));
        match found {
            Some((pane, ix)) => self.force_close_item(pane, ix, window, cx),
            None => {
                self.project_settings = None;
                self.project_settings_subscriptions.clear();
            }
        }
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // Session

    /// The editor area as the session keeps it.
    pub(super) fn pane_layout(&self, cx: &App) -> PaneLayout {
        let order = self.pane_group.panes();
        self.member_layout(&self.pane_group.root, &order, cx)
    }

    fn member_layout(&self, member: &Member, order: &[PaneId], cx: &App) -> PaneLayout {
        match member {
            Member::Split { axis, members } => {
                let key = SharedString::from(Member::split_key(members));
                let sizes = self
                    .split_states
                    .get(&key)
                    .map(|state| state.read(cx).sizes().iter().map(|size| f32::from(*size)).collect())
                    .unwrap_or_else(|| self.restored_split_sizes.get(&key).cloned().unwrap_or_default());
                PaneLayout::Split { axis: *axis, members: members.iter().map(|m| self.member_layout(m, order, cx)).collect(), sizes }
            }
            Member::Pane(id) => {
                let pane = self.pane(*id);
                PaneLayout::Pane(PaneState {
                    files: pane.map(|p| p.items.iter().filter_map(Item::path).map(Path::to_path_buf).collect()).unwrap_or_default(),
                    active: pane.and_then(Pane::active_item).and_then(Item::path).map(Path::to_path_buf),
                    focused: *id == self.active_pane,
                    assistant_of: pane.and_then(|p| p.assistant).and_then(|a| order.iter().position(|p| *p == a.of)),
                    counterparts: pane.and_then(|p| p.assistant).is_some_and(|a| a.mode == AssistantMode::Counterparts),
                    terminals: pane.map(|p| self.saved_terminals(&p.items, p.active, cx)).unwrap_or_default(),
                    previews: pane.map(|p| saved_previews(&p.items, p.active)).unwrap_or_default(),
                })
            }
        }
    }

    /// Reopens the saved panes and tabs (or the tabs of a session from
    /// before panes), then a file named on the command line.
    pub(super) fn restore_tabs(&mut self, saved: ProjectSession, opened_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let opened_file = opened_path.is_file()
            && !opened_path.ends_with("Package.swift")
            && crate::project::detect(&opened_path).root == self.project.root
            && opened_path.extension().is_some_and(|e| e != "xcodeproj" && e != "xcworkspace");
        let layout = saved.panes.unwrap_or_else(|| {
            PaneLayout::Pane(PaneState { files: saved.open_files, active: saved.active_file, focused: true, ..Default::default() })
        });
        self.restore_panes(&layout, window, cx);
        if opened_file {
            self.open_file(opened_path, None, false, window, cx);
        } else if self.panes.iter().any(|p| !p.items.is_empty()) {
            self.focus_active(window, cx);
        }
    }

    /// Builds the saved split tree and opens each pane's files, shown but
    /// not focused one by one (the focused pane gets focus once, after).
    fn restore_panes(&mut self, layout: &PaneLayout, window: &mut Window, cx: &mut Context<Self>) {
        let mut order = vec![];
        let root = self.restore_member(layout, &mut order);
        self.pane_group = PaneGroup { root };
        for (state, &id) in layout.panes().into_iter().zip(&order) {
            if let Some(p) = self.pane_mut(id) {
                p.assistant = state.assistant_of.and_then(|ix| order.get(ix)).filter(|of| **of != id).map(|&of| Assistant {
                    of,
                    mode: if state.counterparts { AssistantMode::Counterparts } else { AssistantMode::Manual },
                });
            }
            for path in state.files.iter().filter(|p| p.exists()) {
                self.add_item(id, path, window, cx);
            }
            let active = state.active.as_ref().and_then(|path| self.pane(id)?.position_of(path));
            if let Some(p) = self.pane_mut(id) {
                p.active = active.unwrap_or(p.items.len().saturating_sub(1));
            }
            self.restore_pane_terminals(id, &state.terminals, window, cx);
            self.restore_pane_previews(id, &state.previews, window, cx);
            if state.focused {
                self.active_pane = id;
            }
        }
    }

    fn restore_member(&mut self, layout: &PaneLayout, order: &mut Vec<PaneId>) -> Member {
        match layout {
            PaneLayout::Split { axis, members, sizes } if members.len() > 1 => {
                let members: Vec<Member> = members.iter().map(|m| self.restore_member(m, order)).collect();
                if sizes.len() == members.len() {
                    self.restored_split_sizes.insert(Member::split_key(&members).into(), sizes.clone());
                }
                Member::Split { axis: *axis, members }
            }
            PaneLayout::Split { members, .. } => match members.first() {
                Some(only) => self.restore_member(only, order),
                None => self.restore_member(&PaneLayout::Pane(PaneState::default()), order),
            },
            PaneLayout::Pane(_) => {
                // The first pane is the one the workspace started with.
                let id = if order.is_empty() { self.active_pane } else { self.add_pane() };
                order.push(id);
                Member::Pane(id)
            }
        }
    }

    /// The panes for the control channel's `dump`: tabs, the shown file's
    /// cursor and text, and a point inside its text to click.
    pub(super) fn panes_summary(&self, cx: &App) -> serde_json::Value {
        let order = self.pane_group.panes();
        let panes: Vec<serde_json::Value> = order
            .iter()
            .filter_map(|id| self.pane(*id))
            .map(|pane| {
                let shown = pane.active_editor_item().map(|item| {
                    let state = item.editor.read(cx);
                    let position = state.cursor_position();
                    let geometry = state.gutter_geometry();
                    let line_height = state.line_height().map(f32::from).unwrap_or(16.);
                    let first_row = geometry.rows.first().map(|&(_, top, _)| f32::from(top) + line_height / 2.);
                    serde_json::json!({
                        "path": item.path,
                        "line": position.line + 1,
                        "column": position.character + 1,
                        "selection": state.selected_text().to_string(),
                        "text": (state.text().len() < 8000).then(|| state.text().to_string()),
                        "can_undo": state.can_undo(),
                        "plist": item.shown_plist().is_some(),
                        "linked_views": state.linked_views().len(),
                        "vim": state.vim_status().map(|s| s.mode.label()),
                        "click": first_row.map(|y| [f32::from(geometry.numbers.right()) + 60., y]),
                        // The editor's left edge, just right of the pane's divider.
                        "left": f32::from(geometry.numbers.left()),
                    })
                });
                serde_json::json!({
                    "tabs": pane.items.iter().map(|item| item.title(cx)).collect::<Vec<_>>(),
                    "active": pane.active_item().map(|item| item.title(cx)),
                    "focused": pane.id == self.active_pane,
                    "assistant": pane.assistant.map(|a| format!("{:?}", a.mode)),
                    "assists": pane.assistant.and_then(|a| order.iter().position(|p| *p == a.of)),
                    "shown": shown,
                    "history": { "back": pane.back.len(), "forward": pane.forward.len() },
                })
            })
            .collect();
        let area = self.editor_area_size.get();
        serde_json::json!({
            "list": panes,
            "layout": layout_string(&self.pane_group.root, &order),
            "focused": order.iter().position(|p| *p == self.active_pane),
            "zoomed": self.zoomed,
            // The editor area's width and height at the last frame.
            "area": [f32::from(area.width), f32::from(area.height)],
            "sizes": match self.pane_layout(cx) {
                PaneLayout::Split { sizes, .. } => sizes,
                PaneLayout::Pane(_) => vec![],
            },
        })
    }

    /// Reopens a pane's saved previews among its tabs.
    fn restore_pane_previews(&mut self, pane: PaneId, saved: &[SavedPreview], window: &mut Window, cx: &mut Context<Self>) {
        let mut saved: Vec<&SavedPreview> = saved.iter().filter(|p| p.path.is_file()).collect();
        saved.sort_by_key(|p| p.position);
        for preview in saved {
            let Some(added) = self.add_rendered_item(pane, &preview.path, window, cx) else { continue };
            let Some(p) = self.pane_mut(pane) else { return };
            let item = p.items.remove(added);
            let ix = preview.position.min(p.items.len());
            p.items.insert(ix, item);
            if preview.active {
                p.active = ix;
            } else if p.active >= ix && p.items.len() > 1 {
                p.active += 1;
            }
        }
    }

    /// Don't Save on closing a file's last editor: a buffer still open (a
    /// preview keeps it) goes back to the saved text.
    pub(super) fn discard_changes(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.buffer(path).is_some_and(|b| b.dirty) {
            return;
        }
        self.reload_from_disk(path, cx);
        if let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) {
            buffer.dirty = false;
            self.lsp.update(cx, |lsp, cx| lsp.set_unsaved(path, false, cx));
        }
        cx.notify();
    }

    /// The pane at `index` in layout order.
    pub fn pane_at(&self, index: usize) -> Option<PaneId> {
        self.pane_group.panes().get(index).copied()
    }

    /// The control channel's `pane` command: `open-next <path>[:line]` (what
    /// ⌥-click in the navigator does), `focus <n>` (a pane by its place in
    /// layout order), `tab <n> <i>` (click a tab), `close-tab <n> <i>`.
    pub fn pane_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
        let numbers: Vec<usize> = arg.split_whitespace().filter_map(|n| n.parse().ok()).collect();
        let pane = |this: &Self| numbers.first().and_then(|&n| this.pane_at(n)).ok_or(format!("usage: pane {verb} <pane index> …"));
        match verb {
            "open-next" => {
                let (path, line) = match arg.rsplit_once(':') {
                    Some((path, line)) if line.parse::<u32>().is_ok() => (path, line.parse::<u32>().ok()),
                    _ => (arg, None),
                };
                let target = line.map(|line| Target::Line { line: line.saturating_sub(1), column: 0 });
                self.open_in_next_pane(PathBuf::from(path), target, window, cx);
            }
            "focus" => {
                let pane = pane(self)?;
                self.focus_pane(pane, window, cx);
            }
            "tab" | "close-tab" => {
                let pane = pane(self)?;
                let ix = *numbers.get(1).ok_or(format!("usage: pane {verb} <pane index> <tab index>"))?;
                if verb == "tab" {
                    self.activate_item(pane, ix, window, cx);
                } else {
                    self.close_item(pane, ix, window, cx);
                }
            }
            _ => return Err(format!("unknown pane command “{verb}”")),
        }
        Ok(())
    }
}

/// A pane's previews, where they are among its tabs.
fn saved_previews(items: &[Item], active: usize) -> Vec<SavedPreview> {
    items
        .iter()
        .enumerate()
        .filter_map(|(ix, item)| Some(SavedPreview { position: ix, path: item.rendered_path()?.to_path_buf(), active: ix == active }))
        .collect()
}

/// The split tree in one line, panes by their place in layout order:
/// `h(0, v(1, 2))`.
fn layout_string(member: &Member, order: &[PaneId]) -> String {
    match member {
        Member::Pane(id) => order.iter().position(|p| p == id).map_or("?".into(), |ix| ix.to_string()),
        Member::Split { axis, members } => {
            let inner: Vec<String> = members.iter().map(|m| layout_string(m, order)).collect();
            format!("{}({})", if *axis == SplitAxis::Horizontal { "h" } else { "v" }, inner.join(", "))
        }
    }
}

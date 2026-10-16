//! Open files: a buffer per file, shared by every pane showing it, and the
//! editor each pane shows it in.
//!
//! A buffer owns what belongs to the file: whether it's dirty, the text it
//! was saved with, the language server's document, and the breakpoints and
//! bookmarks anchored in it. Each pane's view is an editor linked to the
//! buffer's others (`zcode_editor::EditorState::link`): one text, one undo
//! history, one anchor table, a cursor and scroll position each. One of the
//! views is the buffer's `editor`, whose changes the workspace follows; when
//! its tab closes another view takes over.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gpui_kit::component::input::InputEvent;
use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;
use zcode_editor::{EditorState, RangeDecoration, RangeDecorationStyle, RopeExt as _, TabSize};

use super::pane::{EditorItem, Item, PaneId, PlistView};
use super::{PlistEditor, PlistEditorEvent, Target, Workspace, line_anchors};
use crate::lsp::{self, DocumentProviders};

pub(super) const MAX_OPEN_FILE_SIZE: u64 = 16 * 1024 * 1024;

pub struct Buffer {
    pub path: PathBuf,
    /// One of the buffer's views, the one whose changes the workspace
    /// follows. Every view has the same text, but cursors and selections
    /// are each pane's own: commands read `Workspace::active_editor`.
    pub editor: Entity<EditorState>,
    pub dirty: bool,
    /// A package's checked-out source: shown, never edited or saved.
    pub read_only: bool,
    /// Text as last loaded/saved, so undoing back to it clears `dirty`.
    pub(super) saved_text: zcode_editor::Rope,
    pub language: SharedString,
    pub(super) mtime: Option<SystemTime>,
    pub(super) breakpoint_anchors: line_anchors::LineAnchors<crate::debugger::Breakpoint>,
    pub(super) bookmark_anchors: line_anchors::LineAnchors<(crate::bookmarks::Bookmark, crate::bookmarks::Slot)>,
    /// Property lists: saved back in the format they were read in (each
    /// pane shows them in a property list editor of its own).
    pub plist_format: Option<crate::plist::Format>,
    pub(super) _subscriptions: Vec<Subscription>,
}

impl Buffer {
    pub fn title(&self) -> String {
        super::pane::file_title(&self.path)
    }
}

impl Workspace {
    pub fn buffer(&self, path: &Path) -> Option<&Buffer> {
        self.buffers.iter().find(|b| b.path == path)
    }

    /// The focused pane's file.
    pub fn active_buffer(&self) -> Option<&Buffer> {
        self.buffer(self.active_item()?.path.as_path())
    }

    /// The focused pane's text editor, unless its tab shows a property list.
    pub fn active_editor(&self) -> Option<Entity<EditorState>> {
        self.active_item().filter(|item| item.shown_plist().is_none()).map(|item| item.editor.clone())
    }

    /// Every pane's view of `path`.
    pub(super) fn views_of(&self, path: &Path) -> Vec<&EditorItem> {
        self.panes.iter().flat_map(|pane| pane.items.iter().filter_map(Item::editor)).filter(|item| item.path == path).collect()
    }

    /// The path of the buffer `editor` (an entity id, any pane's view)
    /// edits; it follows renames, which a path captured when the editor
    /// opened wouldn't.
    pub(super) fn buffer_path_of(&self, editor: EntityId) -> Option<PathBuf> {
        self.panes
            .iter()
            .flat_map(|pane| pane.items.iter().filter_map(Item::editor))
            .find(|item| item.editor.entity_id() == editor)
            .map(|item| item.path.clone())
    }

    /// The active editor's gutter items of `lane` as painted last frame:
    /// 1-based line and bounds.
    pub(super) fn painted_gutter(&self, lane: &str, cx: &App) -> Vec<(u32, Bounds<Pixels>)> {
        let Some(editor) = self.active_editor() else { return vec![] };
        editor
            .read(cx)
            .painted_gutter_items()
            .iter()
            .filter(|item| item.lane.as_ref() == lane)
            .map(|item| (item.row + 1, item.bounds))
            .collect()
    }

    /// The gutter's lanes: breakpoint markers, bookmark ribbons over them,
    /// test diamonds over both, each taking its clicks first.
    fn install_gutter_lanes(&self, editor: &Entity<EditorState>, cx: &mut Context<Self>) {
        let lanes = [self.breakpoint_lane(editor, cx), self.bookmark_lane(editor, cx), self.test_lane(editor, cx)];
        editor.update(cx, |state, cx| {
            for lane in lanes {
                state.set_gutter_lane(lane, cx);
            }
        });
    }

    fn language_for(path: &Path) -> &'static str {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match (name, ext) {
            ("Podfile" | "Fastfile" | "Gemfile", _) => "ruby",
            ("Makefile", _) => "make",
            (_, "swift" | "swiftinterface") => "swift",
            (_, "m" | "h" | "c") => "c",
            (_, "mm" | "cc" | "cpp" | "cxx" | "hpp" | "hh") => "cpp",
            (_, "json" | "xcscheme" | "resolved") if ext != "xcscheme" => "json",
            (_, "plist" | "entitlements" | "xcscheme" | "xml" | "storyboard" | "xib" | "html") => {
                "html"
            }
            (_, "md" | "markdown") => "markdown",
            (_, "yml" | "yaml") => "yaml",
            (_, "toml") => "toml",
            (_, "sh" | "bash" | "zsh") => "bash",
            (_, "rs") => "rust",
            (_, "js" | "mjs" | "ts") => "javascript",
            (_, "py") => "python",
            _ => "text",
        }
    }

    fn new_editor(soft_wrap: bool, language: &'static str, window: &mut Window, cx: &mut Context<EditorState>) -> EditorState {
        EditorState::new(window, cx)
            .language(language)
            .line_number(true)
            .indent_guides(true)
            .tab_size(TabSize {
                tab_size: 4,
                hard_tabs: false,
            })
            .soft_wrap(soft_wrap)
    }

    /// Reads a file into a new buffer, with no pane showing it yet. Tells
    /// the user and returns `false` when it can't be opened.
    pub(super) fn open_buffer(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if size > MAX_OPEN_FILE_SIZE {
            self.notify_error(format!("{} is too large to open", path.display()), window, cx);
            return false;
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.notify_error(format!("Couldn’t open {}: {err}", path.display()), window, cx);
                return false;
            }
        };
        // Property lists open in the property list editor; binary ones are
        // edited as XML and saved as binary again.
        let plist_format = if crate::plist::is_property_list(path) || crate::plist::is_binary(&bytes) {
            match crate::plist::load(bytes.clone()) {
                Ok((format, _)) => Some(format),
                Err(err) if crate::plist::is_binary(&bytes) => {
                    self.notify_error(format!("Couldn’t open {}: {err:#}", path.display()), window, cx);
                    return false;
                }
                Err(_) => None,
            }
        } else {
            None
        };
        let text = match plist_format {
            Some(crate::plist::Format::Binary) => crate::plist::load(bytes).map(|(_, text)| text).unwrap_or_default(),
            _ => match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => {
                    self.notify_error(format!("{} is not a UTF-8 text file", path.display()), window, cx);
                    return false;
                }
            },
        };
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let language = Self::language_for(path);
        let read_only = crate::packages::location::is_package_checkout(path);
        let soft_wrap = self.soft_wrap;
        let editor = cx.new(|cx| Self::new_editor(soft_wrap, language, window, cx).default_value(text));
        if let Some(language_id) = lsp::language_id_for(path) {
            self.lsp.update(cx, |store, cx| {
                store.open_document(path.to_path_buf(), language_id, editor.read(cx).text().clone(), cx)
            });
        }
        let saved_text = editor.read(cx).text().clone();
        let subscription = self.follow_buffer_changes(&editor, path.to_path_buf(), cx);
        self.buffers.push(Buffer {
            path: path.to_path_buf(),
            saved_text,
            editor,
            dirty: false,
            read_only,
            language: language.into(),
            mtime,
            breakpoint_anchors: Default::default(),
            bookmark_anchors: Default::default(),
            plist_format,
            _subscriptions: vec![subscription],
        });
        self.anchor_breakpoints(path, false, cx);
        self.anchor_bookmarks(path, false, cx);
        true
    }

    /// What the workspace does after each edit of a buffer, whichever pane
    /// made it: dirty state, the language server's copy, tests, and the
    /// lines breakpoints and bookmarks are on.
    fn follow_buffer_changes(&self, editor: &Entity<EditorState>, path: PathBuf, cx: &mut Context<Self>) -> Subscription {
        cx.subscribe(editor, move |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                if let Some(buffer) = this.buffers.iter_mut().find(|b| b.path == path) {
                    let text = buffer.editor.read(cx).text();
                    // Cheap length check first; full compare only when equal.
                    let dirty = text.len() != buffer.saved_text.len() || *text != buffer.saved_text;
                    if dirty != buffer.dirty {
                        buffer.dirty = dirty;
                        cx.notify();
                    }
                }
                let text = this.buffer(&path).map(|b| b.editor.read(cx).text().clone());
                if let Some(text) = text {
                    this.lsp.update(cx, |lsp, cx| lsp.document_changed(&path, text, cx));
                }
                this.schedule_test_reparse(&path, cx);
                this.track_breakpoints(&path, cx);
                this.track_bookmarks(&path, cx);
            }
        })
    }

    /// A tab for `path`, whose buffer is open: the buffer's own
    /// editor when no pane shows it yet, otherwise a new editor linked to it.
    pub(super) fn new_editor_item(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Option<EditorItem> {
        let buffer = self.buffer(path)?;
        let (shared, read_only, plist_format) = (buffer.editor.clone(), buffer.read_only, buffer.plist_format);
        let language = Self::language_for(path);
        let editor = if self.views_of(path).is_empty() {
            shared
        } else {
            let soft_wrap = self.soft_wrap;
            let editor = cx.new(|cx| Self::new_editor(soft_wrap, language, window, cx));
            EditorState::link(&shared, &editor, window, cx);
            editor
        };
        editor.update(cx, |state, cx| state.set_read_only(read_only, cx));
        self.install_gutter_lanes(&editor, cx);

        if lsp::language_id_for(path).is_some() {
            let apply_edit: crate::lsp::ApplyEditFn = {
                let workspace = cx.weak_entity();
                Rc::new(move |edit, window, cx| {
                    workspace
                        .update(cx, |this, cx| this.apply_workspace_edit(edit, window, cx))
                        .ok();
                })
            };
            DocumentProviders::new(self.lsp.downgrade(), path.to_path_buf(), apply_edit).install(&editor, cx);
            let workspace = cx.weak_entity();
            editor.update(cx, |state, _| {
                state.lsp_mut().show_document = Some(Rc::new(move |params, window, cx| {
                    let Some(path) = lsp::uri_to_path(&params.uri) else {
                        return false;
                    };
                    let target = params.selection.map(Target::LspRange);
                    // Called while the editor is being updated: navigate after.
                    let workspace = workspace.clone();
                    window.defer(cx, move |window, cx| {
                        workspace
                            .update(cx, |this, cx| this.open_file(path, target, true, window, cx))
                            .ok();
                    });
                    true
                }));
            });
        }
        // Variable values on hover while paused, in front of Quick Help.
        crate::debugger::hover::install(&editor, self.debugger.downgrade(), cx);

        let view = editor.entity_id();
        let focus = editor.read(cx).focus_handle(cx);
        let mut subscriptions = vec![
            cx.observe(&editor, |_, _, cx| cx.notify()),
            // Clicking into a pane, or tabbing into it, focuses the pane.
            cx.on_focus_in(&focus, window, move |this, window, cx| this.view_focused(view, window, cx)),
        ];
        let plist = plist_format.map(|_| {
            let schema = crate::plist::schema::Schema::for_file_name(&super::pane::file_title(path));
            let plist_editor = cx.new(|cx| PlistEditor::new(editor.clone(), schema, cx));
            subscriptions.push(cx.subscribe_in(&plist_editor, window, move |this, _, event: &PlistEditorEvent, window, cx| {
                if let PlistEditorEvent::OpenAsSource = event
                    && let Some((pane, ix)) = this.find_view(view)
                {
                    this.open_as(pane, ix, true, window, cx);
                }
            }));
            subscriptions.push(cx.observe(&plist_editor, |_, _, cx| cx.notify()));
            let focus = plist_editor.read(cx).focus_handle(cx);
            subscriptions.push(cx.on_focus_in(&focus, window, move |this, window, cx| this.view_focused(view, window, cx)));
            PlistView { editor: plist_editor, as_source: false }
        });
        Some(EditorItem { path: path.to_path_buf(), editor, plist, exec_mark: None, _subscriptions: subscriptions })
    }

    /// After a tab closed: the buffer closes with its last view, or hands
    /// its events to a view still open.
    pub(super) fn view_closed(&mut self, path: &Path, view: EntityId, cx: &mut Context<Self>) {
        let remaining = self.views_of(path).first().map(|item| item.editor.clone());
        let Some(ix) = self.buffers.iter().position(|b| b.path == path) else { return };
        match remaining {
            None => {
                let buffer = self.buffers.remove(ix);
                self.lsp.update(cx, |lsp, _| lsp.close_document(&buffer.path));
            }
            Some(editor) if self.buffers[ix].editor.entity_id() == view => {
                let subscription = self.follow_buffer_changes(&editor, path.to_path_buf(), cx);
                let buffer = &mut self.buffers[ix];
                buffer.editor = editor;
                buffer._subscriptions = vec![subscription];
            }
            Some(_) => {}
        }
    }

    pub(super) fn reload_from_disk(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else {
            return;
        };
        let text = match (buffer.plist_format.is_some(), std::fs::read(path)) {
            (true, Ok(bytes)) => match crate::plist::load(bytes) {
                Ok((format, text)) => {
                    buffer.plist_format = Some(format);
                    text
                }
                Err(_) => return,
            },
            (false, Ok(bytes)) => match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => return,
            },
            (_, Err(_)) => return,
        };
        buffer.mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        buffer.saved_text = zcode_editor::Rope::from(text.as_str());
        let editor = buffer.editor.clone();
        // Each pane's cursor stays where it was, as far as the text goes.
        let cursors: Vec<(Entity<EditorState>, usize)> =
            self.views_of(path).iter().map(|item| (item.editor.clone(), item.editor.read(cx).cursor())).collect();
        let Some(window) = cx.windows().first().copied() else { return };
        window
            .update(cx, |_, window, cx| {
                if *editor.read(cx).text() != text {
                    editor.update(cx, |state, cx| state.set_value(text, window, cx));
                    for (view, cursor) in cursors {
                        view.update(cx, |state, cx| {
                            let clamped = cursor.min(state.text().len());
                            state.set_selected_range(clamped..clamped, cx);
                        });
                    }
                }
            })
            .ok();
        let text = self.buffer(path).map(|b| b.editor.read(cx).text().clone());
        if let Some(text) = text {
            self.lsp.update(cx, |lsp, cx| lsp.document_changed(path, text, cx));
        }
        for item in self.views_of(path) {
            if let Some(plist) = &item.plist {
                plist.editor.update(cx, |editor, cx| editor.text_replaced(cx));
            }
        }
        // Replacing the text collapsed the anchors; breakpoints and bookmarks
        // keep their lines.
        self.anchor_breakpoints(path, true, cx);
        self.anchor_bookmarks(path, true, cx);
    }

    pub fn save_buffer(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.get_mut(ix).filter(|b| !b.read_only) else { return };
        let mut text = buffer.editor.read(cx).text().to_string();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        let format = buffer.plist_format.unwrap_or(crate::plist::Format::Xml);
        let bytes = match crate::plist::encode(&text, format) {
            Ok(bytes) => bytes,
            Err(err) => {
                let message = format!("Couldn’t save {}: {err:#}", buffer.path.display());
                self.notify_error(message, window, cx);
                return;
            }
        };
        match std::fs::write(&buffer.path, &bytes) {
            Ok(()) => {
                buffer.dirty = false;
                buffer.saved_text = buffer.editor.read(cx).text().clone();
                buffer.mtime = std::fs::metadata(&buffer.path).and_then(|m| m.modified()).ok();
                let path = buffer.path.clone();
                self.lsp.update(cx, |lsp, cx| lsp.did_save(&path, cx));
                if lsp::language_id_for(&path).is_some() {
                    self.schedule_index_refresh(cx);
                }
            }
            Err(err) => {
                let message = format!("Couldn’t save {}: {err}", buffer.path.display());
                self.notify_error(message, window, cx);
            }
        }
        window.set_window_edited(self.buffers.iter().any(|b| b.dirty));
        cx.notify();
    }

    pub(super) fn save_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.buffers.iter().position(|b| b.path == path) {
            self.save_buffer(ix, window, cx);
        }
    }

    /// Open As ▸ Source Code (`as_source`) or ▸ Property List for a tab.
    pub fn open_as(&mut self, pane: PaneId, ix: usize, as_source: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Item::Editor(item)) = self.pane_mut(pane).and_then(|p| p.items.get_mut(ix)) else { return };
        let Some(plist) = item.plist.as_mut() else { return };
        plist.as_source = as_source;
        self.activate_item(pane, ix, window, cx);
    }

    /// Opens a file and shows it as source code or as a property list.
    pub fn open_file_as(&mut self, path: PathBuf, as_source: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(path.clone(), None, true, window, cx);
        let path = path.canonicalize().unwrap_or(path);
        let pane = self.active_pane;
        if let Some(ix) = self.pane(pane).and_then(|p| p.position_of(&path)) {
            self.open_as(pane, ix, as_source, window, cx);
        }
    }

    /// Refreshes the index a moment after the last save of a source file.
    fn schedule_index_refresh(&mut self, cx: &mut Context<Self>) {
        self.index_refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            this.update(cx, |this, cx| {
                this.build.update(cx, |build, cx| build.refresh_index(cx));
            })
            .ok();
        }));
    }

    pub fn save_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for ix in 0..self.buffers.len() {
            if self.buffers[ix].dirty {
                self.save_buffer(ix, window, cx);
            }
        }
    }

    /// Byte range covering a whole 1-based line, newline included.
    fn line_range(text: &zcode_editor::Rope, line: u32) -> Option<std::ops::Range<usize>> {
        let row = line.checked_sub(1)? as usize;
        if row >= text.lines_len() {
            return None;
        }
        let start = text.line_start_offset(row);
        let end = (text.line_end_offset(row) + 1).min(text.len());
        (end > start).then_some(start..end)
    }

    /// Highlights the line execution stopped on (green, like Xcode), in
    /// every pane showing its file.
    pub(super) fn set_exec_mark(&mut self, location: Option<(PathBuf, u32)>, cx: &mut Context<Self>) {
        self.exec_location = location.clone();
        for pane in &mut self.panes {
            for item in pane.items.iter_mut() {
                let Item::Editor(item) = item else { continue };
                let decorations: Vec<RangeDecoration> = match &location {
                    Some((path, line)) if *path == item.path => {
                        let text = item.editor.read(cx).text().clone();
                        Self::line_range(&text, *line)
                            .map(|range| {
                                vec![RangeDecoration::new(range)
                                    .with_style(RangeDecorationStyle::Fill)
                                    .with_color(gpui_kit::hsla(0.33, 0.7, 0.45, 0.28))]
                            })
                            .unwrap_or_default()
                    }
                    _ => vec![],
                };
                match &item.exec_mark {
                    Some(mark) => mark.set(decorations, cx),
                    None if !decorations.is_empty() => {
                        let mark = item
                            .editor
                            .update(cx, |state, cx| state.create_range_decorations_collection(decorations, cx));
                        item.exec_mark = Some(mark);
                    }
                    None => {}
                }
            }
        }
    }

    // ---------------------------------------------------------------------
    // Workspace edits (rename, code actions)

    pub fn apply_workspace_edit(
        &mut self,
        edit: lsp_types::WorkspaceEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut per_file: Vec<(PathBuf, Vec<lsp_types::TextEdit>)> = vec![];
        if let Some(changes) = edit.changes {
            for (uri, edits) in changes {
                if let Some(path) = lsp::uri_to_path(&uri) {
                    per_file.push((path, edits));
                }
            }
        }
        if let Some(lsp_types::DocumentChanges::Edits(doc_edits)) = edit.document_changes {
            for doc in doc_edits {
                if let Some(path) = lsp::uri_to_path(&doc.text_document.uri) {
                    let edits = doc
                        .edits
                        .into_iter()
                        .map(|e| match e {
                            lsp_types::OneOf::Left(edit) => edit,
                            lsp_types::OneOf::Right(annotated) => annotated.text_edit,
                        })
                        .collect();
                    per_file.push((path, edits));
                }
            }
        }
        let mut touched = 0;
        let previously_active = (self.active_pane, self.pane(self.active_pane).map_or(0, |p| p.active));
        for (path, mut edits) in per_file {
            let path = path.canonicalize().unwrap_or(path);
            // Package checkouts are read-only.
            if crate::packages::location::is_package_checkout(&path) {
                continue;
            }
            // Apply back to front so earlier offsets stay valid.
            edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
            // Edits land in (possibly newly opened) tabs rather than straight
            // on disk: the language server sees the new text immediately and
            // the user can review the change before saving.
            if self.buffer(&path).is_none() && path.is_file() {
                self.open_file(path.clone(), None, false, window, cx);
            }
            if let Some(buffer) = self.buffer(&path) {
                buffer.editor.update(cx, |state, cx| {
                    for edit in &edits {
                        let text = state.text().clone();
                        let range = lsp::lsp_range_to_offsets(&text, edit.range);
                        state.set_selected_range(range, cx);
                        state.replace(edit.new_text.clone(), window, cx);
                    }
                });
            } else if let Ok(contents) = std::fs::read_to_string(&path) {
                let mut text = zcode_editor::Rope::from(contents.as_str());
                for edit in &edits {
                    let range = lsp::lsp_range_to_offsets(&text, edit.range);
                    let mut s = text.to_string();
                    s.replace_range(range, &edit.new_text);
                    text = zcode_editor::Rope::from(s.as_str());
                }
                std::fs::write(&path, text.to_string()).ok();
                self.lsp.read(cx).files_changed(std::slice::from_ref(&path));
            }
            touched += 1;
        }
        let (pane, ix) = previously_active;
        if self.pane(pane).is_some_and(|p| ix < p.items.len()) {
            self.activate_item(pane, ix, window, cx);
        }
        if touched > 1 {
            window.push_notification(
                Notification::info(format!("Updated {touched} files")),
                cx,
            );
        }
        cx.notify();
    }
}

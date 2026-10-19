//! Open files: a buffer per file, shared by every pane showing it, and the
//! editor each pane shows it in.
//!
//! A buffer owns what belongs to the file: whether it's dirty, the text it
//! was saved with, the language server's document, and the breakpoints and
//! bookmarks anchored in it. Each pane's view is an editor linked to the
//! buffer's others (`zcode_editor::EditorState::link`): one text, one undo
//! history, one anchor table, a cursor and scroll position each. One of the
//! views is the buffer's `editor`, whose changes the workspace follows; when
//! its tab closes another view takes over. A file a multibuffer shows that
//! no pane had open gets a headless buffer: an editor without a language,
//! and no language server document until it's edited or shown in a pane.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gpui_kit::component::input::InputEvent;
use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;
use zcode_editor::{EditorState, RangeDecoration, RangeDecorationStyle, RopeExt as _};

use super::pane::{EditorItem, Item, PaneId, PlistView};
use super::{PlistEditor, PlistEditorEvent, Target, Workspace, line_anchors};
use crate::lsp::{self, DocumentProviders};

pub(super) const MAX_OPEN_FILE_SIZE: u64 = 64 * 1024 * 1024;
/// Past this a file goes to no language server and gets no change bars
/// (the limit files could be opened at before they got no syntax tree).
pub(super) const MAX_LANGUAGE_FEATURES_SIZE: usize = 16 * 1024 * 1024;

/// The language servers' id for `path`'s language, unless its text, `len`
/// bytes, is too large to give them.
fn served_language(path: &Path, len: usize) -> Option<&'static str> {
    (len <= MAX_LANGUAGE_FEATURES_SIZE).then(|| lsp::language_id_for(path)).flatten()
}

pub struct Buffer {
    pub path: PathBuf,
    /// One of the buffer's views, the one whose changes the workspace
    /// follows. Every view has the same text, but cursors and selections
    /// are each pane's own: commands read `Workspace::active_editor`.
    pub editor: Entity<EditorState>,
    pub dirty: bool,
    /// A package's checked-out source: shown, never edited or saved.
    pub read_only: bool,
    /// `editor` isn't a pane's (a headless buffer's own, or a multibuffer's
    /// view): a pane showing the file makes a view of its own.
    pub headless: bool,
    /// Text as last loaded/saved, so undoing back to it clears `dirty`.
    pub(super) saved_text: zcode_editor::Rope,
    pub language: SharedString,
    pub(super) mtime: Option<SystemTime>,
    pub(super) breakpoint_anchors: line_anchors::LineAnchors<crate::debugger::Breakpoint>,
    pub(super) bookmark_anchors: line_anchors::LineAnchors<(crate::bookmarks::Bookmark, crate::bookmarks::Slot)>,
    /// Property lists: saved back in the format they were read in (each
    /// pane shows them in a property list editor of its own).
    pub plist_format: Option<crate::plist::Format>,
    /// The file against git: its versions, change hunks, blame.
    pub git: super::change_bars::BufferGit,
    /// The language server's semantic tokens for it.
    pub(super) tokens: super::semantic_tokens::BufferTokens,
    /// The SwiftUI previews it declares.
    pub(super) previews: super::previews::BufferPreviews,
    pub(super) _subscriptions: Vec<Subscription>,
}

/// An npm package's file (a definition in a library's `.d.ts`): shown,
/// never edited.
fn is_installed_package(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "node_modules")
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

    /// The focused pane's text editor, unless its tab shows a property
    /// list; a multibuffer's editor.
    pub fn active_editor(&self) -> Option<Entity<EditorState>> {
        match self.pane(self.active_pane)?.active_item()? {
            Item::Editor(item) => item.shown_plist().is_none().then(|| item.editor.clone()),
            Item::MultiBuffer(item) => Some(item.host.clone()),
            Item::Diff(_) | Item::Terminal(_) | Item::ProjectSettings(_) | Item::Organizer(_) | Item::AssetCatalog(_) => None,
        }
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
    /// test diamonds and preview marks over both, each taking its clicks
    /// first; change bars after the numbers, and the authors column when
    /// it's shown.
    fn install_gutter_lanes(&self, editor: &Entity<EditorState>, cx: &mut Context<Self>) {
        let mut lanes =
            vec![self.breakpoint_lane(editor, cx), self.bookmark_lane(editor, cx), self.test_lane(editor, cx), self.preview_lane(editor, cx), self.changes_lane(editor, cx)];
        if self.git.show_authors {
            lanes.push(self.authors_lane(editor, cx));
        }
        editor.update(cx, |state, cx| {
            for lane in lanes {
                state.set_gutter_lane(lane, cx);
            }
        });
    }

    pub(crate) fn language_for(path: &Path) -> &'static str {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match (name, ext) {
            ("Podfile" | "Fastfile" | "Gemfile", _) => "ruby",
            ("Makefile", _) => "make",
            _ if name.starts_with(".env") => "bash",
            (".prettierrc" | ".eslintrc" | ".babelrc" | ".swcrc", _) => "json",
            (_, "swift" | "swiftinterface") => "swift",
            (_, "m" | "h" | "c") => "c",
            (_, "mm" | "cc" | "cpp" | "cxx" | "hpp" | "hh") => "cpp",
            (_, "json" | "jsonc" | "json5" | "resolved") => "json",
            (_, "plist" | "entitlements" | "xcscheme" | "xml" | "storyboard" | "xib" | "html" | "htm" | "svg") => "html",
            (_, "md" | "markdown" | "mdx") => "markdown",
            (_, "yml" | "yaml") => "yaml",
            (_, "toml") => "toml",
            (_, "sh" | "bash" | "zsh") => "bash",
            (_, "rs") => "rust",
            (_, "js" | "mjs" | "cjs") => "javascript",
            (_, "jsx") => "jsx",
            (_, "ts" | "mts" | "cts") => "typescript",
            (_, "tsx") => "tsx",
            (_, "css" | "scss" | "pcss" | "postcss") => "css",
            (_, "less") => "less",
            (_, "vue") => "vue",
            (_, "svelte") => "svelte",
            (_, "astro") => "astro",
            (_, "py") => "python",
            _ => "text",
        }
    }

    /// An editor for a file of `language`, as the window's settings say.
    pub(super) fn new_editor(config: &super::settings::EditorConfig, language: &'static str, window: &mut Window, cx: &mut Context<EditorState>) -> EditorState {
        let mut state = EditorState::new(window, cx)
            .language(language)
            .line_number(config.line_numbers)
            .indent_guides(config.indent_guides)
            .tab_size(config.tab())
            .soft_wrap(config.soft_wrap);
        if config.show_whitespaces {
            state.set_show_whitespaces(true, window, cx);
        }
        state
    }

    /// Reads a file into a new buffer, with no pane showing it yet. Tells
    /// the user and returns `false` when it can't be opened.
    pub(super) fn open_buffer(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.open_buffer_with(path, false, window, cx)
    }

    /// `open_buffer`, headless for a multibuffer: no highlighting, language
    /// server document or semantic tokens until a pane shows it or it's
    /// edited (it is still compared with git, and keeps its breakpoints).
    pub(super) fn open_buffer_with(&mut self, path: &Path, headless: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
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
        // Package sources, the SDK's interfaces, and the files zcode shows
        // (Open Default Settings) can't be edited.
        let read_only = crate::packages::location::is_package_checkout(path)
            || crate::toolchain::is_in_developer_dir(path)
            || is_installed_package(path)
            || super::settings::is_generated(path);
        let config = self.editor_config();
        let served = served_language(path, text.len());
        let editor = if headless {
            cx.new(|cx| EditorState::new(window, cx).default_value(text))
        } else {
            cx.new(|cx| Self::new_editor(&config, language, window, cx).default_value(text))
        };
        if !headless && let Some(language_id) = served {
            let (text, journal) = editor.update(cx, |state, _| (state.text().clone(), state.edit_journal()));
            self.lsp.update(cx, |store, cx| store.open_document(path.to_path_buf(), language_id, text, Some(journal), cx));
        }
        let saved_text = editor.read(cx).text().clone();
        let subscription = self.follow_buffer_changes(&editor, path.to_path_buf(), cx);
        self.buffers.push(Buffer {
            path: path.to_path_buf(),
            saved_text,
            editor,
            dirty: false,
            read_only,
            headless,
            language: language.into(),
            mtime,
            breakpoint_anchors: Default::default(),
            bookmark_anchors: Default::default(),
            plist_format,
            git: Default::default(),
            tokens: Default::default(),
            previews: Default::default(),
            _subscriptions: vec![subscription],
        });
        self.follow_open_files();
        if !headless {
            self.schedule_semantic_tokens(path, Duration::ZERO, cx);
        }
        self.anchor_breakpoints(path, false, cx);
        self.anchor_bookmarks(path, false, cx);
        self.scan_previews(path, Duration::ZERO, cx);
        // A multibuffer reads a page of files' bases at once.
        if !read_only && !headless {
            self.load_git_bases(vec![path.to_path_buf()], cx);
        }
        true
    }

    /// Opens the language server's document of a headless buffer's file.
    fn ensure_lsp_document(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(editor) = self.buffer(path).map(|b| b.editor.clone()) else { return };
        let Some(language_id) = served_language(path, editor.read(cx).text().len()) else { return };
        if self.lsp.read(cx).is_open(path) {
            return;
        }
        let (text, journal) = editor.update(cx, |state, _| (state.text().clone(), state.edit_journal()));
        self.lsp.update(cx, |store, cx| store.open_document(path.to_path_buf(), language_id, text, Some(journal), cx));
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
                // A headless buffer's file gets its document once it's edited.
                this.ensure_lsp_document(&path, cx);
                let text = this.buffer(&path).map(|b| b.editor.read(cx).text().clone());
                if let Some(text) = text {
                    this.lsp.update(cx, |lsp, cx| lsp.document_changed(&path, text, cx));
                }
                this.schedule_test_reparse(&path, cx);
                this.previews_edited(&path, cx);
                this.track_breakpoints(&path, cx);
                this.track_bookmarks(&path, cx);
                this.buffer_edited_for_git(&path, cx);
                this.semantic_tokens_edited(&path, cx);
                this.inlay_hints_edited(&path, cx);
                this.predictions_edited(&path, cx);
            }
        })
    }

    /// A tab for `path`, whose buffer is open: the buffer's own
    /// editor when no pane shows it yet, otherwise a new editor linked to it.
    pub(super) fn new_editor_item(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Option<EditorItem> {
        let buffer = self.buffer(path)?;
        let (shared, read_only, plist_format, headless) = (buffer.editor.clone(), buffer.read_only, buffer.plist_format, buffer.headless);
        let language = Self::language_for(path);
        if headless {
            self.ensure_lsp_document(path, cx);
        }
        let editor = if self.views_of(path).is_empty() && !headless {
            shared
        } else {
            let config = self.editor_config();
            let editor = cx.new(|cx| Self::new_editor(&config, language, window, cx));
            EditorState::link(&shared, &editor, window, cx);
            editor
        };
        editor.update(cx, |state, cx| state.set_read_only(read_only, cx));
        self.install_gutter_lanes(&editor, cx);

        if served_language(path, editor.read(cx).text().len()).is_some() {
            let apply_edit: crate::lsp::ApplyEditFn = {
                let workspace = cx.weak_entity();
                Rc::new(move |edit, window, cx| {
                    workspace
                        .update(cx, |this, cx| this.apply_workspace_edit(edit, window, cx))
                        .ok();
                })
            };
            let emmet = crate::languages::emmet::syntax_language(language).map(|language| {
                let options = crate::languages::emmet::options(self.settings.current.tab_size, self.settings.current.hard_tabs);
                Rc::new(crate::languages::emmet::EmmetCompletions { language, options }) as Rc<dyn crate::lsp::LocalCompletions>
            });
            DocumentProviders::new(self.lsp.downgrade(), path.to_path_buf(), apply_edit).with_local(emmet).install(&editor, cx);
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
        // A view linked to an open buffer starts without its tokens.
        if !self.views_of(path).is_empty() || headless {
            self.schedule_semantic_tokens(path, Duration::ZERO, cx);
        }
        let mut subscriptions = vec![
            // Inlay hints are asked for the rows each view shows.
            cx.subscribe(&editor, |this, editor, event: &zcode_editor::ViewportChanged, cx| {
                this.inlay_hints_viewport_changed(&editor, &event.rows, cx);
            }),
            cx.observe(&editor, |this, editor, cx| {
                // Inline blame follows the focused editor's cursor.
                this.refresh_inline_blame(cx);
                this.predictions_editor_changed(&editor, cx);
                cx.notify();
            }),
            // Clicking into a pane, or tabbing into it, focuses the pane.
            cx.on_focus_in(&focus, window, move |this, window, cx| this.view_focused(view, window, cx)),
        ];
        subscriptions.extend(self.install_vim(&editor, window, cx));
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
        self.inlay_hints_view_closed(view);
        let pane_view = self.views_of(path).first().map(|item| item.editor.clone());
        let headless = pane_view.is_none();
        let remaining = pane_view
            .or_else(|| self.diff_views_of(path).first().map(|diff| diff.read(cx).current.clone()))
            .or_else(|| self.multibuffer_views_of(path, cx).into_iter().next());
        let Some(ix) = self.buffers.iter().position(|b| b.path == path) else { return };
        match remaining {
            None => {
                let buffer = self.buffers.remove(ix);
                self.lsp.update(cx, |lsp, cx| lsp.close_document(&buffer.path, cx));
            }
            Some(editor) if self.buffers[ix].editor.entity_id() == view => {
                let subscription = self.follow_buffer_changes(&editor, path.to_path_buf(), cx);
                let buffer = &mut self.buffers[ix];
                buffer.editor = editor;
                buffer.headless = headless;
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
        // A reload isn't an edit event: the change bars and blame follow here.
        self.buffer_edited_for_git(path, cx);
        self.file_changed_on_disk(cx);
    }

    pub fn save_buffer(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (trim, final_newline) = (self.settings.current.remove_trailing_whitespace_on_save, self.settings.current.ensure_final_newline_on_save);
        let Some(buffer) = self.buffers.get_mut(ix).filter(|b| !b.read_only) else { return };
        if trim {
            // In the editor, so ⌘Z brings the spaces back.
            let ranges = trailing_whitespace(&buffer.editor.read(cx).text().to_string());
            if !ranges.is_empty() {
                buffer.editor.update(cx, |state, cx| state.edit(ranges.into_iter().map(|r| (r, "")), window, cx));
            }
        }
        let mut text = buffer.editor.read(cx).text().to_string();
        if final_newline && !text.is_empty() && !text.ends_with('\n') {
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
                // Build settings changed: sourcekit-lsp gets them.
                if path.extension().is_some_and(|e| e == "xcconfig")
                    && let Some(build_server) = &self.build_server
                {
                    build_server.update(cx, |server, cx| server.project_changed(cx));
                }
                self.file_changed_on_disk(cx);
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

    /// How `path` is formatted, as `formatter` says: Prettier's output as
    /// line edits, or its language server's `textDocument/formatting`
    /// (Biome's, when the web app uses Biome). Both in the server's
    /// coordinates. `None` when nothing formats it.
    pub(super) fn document_formatting(&mut self, path: &Path, cx: &mut Context<Self>) -> Option<Task<anyhow::Result<Vec<lsp_types::TextEdit>>>> {
        let text = self.buffer(path)?.editor.read(cx).text().clone();
        let prettier = match self.settings.current.formatter {
            crate::settings::Formatter::LanguageServer => None,
            crate::settings::Formatter::Prettier => Some(lsp::prettier::find(path, &self.project.root)),
            crate::settings::Formatter::Auto => lsp::prettier::find(path, &self.project.root).filter(|_| lsp::prettier::supports(path)).map(Some),
        };
        if let Some(prettier) = prettier {
            let path = path.to_path_buf();
            return Some(cx.background_spawn(async move {
                let prettier = anyhow::Context::context(prettier, "Prettier isn’t installed in this web app (npm install --save-dev prettier)")?;
                let before = text.to_string();
                let after = lsp::prettier::format(&prettier, &path, &before)?;
                Ok(lsp::prettier::line_edits(&before, &after))
            }));
        }
        let servers = self.lsp.update(cx, |lsp, _| lsp.synced_servers(path, Some(&text), Some("/documentFormattingProvider")));
        let (_, server) = servers.iter().find(|(_, s)| s.name() == "biome").or(servers.first())?.clone();
        let request = server.request_raw(
            "textDocument/formatting",
            serde_json::json!({ "textDocument": { "uri": lsp::path_to_uri(path).as_str() }, "options": self.formatting_options() }),
        );
        Some(cx.background_spawn(async move { Ok(serde_json::from_value(request.await?).unwrap_or_default()) }))
    }

    /// ⌘S, Save All and `:w`: with `format_on_save`, the file is formatted
    /// first (waiting at most a moment); otherwise as `save_path`. Saves
    /// before a build don't format.
    pub(super) fn save_path_from_user(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let request = if self.settings.current.format_on_save && self.buffer(path).is_some_and(|b| !b.read_only && b.plist_format.is_none()) {
            self.document_formatting(path, cx)
        } else {
            None
        };
        let Some(request) = request else {
            self.save_path(path, window, cx);
            return;
        };
        let Some(before) = self.buffer(path).map(|b| b.editor.read(cx).text().to_string()) else { return };
        let path = path.to_path_buf();
        cx.spawn_in(window, async move |this, cx| {
            let timeout = cx.background_executor().timer(super::settings::FORMAT_ON_SAVE_TIMEOUT);
            let result = match futures::future::select(Box::pin(request), Box::pin(timeout)).await {
                futures::future::Either::Left((result, _)) => result.ok(),
                futures::future::Either::Right(_) => None,
            };
            this.update_in(cx, |this, window, cx| {
                let unchanged = this.buffer(&path).is_some_and(|b| b.editor.read(cx).text().to_string() == before);
                let edits: Vec<lsp_types::TextEdit> = result.unwrap_or_default();
                if unchanged && !edits.is_empty() {
                    this.apply_file_edits(vec![(path.clone(), edits)], window, cx);
                }
                this.save_path(&path, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Save All from the user (`save_path_from_user` for each file).
    pub(super) fn save_all_from_user(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dirty: Vec<PathBuf> = self.buffers.iter().filter(|b| b.dirty).map(|b| b.path.clone()).collect();
        for path in dirty {
            self.save_path_from_user(&path, window, cx);
        }
    }

    /// The editor's indentation, for the language server's formatting.
    pub(super) fn formatting_options(&self) -> serde_json::Value {
        serde_json::json!({ "tabSize": self.settings.current.tab_size, "insertSpaces": !self.settings.current.hard_tabs })
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
        self.apply_file_edits(workspace_edit_files(edit), window, cx);
    }

    /// Applies each file's edits: in its buffer (opened as a tab if it
    /// wasn't), or on disk for a file that can't be.
    pub(super) fn apply_file_edits(
        &mut self,
        per_file: Vec<(PathBuf, Vec<lsp_types::TextEdit>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                self.lsp.update(cx, |lsp, cx| lsp.files_changed(std::slice::from_ref(&path), cx));
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

/// A workspace edit's text edits, by file.
pub(super) fn workspace_edit_files(edit: lsp_types::WorkspaceEdit) -> Vec<(PathBuf, Vec<lsp_types::TextEdit>)> {
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
    per_file
}

/// The spaces and tabs at the ends of lines, as byte ranges.
fn trailing_whitespace(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = vec![];
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        let trimmed = content.trim_end_matches([' ', '\t']);
        if trimmed.len() < content.len() {
            ranges.push(start + trimmed.len()..start + content.len());
        }
        start += line.len();
    }
    ranges
}

#[cfg(test)]
mod tests {
    #[test]
    fn trailing_whitespace_on_each_line() {
        let text = "let a = 1  \n\t\nok\nend \t";
        let ranges = super::trailing_whitespace(text);
        assert_eq!(ranges, [9..11, 12..13, 20..22]);
        assert!(super::trailing_whitespace("clean\n").is_empty());
    }
}

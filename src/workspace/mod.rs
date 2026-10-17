//! The project window: navigator, editor panes, inspector, debug area,
//! toolbar, status.

mod add_files;
mod add_package;
mod blame;
mod bookmark_dialog;
mod bookmarks;
mod breakpoint_editor;
mod branch_picker;
mod breakpoints;
mod buffers;
mod change_bars;
mod commit_sheet;
mod console;
mod diff_view;
mod edit_scheme;
mod editing;
mod inspector;
mod editor_probe;
mod events;
mod find;
mod hierarchy;
mod hunk_popover;
mod inlay_hints;
mod issues;
mod line_anchors;
mod multibuffer;
mod navigation;
mod navigator;
mod packages;
mod pane;
mod pane_group;
mod pane_view;
mod panels;
mod panes;
mod pickers;
mod plist_editor;
mod project_files;
mod project_settings;
mod remote_sheet;
mod semantic_tokens;
mod source_control;
mod test_gutter;
mod testing;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::tree::{TreeItem, TreeState};
use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;

use crate::actions::*;
use crate::build::BuildController;
use crate::debugger::Debugger;
use crate::lsp::LspStore;
use crate::project::{self, ProjectInfo};
use crate::search::SearchOptions;
use crate::session::{ProjectSession, Session};

pub use buffers::Buffer;
pub use find::FindState;
pub use navigator::NavigatorTab;
pub use multibuffer::{MultiBufferKind, MultiBufferView};
pub use pane::{AssistantMode, DiffItem, EditorItem, Item, MultiBufferItem, Pane, PaneId};
pub use remote_sheet::RemoteAction;
pub use pane_group::SplitAxis;
pub use panels::DebugTab;
pub use plist_editor::{PlistEditor, PlistEditorEvent};
pub use project_settings::{ProjectSettings, ProjectSettingsEvent};

const DEFAULT_FONT_SIZE: f32 = 13.;

/// A position to reveal after opening a file.
#[derive(Clone, Debug)]
pub enum Target {
    /// 0-based line and UTF-16 column (LSP / compiler coordinates).
    Line { line: u32, column: u32 },
    LspRange(lsp_types::Range),
    Offset(usize),
}

struct Modal {
    view: AnyView,
    /// Where a popover points (window coordinates); sheets are centered.
    position: Option<Point<Pixels>>,
    _subscription: Subscription,
}

pub struct Workspace {
    focus_handle: FocusHandle,
    pub project: ProjectInfo,
    /// zcode's cache folder for the project (index, logs).
    cache_dir: PathBuf,
    pub files: Arc<Vec<PathBuf>>,
    pub tree: Entity<TreeState>,
    /// The navigator's file items; Package Dependencies follow them.
    file_items: Vec<TreeItem>,
    packages: packages::PackagesState,
    pub lsp: Entity<LspStore>,
    pub build: Entity<BuildController>,
    pub debugger: Entity<Debugger>,
    /// Open files, one buffer each however many panes show them.
    pub buffers: Vec<Buffer>,
    /// The editor area: panes of tabs, how they're split, the focused one.
    panes: Vec<Pane>,
    pane_group: pane_group::PaneGroup,
    active_pane: PaneId,
    next_pane_id: u64,
    /// Focus Editor Pane: only the focused pane is shown.
    zoomed: bool,
    /// Each split's pane sizes as dragged, and as the session had them.
    split_states: std::collections::HashMap<SharedString, Entity<gpui_kit::component::resizable::ResizableState>>,
    restored_split_sizes: std::collections::HashMap<SharedString, Vec<f32>>,
    /// The project editor, in whichever pane has its tab.
    pub project_settings: Option<Entity<ProjectSettings>>,
    project_settings_subscriptions: Vec<Subscription>,
    /// Where execution stopped, marked in every pane showing it.
    exec_location: Option<(PathBuf, u32)>,
    xcode_projects: Vec<PathBuf>,
    /// Parsed projects for the File inspector.
    project_index: project_files::ProjectIndex,
    /// Serializes `project.pbxproj` edits.
    project_edit_lock: Arc<parking_lot::Mutex<()>>,
    add_files_dialog: Option<WeakEntity<add_files::AddFilesDialog>>,
    edit_scheme_dialog: Option<WeakEntity<edit_scheme::EditSchemeDialog>>,
    breakpoint_editor: Option<WeakEntity<breakpoint_editor::BreakpointEditor>>,
    breakpoint_gutter: Rc<breakpoints::BreakpointGutter>,
    popup_menu: Option<breakpoints::PopupMenuState>,
    pub bookmarks: crate::bookmarks::Bookmarks,
    bookmarks_ui: bookmarks::BookmarksUi,
    console: console::Console,
    pub show_inspector: bool,
    /// What the File inspector shows when it isn't the active file (a
    /// folder picked in the navigator).
    inspected: Option<PathBuf>,
    inspector_info: Option<inspector::InspectorInfo>,
    pub navigator: NavigatorTab,
    pub show_navigator: bool,
    pub show_debug_area: bool,
    pub debug_tab: DebugTab,
    pub raw_log: bool,
    pub find: FindState,
    /// Find Call Hierarchy and the type hierarchies, in the Find navigator.
    pub hierarchy: hierarchy::HierarchyState,
    pub inlay_hints: inlay_hints::InlayHints,
    /// Whether the language server was ready last time it said anything,
    /// so its features are asked for again once it is (again).
    lsp_ready: bool,
    pub outline: Vec<pickers::OutlineEntry>,
    outline_path: Option<PathBuf>,
    /// The repository, its status, and git's state in the editors.
    pub git: source_control::GitState,
    pub font_size: f32,
    pub soft_wrap: bool,
    memory_label: String,
    modal: Option<Modal>,
    issue_cursor: Option<usize>,
    debug_scroll: UniformListScrollHandle,
    restore_destination: Option<String>,
    pub outline_generation: u64,
    session: Session,
    pub tests: testing::TestsState,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
    outline_task: Option<Task<()>>,
    pub(super) index_refresh: Option<Task<()>>,
}

impl Workspace {
    pub fn new(
        project: ProjectInfo,
        opened_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let cache_dir = crate::paths::project_cache_dir(&project.root);
        let lsp_args = Self::lsp_args(&project, &cache_dir);
        let lsp = cx.new(|_| LspStore::new(project.root.clone(), lsp_args));
        let t0 = std::time::Instant::now();
        let trace = std::env::var_os("ZCODE_TRACE_STARTUP").is_some();
        let mark = move |label: &str| {
            if trace {
                eprintln!("[workspace] {label}: {:?}", t0.elapsed());
            }
        };
        let build = cx.new(|cx| BuildController::new(project.clone(), cache_dir.clone(), cx));
        mark("build controller");
        let tree = cx.new(|cx| TreeState::new(cx));
        let debugger = cx.new(|_| Debugger::default());
        let find_input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in Workspace"));

        mark("find input");
        let mut session = Session::load();
        session.note_recent(&opened_path);
        let saved = session.project(&project.root);
        session.save();

        let mut subscriptions = vec![
            cx.subscribe_in(&lsp, window, Self::on_lsp_event),
            cx.subscribe_in(&build, window, Self::on_build_event),
            cx.subscribe_in(&debugger, window, Self::on_debugger_event),
            cx.subscribe_in(&find_input, window, |this, _, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.run_find(window, cx);
                }
            }),
        ];
        // Devices are listed while the window is active, and at once when it
        // becomes active (one may have been plugged in meanwhile).
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            this.build.update(cx, |build, cx| build.set_window_active(active, cx));
            // A commit or checkout may have happened elsewhere meanwhile.
            if active && this.git.root.is_some() {
                this.refresh_scm(cx);
            }
        }));
        // Semantic token colors come from the theme, which follows the appearance.
        subscriptions.push(cx.observe_window_appearance(window, |this, _, cx| this.refresh_semantic_tokens(cx)));
        // Only redraw when the displayed memory figure changes.
        subscriptions.push(cx.observe_global::<crate::perf::Perf>(|this, cx| {
            let shown = crate::perf::Perf::global(cx).status_label();
            if this.memory_label != shown {
                this.memory_label = shown;
                cx.notify();
            }
        }));

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            project: project.clone(),
            cache_dir: cache_dir.clone(),
            files: Arc::new(vec![]),
            tree,
            file_items: vec![],
            packages: Default::default(),
            lsp,
            build,
            debugger,
            buffers: vec![],
            panes: vec![Pane::new(PaneId(0))],
            pane_group: pane_group::PaneGroup::new(PaneId(0)),
            active_pane: PaneId(0),
            next_pane_id: 1,
            zoomed: false,
            split_states: Default::default(),
            restored_split_sizes: Default::default(),
            project_settings: None,
            project_settings_subscriptions: vec![],
            exec_location: None,
            xcode_projects: vec![],
            project_index: Default::default(),
            project_edit_lock: Default::default(),
            add_files_dialog: None,
            edit_scheme_dialog: None,
            breakpoint_editor: None,
            breakpoint_gutter: Default::default(),
            popup_menu: None,
            bookmarks: Default::default(),
            bookmarks_ui: Default::default(),
            console: Default::default(),
            show_inspector: saved.show_inspector.unwrap_or(false),
            inspected: None,
            inspector_info: None,
            navigator: NavigatorTab::Project,
            show_navigator: saved.show_navigator.unwrap_or(true),
            show_debug_area: saved.show_debug_area.unwrap_or(false),
            debug_tab: DebugTab::Console,
            raw_log: false,
            find: FindState {
                input: find_input,
                replace_input: None,
                replacing: false,
                options: SearchOptions::default(),
                results: Arc::new(vec![]),
                searching: false,
                error: None,
                title: None,
                cancel: Arc::new(AtomicBool::new(false)),
                _task: None,
            },
            hierarchy: Default::default(),
            inlay_hints: inlay_hints::InlayHints::new(session.inlay_hints.unwrap_or(false)),
            lsp_ready: false,
            outline: vec![],
            outline_path: None,
            git: source_control::GitState::new(&session),
            font_size: session.font_size.unwrap_or(DEFAULT_FONT_SIZE),
            soft_wrap: false,
            memory_label: String::new(),
            modal: None,
            issue_cursor: None,
            debug_scroll: UniformListScrollHandle::new(),
            restore_destination: None,
            outline_generation: 0,
            session,
            tests: Default::default(),
            _subscriptions: subscriptions,
            _tasks: vec![],
            outline_task: None,
            index_refresh: None,
        };

        // Restore build selection.
        if saved.scheme.is_some() || saved.configuration.is_some() {
            this.build.update(cx, |build, _| {
                if let Some(c) = saved.configuration.clone() {
                    build.configuration = c;
                }
                build.scheme = saved.scheme.clone();
            });
        }
        if saved.test_plan.is_some() {
            this.build.update(cx, |build, _| build.test_plan = saved.test_plan.clone());
        }
        if !saved.scheme_options.is_empty() {
            this.build.update(cx, |build, _| build.stored_options = saved.scheme_options.clone());
        }
        this.restore_destination = saved.destination.clone();
        this.debugger.update(cx, |d, _| {
            let breakpoints = saved
                .all_breakpoints()
                .into_iter()
                .filter(|bp| bp.path().is_none_or(Path::exists))
                .collect();
            d.restore_breakpoints(breakpoints);
            d.breakpoints_active = saved.breakpoints_active.unwrap_or(true);
        });
        this.bookmarks.restore(saved.bookmarks.clone());

        mark("state");
        this.rescan(cx);
        this.refresh_scm(cx);
        this.start_activity_ticker(cx);
        this.watch_files(cx);
        this.prune_missing_bookmarks(cx);
        mark("background tasks");
        this.restore_tabs(saved, opened_path, window, cx);
        mark("restore tabs");
        if this.buffers.is_empty() {
            // Keep shortcuts working before any file is open.
            this.focus_handle.focus(window, cx);
        }
        this
    }

    fn lsp_args(project: &ProjectInfo, cache_dir: &Path) -> Vec<String> {
        let db = crate::build::compiledb::database_path(cache_dir);
        if db.exists() {
            let rel = crate::paths::relative_path(&project.root, cache_dir);
            return vec![
                "--compilation-db-search-path".into(),
                rel.display().to_string(),
            ];
        }
        vec![]
    }

    pub fn title(&self) -> String {
        if self.showing_project_settings() {
            return format!("{} — Project Settings", self.project.name);
        }
        match self.active_item() {
            Some(item) => format!("{} — {}", self.project.name, item.title()),
            None => self.project.name.clone(),
        }
    }

    // ---------------------------------------------------------------------
    // Files and the project tree

    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        let root = self.project.root.clone();
        let kind = self.project.kind.clone();
        let task = cx.spawn(async move |this, cx| {
            let scan_root = root.clone();
            let (files, xcode_projects) = cx
                .background_spawn(async move { (project::scan_files(&scan_root), project::xcode_projects(&kind)) })
                .await;
            this.update(cx, |this, cx| {
                // Tree items are !Send, so they are built here (fast).
                this.file_items = project::build_tree(&root, &files);
                this.files = Arc::new(files);
                if this.xcode_projects != xcode_projects {
                    this.xcode_projects = xcode_projects;
                    if this.show_inspector {
                        this.refresh_project_index(cx);
                    }
                }
                this.update_tree(cx);
                this.refresh_packages(cx);
                this.refresh_tests(cx);
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(task);
    }

    /// Polls the project for external changes (git checkout, generators,
    /// another editor) every few seconds: cheap, and avoids FSEvents setup
    /// cost at startup.
    fn watch_files(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let Ok(paths) = this.update(cx, |this, _| {
                    this.buffers
                        .iter()
                        .map(|b| (b.path.clone(), b.mtime, b.dirty))
                        .collect::<Vec<_>>()
                }) else {
                    break;
                };
                let changed: Vec<PathBuf> = cx
                    .background_spawn(async move {
                        paths
                            .into_iter()
                            .filter(|(path, mtime, dirty)| {
                                !dirty
                                    && std::fs::metadata(path)
                                        .and_then(|m| m.modified())
                                        .ok()
                                        .is_some_and(|m| Some(m) != *mtime)
                            })
                            .map(|(path, _, _)| path)
                            .collect()
                    })
                    .await;
                if !changed.is_empty() {
                    let ok = this.update(cx, |this, cx| {
                        for path in &changed {
                            this.reload_from_disk(path, cx);
                        }
                    });
                    if ok.is_err() {
                        break;
                    }
                }
            }
        });
        self._tasks.push(task);
    }

    /// Re-renders once a second while a build or run is in progress so the
    /// activity view's elapsed time stays current.
    fn start_activity_ticker(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, cx| {
                    if this.build.read(cx).phase.is_busy() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        self._tasks.push(task);
    }

    pub(super) fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self
            .active_item()
            .and_then(|b| b.path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.project.root.clone());
        self.new_file_in(dir, window, cx);
    }

    /// Moves a file or folder to the user's Trash (Finder-style naming on
    /// collision) and closes any tabs inside it.
    pub fn move_to_trash(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = path.canonicalize().unwrap_or(path);
        let trash = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".Trash"))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut target = trash.join(&name);
        let mut n = 2;
        while target.exists() {
            target = trash.join(format!("{name} {n}"));
            n += 1;
        }
        if let Err(err) = std::fs::rename(&path, &target) {
            self.notify_error(format!("Couldn’t move {name} to the Trash: {err}"), window, cx);
            return;
        }
        let trashed: Vec<PathBuf> = self.buffers.iter().map(|b| b.path.clone()).filter(|p| p.starts_with(&path)).collect();
        for file in trashed {
            self.close_file_everywhere(&file, window, cx);
        }
        self.debugger.update(cx, |d, cx| d.remove_breakpoints_in(&path, cx));
        if self.bookmarks.remove_in(&path) {
            self.bookmarks_changed(true, cx);
        }
        self.rescan(cx);
        self.refresh_scm(cx);
        self.remove_from_projects(path, window, cx);
    }

    /// Renames a file or folder, keeping open tabs pointed at it. A name
    /// with `/` in it moves the item, relative to its folder (as in Zed).
    pub fn rename_path(&mut self, path: PathBuf, new_name: String, window: &mut Window, cx: &mut Context<Self>) {
        let path = path.canonicalize().unwrap_or(path);
        let Some(parent) = path.parent() else { return };
        let target = crate::xcodeproj::files::normalize(&parent.join(&new_name));
        if target.exists() {
            self.notify_error(format!("“{new_name}” already exists."), window, cx);
            return;
        }
        if target.starts_with(&path) {
            self.notify_error(format!("Can’t move “{}” into itself.", path.display()), window, cx);
            return;
        }
        if let Some(folder) = target.parent()
            && let Err(err) = std::fs::create_dir_all(folder)
        {
            self.notify_error(format!("Couldn’t create {}: {err}", folder.display()), window, cx);
            return;
        }
        // Save edits first so nothing is lost when tabs reopen.
        let affected: Vec<usize> = (0..self.buffers.len())
            .filter(|&ix| self.buffers[ix].path.starts_with(&path))
            .collect();
        for &ix in &affected {
            if self.buffers[ix].dirty {
                self.save_buffer(ix, window, cx);
            }
        }
        if let Err(err) = std::fs::rename(&path, &target) {
            self.notify_error(format!("Couldn’t rename: {err}"), window, cx);
            return;
        }
        self.debugger.update(cx, |d, cx| d.move_files(&path, &target, cx));
        if self.bookmarks.move_files(&path, &target) {
            self.save_session(cx);
        }
        // Tabs reopen at the new path, in the panes they were in.
        let focused = self.active_pane;
        let mut reopen = vec![];
        for pane in &self.panes {
            for item in pane.items.iter().filter_map(Item::editor).filter(|item| item.path.starts_with(&path)) {
                let rel = item.path.strip_prefix(&path).unwrap_or(Path::new(""));
                reopen.push((pane.id, if rel.as_os_str().is_empty() { target.clone() } else { target.join(rel) }));
            }
        }
        let moved: Vec<PathBuf> = self.buffers.iter().map(|b| b.path.clone()).filter(|p| p.starts_with(&path)).collect();
        for file in moved {
            self.close_file_everywhere(&file, window, cx);
        }
        for (pane, new_path) in reopen {
            self.open_file_in(pane, new_path, None, false, window, cx);
        }
        self.focus_pane(focused, window, cx);
        self.rescan(cx);
        self.refresh_scm(cx);
        self.move_in_projects(path, target, window, cx);
    }

    pub(super) fn prompt_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let delegate = pickers::RenameFileDelegate::new(cx.weak_entity(), path.clone());
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let picker = cx.new(|cx| crate::picker::Picker::with_query(delegate, &name, window, cx));
        self.show_modal(picker, window, cx);
    }

    pub(super) fn new_file_in(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_new_path(&dir, Some("Untitled.swift"));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = receiver.await else { return };
            this.update_in(cx, |this, window, cx| this.create_file(path, window, cx)).ok();
        })
        .detach();
    }

    /// Creates a file from the New File panel (from a template, unless it
    /// exists), adds it to the Xcode project and opens it.
    pub fn create_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !path.exists() {
            let template = match path.extension().and_then(|e| e.to_str()) {
                Some("swift") => format!("//\n//  {}\n//\n\nimport Foundation\n\n", path.file_name().unwrap_or_default().to_string_lossy()),
                _ => String::new(),
            };
            if let Err(err) = std::fs::write(&path, template) {
                self.notify_error(format!("Couldn’t create {}: {err}", path.display()), window, cx);
                return;
            }
        }
        let path = path.canonicalize().unwrap_or(path);
        self.rescan(cx);
        self.open_file(path.clone(), None, true, window, cx);
        self.add_new_file_to_projects(path, window, cx);
    }

    pub fn save_session(&mut self, cx: &App) {
        let build = self.build.read(cx);
        let layout = self.pane_layout(cx);
        let mut open_files: Vec<PathBuf> = vec![];
        for pane in layout.panes() {
            open_files.extend(pane.files.iter().filter(|f| !open_files.contains(f)).cloned().collect::<Vec<_>>());
        }
        let state = ProjectSession {
            open_files,
            active_file: self.active_item().map(|item| item.path.clone()),
            panes: Some(layout),
            scheme: build.scheme.clone(),
            destination: build.destination.as_ref().map(|d| d.id.clone()),
            configuration: Some(build.configuration.clone()),
            test_plan: build.test_plan.clone(),
            scheme_options: build.stored_options.clone(),
            show_navigator: Some(self.show_navigator),
            show_debug_area: Some(self.show_debug_area),
            show_inspector: Some(self.show_inspector),
            breakpoints: Default::default(),
            breakpoints_v2: self.debugger.read(cx).breakpoints.clone(),
            breakpoints_active: Some(self.debugger.read(cx).breakpoints_active),
            bookmarks: self.bookmarks.entries.clone(),
        };
        self.session = Session::load();
        self.session.set_project(&self.project.root, state);
        self.session.font_size = Some(self.font_size);
        self.session.inline_blame = Some(self.git.inline_blame);
        self.session.show_authors = Some(self.git.show_authors);
        self.session.side_by_side_comparison = Some(self.git.side_by_side);
        self.session.inlay_hints = Some(self.inlay_hints.enabled);
        self.session.save();
    }

    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.project.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    // ---------------------------------------------------------------------
    // Project settings

    /// The `.xcodeproj` bundles the project editor shows, found during the
    /// background scan.
    pub(super) fn xcode_projects(&self) -> &[PathBuf] {
        &self.xcode_projects
    }

    // ---------------------------------------------------------------------
    // Modals

    pub fn show_modal<V>(&mut self, view: Entity<V>, window: &mut Window, cx: &mut Context<Self>)
    where
        V: Render + EventEmitter<DismissEvent> + Focusable,
    {
        self.show_modal_at(view, None, window, cx);
    }

    /// Shows a popover pointing at `position`, or a centered sheet.
    pub fn show_modal_at<V>(&mut self, view: Entity<V>, position: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>)
    where
        V: Render + EventEmitter<DismissEvent> + Focusable,
    {
        let subscription = cx.subscribe_in(&view, window, |this, _, _: &DismissEvent, window, cx| {
            this.dismiss_modal(window, cx);
        });
        view.read(cx).focus_handle(cx).focus(window, cx);
        self.modal = Some(Modal {
            view: view.into(),
            position,
            _subscription: subscription,
        });
        cx.notify();
    }

    pub fn dismiss_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The scheme editor keeps its changes however it closes.
        if let Some(dialog) = self.edit_scheme_dialog.take().and_then(|d| d.upgrade()) {
            let options = dialog.read(cx).edited(cx);
            self.save_edited_scheme(options, window, cx);
        }
        // So does the breakpoint editor.
        if let Some(editor) = self.breakpoint_editor.take().and_then(|e| e.upgrade()) {
            self.apply_breakpoint_edit(&editor, cx);
        }
        self.bookmarks_ui.dialog = None;
        // The commit message waits for the next Commit….
        if let Some(sheet) = self.git.commit_sheet.take().and_then(|s| s.upgrade()) {
            self.git.commit_draft = sheet.read(cx).draft(cx);
        }
        if self.modal.take().is_some() {
            match self.active_editor() {
                Some(editor) => editor.update(cx, |state, cx| state.focus(window, cx)),
                None => self.focus_handle.focus(window, cx),
            }
            cx.notify();
        }
    }

    pub fn notify_error(&self, message: String, window: &mut Window, cx: &mut App) {
        log::warn!("{message}");
        window.push_notification(Notification::error(message), cx);
    }

}

impl Workspace {
    /// A JSON summary of UI state for the debug harness and tests.
    /// A debug-channel command for the property list editor on screen: the
    /// active tab's, or the project settings' Info tab.
    pub fn plist_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let editor = if self.showing_project_settings() {
            self.project_settings.as_ref().and_then(|s| s.read(cx).info_editor()).ok_or("the Info tab isn’t shown")?
        } else {
            self.active_item().and_then(|item| item.shown_plist().cloned()).ok_or("no property list editor shown")?
        };
        editor.update(cx, |editor, cx| editor.command(command, window, cx))
    }

    pub fn debug_state(&mut self, cx: &App) -> serde_json::Value {
        let testing = self.tests_summary(cx);
        let build = self.build.read(cx);
        let active = self.active_item().zip(self.active_buffer()).map(|(item, b)| {
            let state = item.editor.read(cx);
            let pos = state.cursor_position();
            serde_json::json!({
                "path": b.path,
                "dirty": b.dirty,
                "read_only": b.read_only,
                "editor_read_only": state.is_read_only(),
                "can_undo": state.can_undo(),
                "probe": self.editor_probe_state(cx),
                "line": pos.line + 1,
                "column": pos.character + 1,
                "selection": state.selected_text().to_string(),
                "text": (state.text().len() < 8000).then(|| state.text().to_string()),
                "line_text": zcode_editor::RopeExt::slice_line(state.text(), pos.line as usize).to_string(),
                "diagnostics": state.diagnostics().map(|d| d.len()).unwrap_or(0),
                "completion_menu_open": state.completion_menu_state().open,
                "completion_items": state.completion_menu_state().items.iter().take(5).map(|i| i.label.clone()).collect::<Vec<_>>(),
                "plist": item.plist.as_ref().map(|p| {
                    let mut summary = p.editor.read(cx).summary(cx);
                    summary["format"] = format!("{:?}", b.plist_format.unwrap_or(crate::plist::Format::Xml)).into();
                    summary["as_source"] = p.as_source.into();
                    summary
                }),
            })
        });
        let issues = self.all_issues(cx);
        let perf = crate::perf::Perf::global(cx);
        let memory = serde_json::json!({
            "samples": perf.history.len(),
            "footprint_mb": crate::perf::mb(perf.current().footprint),
            "baseline_footprint_mb": crate::perf::mb(perf.baseline_footprint()),
            "language_server_mb": perf.language_server.map(|(_, s)| crate::perf::mb(s.footprint)),
            "status_label": perf.status_label(),
            "warning": perf.memory_warning(),
        });
        let mut state = serde_json::json!({
            "project": self.project.name,
            "kind": self.project.kind.label(),
            "files": self.files.len(),
            "buffers": self.buffers.iter().map(|b| b.title()).collect::<Vec<_>>(),
            "active": active,
            "modal_open": self.modal.is_some(),
            "xcode_projects": self.xcode_projects,
            "inspector": self.inspector_summary(),
            "add_files": self.add_files_dialog.as_ref().and_then(|d| d.upgrade()).map(|d| d.read(cx).summary()),
            "project_settings": self.project_settings.as_ref().map(|s| {
                let mut summary = s.read(cx).summary(cx);
                summary["shown"] = self.showing_project_settings().into();
                summary
            }),
            "navigator": format!("{:?}", self.navigator),
            "show_navigator": self.show_navigator,
            "show_debug_area": self.show_debug_area,
            "lsp": format!("{:?}", self.lsp.read(cx).status()),
            "lsp_progress": self.lsp.read(cx).progress_message(),
            "scheme": build.scheme,
            "configuration": build.configuration,
            "destination": build.destination.as_ref().map(|d| format!("{} ({})", d.name, d.runtime)),
            "destinations": build.destinations.len(),
            // The menu as shown: section, item and the picker's detail.
            "destination_menu": build.destinations.iter().map(|d| format!("{} | {} | {}", d.group(), d.menu_label(), d.detail())).collect::<Vec<_>>(),
            "phase": format!("{:?}", build.phase),
            "indexing": build.indexing,
            "issues": issues.iter().map(|i| format!("{:?} {}:{} {}", i.severity, i.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), i.line.unwrap_or(0), i.message)).collect::<Vec<_>>(),
            "tests": build.tests.iter().map(|t| format!("{:?} {}", t.status, t.full_name())).collect::<Vec<_>>(),
            "testing": testing,
            "console_tail": build.console.iter().rev().take(40).rev().cloned().collect::<Vec<_>>(),
            "log_lines": build.log.len(),
            "reports": build.reports.len(),
            "find_results": self.find.results.iter().map(|f| f.matches.len()).sum::<usize>(),
            "find_replacing": self.find.replacing && self.find.replace_input.is_some(),
            "outline": self.outline.iter().map(|o| o.name.clone()).collect::<Vec<_>>(),
            "scm_branch": self.git.status.branch,
            "scm_changes": self.git.status.files.len(),
            "font_size": self.font_size,
            "memory": memory,
            "history": self.pane(self.active_pane).map(|p| serde_json::json!({ "back": p.back.len(), "forward": p.forward.len() })),
            "debugger": {
                "state": format!("{:?}", self.debugger.read(cx).state),
                "frames": self.debugger.read(cx).frames.iter().take(4).map(|f| format!("{} {}:{}", f.name, f.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), f.line)).collect::<Vec<_>>(),
                "variables": self.debugger.read(cx).variables.iter().take(8).map(|v| format!("{} = {}", v.name, v.value)).collect::<Vec<_>>(),
            },
        });
        // Added separately: one `json!` this size reaches the macro recursion limit.
        state["breakpoints"] = self.breakpoints_summary(cx);
        state["bookmarks"] = self.bookmarks_summary(cx);
        state["packages"] = self.packages_summary(cx);
        state["console"] = self.console_summary(cx);
        state["panes"] = self.panes_summary(cx);
        state["git"] = self.git_summary(cx);
        state["multibuffer"] = self.multibuffer_summary(cx);
        state["language_features"] = serde_json::json!({
            "server": self.lsp.read(cx).features().summary(),
            "inlay_hints": self.inlay_hints_summary(cx),
            "semantic_tokens": self.semantic_tokens_summary(cx),
            "signature_help": self.signature_help_summary(cx),
            "hierarchy": self.hierarchy_summary(),
        });
        state["last_log"] = serde_json::json!(build.reports.first().map(|r| r.log_path.clone()));
        state["scheme_options"] = serde_json::json!(build.scheme_options.as_ref().map(edit_scheme::options_summary));
        state["edit_scheme"] =
            serde_json::json!(self.edit_scheme_dialog.as_ref().and_then(|d| d.upgrade()).map(|d| d.read(cx).summary(cx)));
        state
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

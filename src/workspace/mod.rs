//! The project window: navigator, editor panes, inspector, debug area,
//! toolbar, status.

mod add_files;
mod add_package;
mod agent;
mod archive;
mod asset_catalog;
mod asset_catalogs;
mod blame;
mod bookmark_dialog;
mod bookmarks;
mod breakpoint_editor;
mod branch_picker;
mod breakpoints;
mod buffers;
mod build_server;
mod change_bars;
mod commit_sheet;
mod console;
mod diff_view;
mod distribute_sheet;
mod edit_scheme;
mod editing;
mod inspector;
mod editor_probe;
mod events;
mod find;
mod hierarchy;
mod hunk_popover;
mod image_view;
mod inlay_hints;
mod issues;
mod line_anchors;
mod mcp;
mod multibuffer;
mod navigation;
mod navigator;
mod organizer;
mod packages;
mod pane;
mod pane_group;
mod pane_view;
mod panels;
mod panes;
mod pickers;
mod plist_editor;
mod predictions;
mod previews;
mod profile;
mod project_files;
mod project_settings;
mod remote_sheet;
mod rendered_view;
mod semantic_tokens;
mod settings;
mod source_control;
mod terminals;
mod vim;
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

pub use agent::McpPermission;
pub use buffers::Buffer;
pub use mcp::issue_json;
pub use organizer::archive_json;
pub use find::FindState;
pub use navigator::NavigatorTab;
pub use multibuffer::{MultiBufferKind, MultiBufferView};
pub use pane::{AssistantMode, DiffItem, EditorItem, Item, MultiBufferItem, Pane, PaneId, TerminalItem};
pub use remote_sheet::RemoteAction;
pub use pane_group::SplitAxis;
pub use panels::DebugTab;
pub use plist_editor::{PlistEditor, PlistEditorEvent};
pub use project_settings::{ProjectSettings, ProjectSettingsEvent};
pub use settings::{is_generated, register_global_settings_actions};

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
    /// The project's window.
    pub window: AnyWindowHandle,
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
    /// sourcekit-lsp's build settings for an Xcode project (`crate::bsp`).
    pub build_server: Option<Entity<crate::bsp::model::BuildServer>>,
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
    /// The files the panes showed at the last render, as the language
    /// servers were told (`note_shown_documents`).
    lsp_shown: Vec<PathBuf>,
    /// Each split's pane sizes as dragged, and as the session had them.
    split_states: std::collections::HashMap<SharedString, Entity<gpui_kit::component::resizable::ResizableState>>,
    restored_split_sizes: std::collections::HashMap<SharedString, Vec<f32>>,
    /// The project editor, in whichever pane has its tab.
    pub project_settings: Option<Entity<ProjectSettings>>,
    project_settings_subscriptions: Vec<Subscription>,
    /// Window ▸ Organizer's tab, while open.
    pub organizer: Option<Entity<organizer::Organizer>>,
    organizer_subscriptions: Vec<Subscription>,
    /// Distribute App's sheet, while open.
    distribute_sheet: Option<WeakEntity<distribute_sheet::DistributeSheet>>,
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
    /// The debug area's terminals.
    terminals: terminals::TerminalsState,
    /// The agent panel (⌘0), made when first shown.
    agent: agent::AgentState,
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
    /// Predictive code completion: requests, recent edits, the indicator.
    predictions: predictions::Predictions,
    /// Product ▸ Profile is waiting for Instruments' templates to show its
    /// chooser (whether it then builds).
    profile_chooser: Option<bool>,
    pub outline: Vec<pickers::OutlineEntry>,
    outline_path: Option<PathBuf>,
    /// The repository, its status, and git's state in the editors.
    pub git: source_control::GitState,
    pub font_size: f32,
    pub soft_wrap: bool,
    /// The settings in effect here: the user's, then the project's.
    settings: settings::SettingsState,
    memory_label: String,
    modal: Option<Modal>,
    issue_cursor: Option<usize>,
    debug_scroll: UniformListScrollHandle,
    restore_destination: Option<String>,
    pub outline_generation: u64,
    session: Session,
    pub tests: testing::TestsState,
    /// The last Show in Xcode Canvas and how it went.
    canvas_request: Option<previews::CanvasRequest>,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
    /// Re-renders once a second while a build or run is in progress.
    activity_ticker: Option<Task<()>>,
    /// FSEvents watches the project's tree whole (`fs_watch`).
    _fs_root: crate::fs_watch::Root,
    /// The open files, as `watch_files` follows them (`follow_open_files`).
    open_file_changes: Rc<crate::fs_watch::Changes>,
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
        let lsp = cx.new(|_| LspStore::new(project.root.clone(), Default::default(), Default::default()));
        let t0 = std::time::Instant::now();
        let trace = std::env::var_os("ZCODE_TRACE_STARTUP").is_some();
        let mark = move |label: &str| {
            if trace {
                eprintln!("[workspace] {label}: {:?}", t0.elapsed());
            }
        };
        let build = cx.new(|cx| BuildController::new(project.clone(), cache_dir.clone(), cx));
        // Idle until sourcekit-lsp connects to it.
        let build_server = project
            .kind
            .is_xcode()
            .then(|| cx.new(|cx| crate::bsp::model::BuildServer::new(project.clone(), cache_dir.clone(), &build, cx)));
        mark("build controller");
        let tree = cx.new(|cx| TreeState::new(cx));
        let debugger = cx.new(|_| Debugger::default());
        let find_input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in Workspace"));

        mark("find input");
        let mut session = Session::load();
        session.note_recent(&opened_path);
        let saved = session.project(&project.root);
        session.save();
        crate::settings::open_project(&project.root, cx);
        let current_settings = crate::settings::Settings::for_project(&project.root, cx);
        let launch = Self::sourcekit_launch(&project, &cache_dir, &current_settings);
        lsp.update(cx, |lsp, cx| {
            lsp.set_config(crate::lsp::LspConfig::from_settings(&current_settings), cx);
            lsp.set_sourcekit(launch, cx);
        });

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
        // New build settings reach sourcekit-lsp's open files.
        if let Some(server) = &build_server {
            subscriptions.push(cx.subscribe(server, |this, _, _: &crate::bsp::model::SettingsChanged, cx| {
                this.lsp.update(cx, |lsp, _| lsp.build_settings_changed());
            }));
        }
        // Devices are listed while the window is active, and at once when it
        // becomes active (one may have been plugged in meanwhile).
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            this.build.update(cx, |build, cx| build.set_window_active(active, cx));
            // The menus' checkmarks follow the window in front.
            if active {
                crate::actions::set_menu_settings(this.settings.current.clone(), cx);
            }
            // A commit or checkout may have happened elsewhere meanwhile.
            if active && this.git.root.is_some() {
                this.refresh_scm(cx);
            }
        }));
        // A settings, keymap or theme file changed, or a toggle wrote one.
        subscriptions.push(cx.observe_global_in::<crate::settings::SettingsStore>(window, |this, window, cx| this.settings_changed(window, cx)));
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
            window: window.window_handle(),
            project: project.clone(),
            cache_dir: cache_dir.clone(),
            files: Arc::new(vec![]),
            tree,
            file_items: vec![],
            packages: Default::default(),
            lsp,
            build,
            build_server,
            debugger,
            buffers: vec![],
            panes: vec![Pane::new(PaneId(0))],
            pane_group: pane_group::PaneGroup::new(PaneId(0)),
            active_pane: PaneId(0),
            next_pane_id: 1,
            zoomed: false,
            lsp_shown: vec![],
            split_states: Default::default(),
            restored_split_sizes: Default::default(),
            project_settings: None,
            project_settings_subscriptions: vec![],
            organizer: None,
            organizer_subscriptions: vec![],
            distribute_sheet: None,
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
            terminals: Default::default(),
            agent: Default::default(),
            show_inspector: saved.show_inspector.unwrap_or(false),
            inspected: None,
            inspector_info: None,
            navigator: NavigatorTab::Project,
            show_navigator: saved.show_navigator.unwrap_or(true),
            show_debug_area: saved.show_debug_area.unwrap_or(false),
            debug_tab: if saved.terminal_tab && !saved.terminals.is_empty() { DebugTab::Terminal } else { DebugTab::Console },
            raw_log: false,
            find: FindState {
                input: find_input,
                replace_input: None,
                replacing: false,
                options: SearchOptions::default(),
                results: Arc::new(vec![]),
                rows: vec![],
                scroll: UniformListScrollHandle::new(),
                searching: false,
                error: None,
                title: None,
                cancel: Arc::new(AtomicBool::new(false)),
                _task: None,
            },
            hierarchy: Default::default(),
            inlay_hints: inlay_hints::InlayHints::new(current_settings.inlay_hints),
            predictions: predictions::Predictions::new(&current_settings.edit_predictions),
            canvas_request: None,
            profile_chooser: None,
            outline: vec![],
            outline_path: None,
            git: source_control::GitState::new(&current_settings),
            font_size: current_settings.buffer_font_size,
            soft_wrap: current_settings.soft_wrap,
            settings: settings::SettingsState::new(current_settings),
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
            activity_ticker: None,
            _fs_root: crate::fs_watch::Root::new(&project.root),
            open_file_changes: Rc::new(crate::fs_watch::Changes::new(vec![])),
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
        this.restore_panel_terminals(&saved.terminals, window, cx);
        this.restore_agent_panel(saved.agent_panel.clone(), saved.show_agent_panel, window, cx);

        mark("state");
        this.rescan(cx);
        this.refresh_scm(cx);
        this.follow_activity(cx);
        this.watch_files(cx);
        this.prune_missing_bookmarks(cx);
        mark("background tasks");
        this.restore_tabs(saved, opened_path, window, cx);
        mark("restore tabs");
        if this.panes.iter().all(|p| p.items.is_empty()) {
            // Keep shortcuts working before any file is open.
            this.focus_handle.focus(window, cx);
        }
        this
    }

    /// How sourcekit-lsp is started: through zcode's build server for an
    /// Xcode project, unless the settings say otherwise or nothing listens
    /// for `zcode bsp`; otherwise with the index build's compilation
    /// database, once there is one.
    pub(super) fn sourcekit_launch(project: &ProjectInfo, cache_dir: &Path, settings: &crate::settings::Settings) -> crate::bsp::Launch {
        if project.kind.is_xcode()
            && settings.sourcekit_build_settings == crate::settings::SourcekitBuildSettings::BuildServer
            && let Some(socket) = crate::instance::listening()
        {
            return crate::bsp::Launch { args: vec![], build_server: Some((socket.to_path_buf(), cache_dir.to_path_buf())) };
        }
        crate::bsp::Launch { args: crate::bsp::database_args(&project.root, cache_dir), build_server: None }
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
            let (files, xcode_projects, scanned) = cx
                .background_spawn(async move {
                    let started = std::time::Instant::now();
                    (project::scan_files(&scan_root), project::xcode_projects(&kind), started.elapsed())
                })
                .await;
            this.update(cx, |this, cx| {
                // Tree items are !Send, so they are built here (fast).
                let started = std::time::Instant::now();
                this.file_items = project::build_tree(&root, &files);
                crate::perf::Perf::record_timing(cx, "scan", scanned, files.len());
                crate::perf::Perf::record_timing(cx, "tree", started.elapsed(), this.file_items.len());
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

    /// Follows the open files for changes made outside zcode (git checkout,
    /// generators, another editor, an agent): their times are looked at when
    /// FSEvents says one may have changed (`fs_watch`, which starts after the
    /// window does), else every few seconds.
    fn watch_files(&mut self, cx: &mut Context<Self>) {
        let changes = self.open_file_changes.clone();
        let task = cx.spawn(async move |this, cx| {
            loop {
                changes.wait(Duration::from_secs(3), cx.background_executor()).await;
                let Ok(paths) = this.update(cx, |this, _| {
                    this.buffers
                        .iter()
                        .map(|b| (b.path.clone(), b.mtime, b.dirty))
                        .collect::<Vec<_>>()
                }) else {
                    break;
                };
                changes.set_paths(paths.iter().map(|(path, _, _)| path.clone()).collect());
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

    /// Follows the open files now (a file just opened): the loop in
    /// `watch_files` sets them when it wakes, which, once FSEvents covers
    /// the files open before, is only when one of those changes or every
    /// 30 s.
    pub(super) fn follow_open_files(&self) {
        self.open_file_changes.set_paths(self.buffers.iter().map(|b| b.path.clone()).collect());
    }

    /// Re-renders once a second while a build or run is in progress so the
    /// activity view's elapsed time stays current; started when one starts
    /// (`BuildEvent::PhaseChanged`), so an idle window has no timer.
    pub(super) fn follow_activity(&mut self, cx: &mut Context<Self>) {
        if self.activity_ticker.is_some() || !self.build.read(cx).phase.is_busy() {
            return;
        }
        self.activity_ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let busy = this.update(cx, |this, cx| {
                    let busy = this.build.read(cx).phase.is_busy();
                    if busy {
                        cx.notify();
                    } else {
                        this.activity_ticker = None;
                    }
                    busy
                });
                if !matches!(busy, Ok(true)) {
                    break;
                }
            }
        }));
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
        let mut previews = vec![];
        let moved_to = |file: &Path| {
            let rel = file.strip_prefix(&path).unwrap_or(Path::new(""));
            if rel.as_os_str().is_empty() { target.clone() } else { target.join(rel) }
        };
        for pane in &self.panes {
            for item in pane.items.iter().filter_map(Item::editor).filter(|item| item.path.starts_with(&path)) {
                reopen.push((pane.id, moved_to(&item.path)));
            }
            for file in pane.items.iter().filter_map(Item::rendered_path).filter(|file| file.starts_with(&path)) {
                previews.push((pane.id, moved_to(file)));
            }
        }
        let moved: Vec<PathBuf> = self.buffers.iter().map(|b| b.path.clone()).filter(|p| p.starts_with(&path)).collect();
        for file in moved {
            self.close_file_everywhere(&file, window, cx);
        }
        for (pane, new_path) in reopen {
            self.open_file_in(pane, new_path, None, false, window, cx);
        }
        for (pane, new_path) in previews {
            self.add_rendered_item(pane, &new_path, window, cx);
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
            terminals: self.panel_terminal_dirs(cx),
            terminal_tab: self.debug_tab == DebugTab::Terminal,
            show_agent_panel: self.agent.shown,
            agent_panel: match &self.agent.panel {
                Some(panel) => Some(panel.read(cx).session_state()),
                None => self.agent.restored.clone(),
            },
        };
        self.session = Session::load();
        self.session.set_project(&self.project.root, state);
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
        // The mention picker goes back to the message being written.
        let back_to_agent = std::mem::take(&mut self.agent.mention_open).then(|| self.agent.panel.clone()).flatten();
        // The commit message waits for the next Commit….
        if let Some(sheet) = self.git.commit_sheet.take().and_then(|s| s.upgrade()) {
            self.git.commit_draft = sheet.read(cx).draft(cx);
        }
        if self.modal.take().is_some() {
            if let Some(panel) = back_to_agent {
                let composer = panel.read(cx).composer.clone();
                composer.update(cx, |state, cx| state.focus(window, cx));
                cx.notify();
                return;
            }
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

    pub fn debug_state(&mut self, window: &Window, cx: &App) -> serde_json::Value {
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
                "laid_out_bytes": state.laid_out_bytes(),
                // "Large" or "Minified" when the file has no syntax tree.
                "no_syntax_tree": state.no_syntax_tree().map(|reason| format!("{reason:?}")),
                "fold_candidates": state.fold_candidate_count(),
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
            "resident_mb": crate::perf::mb(perf.current().resident),
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
            "lsp": format!("{:?}", self.lsp.read(cx).status_for(self.active_item().map(|i| i.path.as_path()))),
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
        // The running program: the app lldb-dap launched or attached to on
        // this Mac, else what Run started (the app, simctl or devicectl).
        state["run_pid"] = self.debugger.read(cx).local_debuggee_pid().or(build.run_pid()).into();
        state["debugger"]["thread"] = self.debugger.read(cx).stopped_thread_label().into();
        state["timings"] = perf.timings_json();
        state["profile"] = self.profile_summary(cx);
        state["archives"] = self.archives_summary(cx);
        state["asset_catalog"] = self.asset_catalog_summary(cx);
        state["image"] = self.image_summary(cx);
        state["rendered"] = self.rendered_summary(cx);
        state["breakpoints"] = self.breakpoints_summary(cx);
        state["bookmarks"] = self.bookmarks_summary(cx);
        state["packages"] = self.packages_summary(cx);
        state["console"] = self.console_summary(cx);
        state["panes"] = self.panes_summary(cx);
        state["git"] = self.git_summary(cx);
        state["multibuffer"] = self.multibuffer_summary(cx);
        state["terminals"] = self.terminals_summary(window, cx);
        state["vim"] = self.vim_summary(window, cx);
        state["settings"] = self.settings_summary(window, cx);
        state["agent"] = self.agent_summary(window, cx);
        state["predictions"] = self.predictions_summary(cx);
        state["previews"] = self.previews_summary(cx);
        state["language_servers"] = self.lsp.read(cx).summary();
        state["lsp_documents"] = self.lsp.read(cx).documents_summary();
        state["build_server"] = self.build_server_status(cx);
        // FSEvents: how many of the checks of files on disk it covers.
        state["file_watching"] = crate::fs_watch::status();
        // Whether an idle window still draws: editors' paints since launch,
        // and whether the focused one's caret blinks (each blink is a frame).
        state["drawing"] = serde_json::json!({
            "editor_paints": zcode_editor::paint_count(),
            "caret_blinking": self.active_editor().map(|editor| editor.read(cx).caret_blinking(cx)),
            "window_active": window.is_window_active(),
            // In points, for checks that click something placed from an edge.
            "window_size": [f32::from(window.viewport_size().width), f32::from(window.viewport_size().height)],
        });
        state["language_features"] = serde_json::json!({
            "server": self.active_item().map(|i| self.lsp.read(cx).features_for(&i.path)).unwrap_or_default().summary(),
            "inlay_hints": self.inlay_hints_summary(cx),
            "semantic_tokens": self.semantic_tokens_summary(cx),
            "signature_help": self.signature_help_summary(cx),
            "hierarchy": self.hierarchy_summary(),
        });
        state["last_log"] = serde_json::json!(build.reports.first().map(|r| r.log_path.clone()));
        state["last_report"] = serde_json::json!(build.reports.first().map(|r| serde_json::json!({
            "action": r.action.verb(),
            "scheme": r.scheme,
            "destination": r.destination,
            "success": r.success,
        })));
        state["scheme_options"] = serde_json::json!(build.scheme_options.as_ref().map(edit_scheme::options_summary));
        state["edit_scheme"] =
            serde_json::json!(self.edit_scheme_dialog.as_ref().and_then(|d| d.upgrade()).map(|d| d.read(cx).summary(cx)));
        state["find"] = self.find_summary(cx);
        state
    }
}

impl Workspace {
    /// The control channel's `bench index`: the index build for the
    /// scheme and destination chosen now, as a save would start it.
    pub fn start_index_build(&mut self, cx: &mut Context<Self>) {
        self.build.update(cx, |build, cx| build.refresh_index(cx));
    }

    /// The control channel's `bench status`: what the scale benchmarks
    /// (`scripts/bench.py`) poll, a few lines however big the project.
    pub fn bench_status(&self, cx: &App) -> serde_json::Value {
        let perf = crate::perf::Perf::global(cx);
        let build = self.build.read(cx);
        let memory = perf.current();
        let active = self.active_item().zip(self.active_buffer());
        serde_json::json!({
            "files": self.files.len(),
            "timings": perf.timings_json(),
            "memory": {
                "footprint_mb": crate::perf::mb(memory.footprint),
                "resident_mb": crate::perf::mb(memory.resident),
                "baseline_footprint_mb": crate::perf::mb(perf.baseline_footprint()),
                "peak_footprint_mb": crate::perf::mb(perf.peak_footprint.max(memory.lifetime_peak)),
                "language_server_mb": perf.language_server.map(|(_, s)| crate::perf::mb(s.footprint)),
                "language_server_resident_mb": perf.language_server.map(|(_, s)| crate::perf::mb(s.resident)),
            },
            // The active file's language server, as in `dump`.
            "lsp": format!("{:?}", self.lsp.read(cx).status_for(active.map(|(item, _)| item.path.as_path()))),
            "lsp_progress": self.lsp.read(cx).progress_message(),
            "scheme": build.scheme,
            "destination": build.destination.as_ref().map(|d| d.name.clone()),
            "indexing": build.indexing,
            // Where the index build's compilation database is, once written.
            "compile_database": Some(crate::build::compiledb::database_path(&self.cache_dir)).filter(|p| p.exists()),
            "build_server": self.build_server_status(cx),
            "find_results": self.find.results.iter().map(|f| f.matches.len()).sum::<usize>(),
            "find_files": self.find.results.len(),
            "find_searching": self.find.searching,
            "buffers": self.buffers.len(),
            "active": active.map(|(_, b)| serde_json::json!({
                "path": b.path,
                "dirty": b.dirty,
                "lines": zcode_editor::RopeExt::lines_len(b.editor.read(cx).text()),
                "semantic_tokens": b.tokens.count,
            })),
            "navigator": format!("{:?}", self.navigator),
            "modal_open": self.modal.is_some(),
        })
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

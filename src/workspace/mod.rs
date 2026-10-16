//! The project window: navigator, editor tabs, debug area, toolbar, status.

mod editing;
mod navigator;
mod panels;
mod pickers;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use gpui_kit::component::highlighter::{Diagnostic as EditorDiagnostic, DiagnosticSeverity};
use gpui_kit::component::input::{EditorState, InputEvent, InputState, Position, RopeExt as _, TabSize};
use gpui_kit::component::tree::TreeState;
use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;

use crate::actions::*;
use crate::build::log::{Issue, IssueSource, Severity};
use crate::build::{BuildAction, BuildController, BuildEvent};
use crate::debugger::{DebugState, DebugTarget, Debugger, DebuggerEvent};
use gpui_kit::component::input::{RangeDecoration, RangeDecorationCollection, RangeDecorationStyle};
use crate::lsp::{self, DocumentProviders, LspStore, LspStoreEvent};
use crate::project::{self, ProjectInfo};
use crate::search::{FileMatches, SearchOptions};
use crate::session::{ProjectSession, Session};

pub use navigator::NavigatorTab;
pub use panels::DebugTab;

const DEFAULT_FONT_SIZE: f32 = 13.;
const MAX_OPEN_FILE_SIZE: u64 = 16 * 1024 * 1024;

/// A position to reveal after opening a file.
#[derive(Clone, Debug)]
pub enum Target {
    /// 0-based line and UTF-16 column (LSP / compiler coordinates).
    Line { line: u32, column: u32 },
    LspRange(lsp_types::Range),
    Offset(usize),
}

#[derive(Clone, Debug, PartialEq)]
struct HistoryEntry {
    path: PathBuf,
    offset: usize,
}

pub struct Buffer {
    pub path: PathBuf,
    pub editor: Entity<EditorState>,
    pub dirty: bool,
    /// Text as last loaded/saved, so undoing back to it clears `dirty`.
    saved_text: gpui_kit::component::input::Rope,
    pub language: SharedString,
    mtime: Option<SystemTime>,
    breakpoint_marks: Option<RangeDecorationCollection>,
    exec_mark: Option<RangeDecorationCollection>,
    _subscriptions: Vec<Subscription>,
}

impl Buffer {
    pub fn title(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

struct Modal {
    view: AnyView,
    _subscription: Subscription,
}

pub struct FindState {
    pub input: Entity<InputState>,
    pub options: SearchOptions,
    pub results: Arc<Vec<FileMatches>>,
    pub searching: bool,
    pub error: Option<String>,
    pub title: Option<String>,
    cancel: Arc<AtomicBool>,
    _task: Option<Task<()>>,
}

pub struct Workspace {
    focus_handle: FocusHandle,
    pub project: ProjectInfo,
    pub files: Arc<Vec<PathBuf>>,
    pub tree: Entity<TreeState>,
    pub lsp: Entity<LspStore>,
    pub build: Entity<BuildController>,
    pub debugger: Entity<Debugger>,
    pub buffers: Vec<Buffer>,
    pub active: Option<usize>,
    pub navigator: NavigatorTab,
    pub show_navigator: bool,
    pub show_debug_area: bool,
    pub debug_tab: DebugTab,
    pub raw_log: bool,
    pub find: FindState,
    pub outline: Vec<pickers::OutlineEntry>,
    outline_path: Option<PathBuf>,
    pub scm_status: Vec<(String, PathBuf)>,
    pub scm_branch: Option<String>,
    pub font_size: f32,
    pub soft_wrap: bool,
    memory_label: String,
    back: Vec<HistoryEntry>,
    forward: Vec<HistoryEntry>,
    modal: Option<Modal>,
    issue_cursor: Option<usize>,
    debug_scroll: UniformListScrollHandle,
    restore_destination: Option<String>,
    pub outline_generation: u64,
    session: Session,
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
            files: Arc::new(vec![]),
            tree,
            lsp,
            build,
            debugger,
            buffers: vec![],
            active: None,
            navigator: NavigatorTab::Project,
            show_navigator: saved.show_navigator.unwrap_or(true),
            show_debug_area: saved.show_debug_area.unwrap_or(false),
            debug_tab: DebugTab::Console,
            raw_log: false,
            find: FindState {
                input: find_input,
                options: SearchOptions::default(),
                results: Arc::new(vec![]),
                searching: false,
                error: None,
                title: None,
                cancel: Arc::new(AtomicBool::new(false)),
                _task: None,
            },
            outline: vec![],
            outline_path: None,
            scm_status: vec![],
            scm_branch: None,
            font_size: session.font_size.unwrap_or(DEFAULT_FONT_SIZE),
            soft_wrap: false,
            memory_label: String::new(),
            back: vec![],
            forward: vec![],
            modal: None,
            issue_cursor: None,
            debug_scroll: UniformListScrollHandle::new(),
            restore_destination: None,
            outline_generation: 0,
            session,
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
        this.restore_destination = saved.destination.clone();
        this.debugger.update(cx, |d, _| {
            d.breakpoints = saved
                .breakpoints
                .iter()
                .filter(|(path, _)| path.exists())
                .map(|(path, lines)| (path.clone(), lines.iter().copied().collect()))
                .collect();
            d.breakpoints_active = saved.breakpoints_active.unwrap_or(true);
        });

        mark("state");
        this.rescan(cx);
        this.refresh_scm(cx);
        this.start_activity_ticker(cx);
        this.watch_files(cx);
        mark("background tasks");
        this.restore_tabs(saved, opened_path, window, cx);
        mark("restore tabs");
        if this.active.is_none() {
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
        match self.active_buffer() {
            Some(buffer) => format!("{} — {}", self.project.name, buffer.title()),
            None => self.project.name.clone(),
        }
    }

    // ---------------------------------------------------------------------
    // Files and the project tree

    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        let root = self.project.root.clone();
        let task = cx.spawn(async move |this, cx| {
            let scan_root = root.clone();
            let files = cx
                .background_spawn(async move { project::scan_files(&scan_root) })
                .await;
            this.update(cx, |this, cx| {
                // Tree items are !Send, so they are built here (fast).
                let items = project::build_tree(&root, &files);
                this.files = Arc::new(files);
                this.tree.update(cx, |tree, cx| tree.set_items(items, cx));
                this.reveal_active_in_tree(cx);
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
            .active_buffer()
            .and_then(|b| b.path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.project.root.clone());
        self.new_file_in(dir, window, cx);
    }

    /// Moves a file or folder to the user's Trash (Finder-style naming on
    /// collision) and closes any tabs inside it.
    pub(super) fn move_to_trash(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
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
        while let Some(ix) = self.buffers.iter().position(|b| b.path.starts_with(&path)) {
            self.force_close(ix, window, cx);
        }
        self.rescan(cx);
        self.refresh_scm(cx);
    }

    /// Renames a file or folder in place, keeping open tabs pointed at it.
    pub fn rename_path(&mut self, path: PathBuf, new_name: String, window: &mut Window, cx: &mut Context<Self>) {
        let path = path.canonicalize().unwrap_or(path);
        let Some(parent) = path.parent() else { return };
        let target = parent.join(&new_name);
        if target.exists() {
            self.notify_error(format!("“{new_name}” already exists."), window, cx);
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
        let mut reopen = vec![];
        while let Some(ix) = self.buffers.iter().position(|b| b.path.starts_with(&path)) {
            let old = self.buffers[ix].path.clone();
            let rel = old.strip_prefix(&path).unwrap_or(Path::new(""));
            reopen.push(if rel.as_os_str().is_empty() { target.clone() } else { target.join(rel) });
            self.force_close(ix, window, cx);
        }
        for new_path in reopen {
            self.open_file(new_path, None, false, window, cx);
        }
        self.rescan(cx);
        self.refresh_scm(cx);
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
            if !path.exists() {
                let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let template = match path.extension().and_then(|e| e.to_str()) {
                    Some("swift") => format!("//\n//  {}\n//\n\nimport Foundation\n\n", path.file_name().unwrap_or_default().to_string_lossy()),
                    _ => String::new(),
                };
                let _ = stem;
                if std::fs::write(&path, template).is_err() {
                    return;
                }
            }
            this.update_in(cx, |this, window, cx| {
                this.rescan(cx);
                this.open_file(path, None, true, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn reload_from_disk(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(path) else { return };
        buffer.mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        buffer.saved_text = gpui_kit::component::input::Rope::from(text.as_str());
        let editor = buffer.editor.clone();
        let Some(window) = cx.windows().first().copied() else { return };
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |state, cx| {
                    if *state.text() != text {
                        let cursor = state.cursor();
                        state.set_value(text, window, cx);
                        let clamped = cursor.min(state.text().len());
                        state.set_selected_range(clamped..clamped, cx);
                    }
                });
            })
            .ok();
        let text = self
            .buffers
            .iter()
            .find(|b| b.path == path)
            .map(|b| b.editor.read(cx).text().clone());
        if let Some(text) = text {
            self.lsp.update(cx, |lsp, cx| lsp.document_changed(path, text, cx));
        }
    }

    fn restore_tabs(
        &mut self,
        saved: ProjectSession,
        opened_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opened_file = opened_path.is_file()
            && !opened_path.ends_with("Package.swift")
            && project::detect(&opened_path).root == self.project.root
            && opened_path.extension().is_some_and(|e| e != "xcodeproj" && e != "xcworkspace");
        for path in saved.open_files.iter().filter(|p| p.exists()) {
            self.open_file(path.clone(), None, false, window, cx);
        }
        if opened_file {
            self.open_file(opened_path, None, false, window, cx);
        } else if let Some(active) = saved.active_file.filter(|p| p.exists()) {
            self.open_file(active, None, false, window, cx);
        }
    }

    pub fn save_session(&mut self, cx: &App) {
        let build = self.build.read(cx);
        let state = ProjectSession {
            open_files: self.buffers.iter().map(|b| b.path.clone()).collect(),
            active_file: self.active_buffer().map(|b| b.path.clone()),
            scheme: build.scheme.clone(),
            destination: build.destination.as_ref().map(|d| d.id.clone()),
            configuration: Some(build.configuration.clone()),
            show_navigator: Some(self.show_navigator),
            show_debug_area: Some(self.show_debug_area),
            breakpoints: self
                .debugger
                .read(cx)
                .breakpoints
                .iter()
                .map(|(path, lines)| (path.clone(), lines.iter().copied().collect()))
                .collect(),
            breakpoints_active: Some(self.debugger.read(cx).breakpoints_active),
        };
        self.session = Session::load();
        self.session.set_project(&self.project.root, state);
        self.session.font_size = Some(self.font_size);
        self.session.save();
    }

    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.project.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    // ---------------------------------------------------------------------
    // Buffers

    pub fn active_buffer(&self) -> Option<&Buffer> {
        self.active.and_then(|ix| self.buffers.get(ix))
    }

    pub fn active_editor(&self) -> Option<Entity<EditorState>> {
        self.active_buffer().map(|b| b.editor.clone())
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

    /// Opens (or focuses) a file, optionally revealing a location.
    pub fn open_file(
        &mut self,
        path: PathBuf,
        target: Option<Target>,
        record_history: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = path.canonicalize().unwrap_or(path);
        if record_history {
            self.push_history(cx);
        }
        if let Some(ix) = self.buffers.iter().position(|b| b.path == path) {
            self.activate(ix, window, cx);
            if let Some(target) = target {
                self.reveal_target(ix, target, window, cx);
            }
            return;
        }

        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > MAX_OPEN_FILE_SIZE {
            self.notify_error(format!("{} is too large to open", path.display()), window, cx);
            return;
        }
        let text = match std::fs::read(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => {
                    self.notify_error(
                        format!("{} is not a UTF-8 text file", path.display()),
                        window,
                        cx,
                    );
                    return;
                }
            },
            Err(err) => {
                self.notify_error(format!("Couldn’t open {}: {err}", path.display()), window, cx);
                return;
            }
        };
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let language = Self::language_for(&path);
        let soft_wrap = self.soft_wrap;
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(true)
                .indent_guides(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .soft_wrap(soft_wrap)
                .default_value(text)
        });

        let lsp_language = lsp::language_id_for(&path);
        if let Some(language_id) = lsp_language {
            let apply_edit: crate::lsp::ApplyEditFn = {
                let workspace = cx.weak_entity();
                Rc::new(move |edit, window, cx| {
                    workspace
                        .update(cx, |this, cx| this.apply_workspace_edit(edit, window, cx))
                        .ok();
                })
            };
            DocumentProviders::new(self.lsp.downgrade(), path.clone(), apply_edit)
                .install(&editor, cx);
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
            self.lsp.update(cx, |store, cx| {
                store.open_document(path.clone(), language_id, editor.read(cx).text().clone(), cx)
            });
        }

        let change = cx.subscribe(&editor, {
            let path = path.clone();
            move |this, _, event: &InputEvent, cx| {
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
                    let text = this
                        .buffers
                        .iter()
                        .find(|b| b.path == path)
                        .map(|b| b.editor.read(cx).text().clone());
                    if let Some(text) = text {
                        this.lsp
                            .update(cx, |lsp, cx| lsp.document_changed(&path, text, cx));
                    }
                }
            }
        });
        let observe = cx.observe(&editor, |_, _, cx| cx.notify());

        let saved_text = editor.read(cx).text().clone();
        self.buffers.push(Buffer {
            path: path.clone(),
            saved_text,
            editor,
            dirty: false,
            language: language.into(),
            mtime,
            breakpoint_marks: None,
            exec_mark: None,
            _subscriptions: vec![change, observe],
        });
        let ix = self.buffers.len() - 1;
        self.refresh_diagnostics_for(&path, cx);
        self.refresh_breakpoint_marks(&path, cx);
        self.activate(ix, window, cx);
        if let Some(target) = target {
            self.reveal_target(ix, target.clone(), window, cx);
            // Scrolling needs the editor's first layout; reveal again after
            // it unless the user has already moved the cursor.
            let revealed_at = self.buffers[ix].editor.read(cx).selected_range();
            let workspace = cx.weak_entity();
            cx.spawn_in(window, async move |_, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(32))
                    .await;
                workspace
                    .update_in(cx, |this, window, cx| {
                        if let Some(ix) = this.buffers.iter().position(|b| b.path == path)
                            && this.buffers[ix].editor.read(cx).selected_range() == revealed_at
                        {
                            this.reveal_target(ix, target, window, cx);
                        }
                    })
                    .ok();
            })
            .detach();
        }
    }

    fn reveal_target(&mut self, ix: usize, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.get(ix) else { return };
        buffer.editor.update(cx, |state, cx| {
            let text = state.text().clone();
            let range = match target {
                Target::Offset(offset) => offset..offset,
                Target::Line { line, column } => {
                    let offset = lsp::lsp_to_offset(&text, lsp_types::Position::new(line, column));
                    offset..offset
                }
                Target::LspRange(range) => lsp::lsp_range_to_offsets(&text, range),
            };
            state.set_selected_range(range.start..range.start, cx);
            if range.end > range.start {
                state.set_selected_range(range, cx);
            }
            state.focus(window, cx);
        });
        cx.notify();
    }

    pub fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.buffers.len() {
            return;
        }
        self.active = Some(ix);
        let editor = self.buffers[ix].editor.clone();
        editor.update(cx, |state, cx| state.focus(window, cx));
        window.set_window_title(&self.title());
        window.set_window_edited(self.buffers.iter().any(|b| b.dirty));
        self.reveal_active_in_tree(cx);
        if self.navigator == NavigatorTab::Symbols {
            self.refresh_outline(cx);
        }
        cx.notify();
    }

    fn reveal_active_in_tree(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.active_buffer().map(|b| b.path.to_string_lossy().to_string()) else {
            return;
        };
        let id: SharedString = path.into();
        self.tree.update(cx, |tree, cx| {
            if let Some(ix) = tree.index_of(&id) {
                tree.set_selected_index(Some(ix), cx);
                tree.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
        });
    }

    pub fn close_buffer(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.buffers.len() {
            return;
        }
        if self.buffers[ix].dirty {
            let name = self.buffers[ix].title();
            let answer = window.prompt(
                PromptLevel::Warning,
                &format!("Do you want to save the changes you made to “{name}”?"),
                Some("Your changes will be lost if you don’t save them."),
                &[
                    PromptButton::ok("Save"),
                    PromptButton::new("Don’t Save"),
                    PromptButton::cancel("Cancel"),
                ],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                let choice = answer.await.unwrap_or(2);
                this.update_in(cx, |this, window, cx| match choice {
                    0 => {
                        this.save_buffer(ix, window, cx);
                        this.force_close(ix, window, cx);
                    }
                    1 => this.force_close(ix, window, cx),
                    _ => {}
                })
                .ok();
            })
            .detach();
            return;
        }
        self.force_close(ix, window, cx);
    }

    fn force_close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.buffers.len() {
            return;
        }
        let buffer = self.buffers.remove(ix);
        self.lsp.update(cx, |lsp, _| lsp.close_document(&buffer.path));
        self.active = match self.active {
            _ if self.buffers.is_empty() => None,
            Some(active) if active > ix => Some(active - 1),
            Some(active) if active == ix => Some(ix.min(self.buffers.len() - 1)),
            other => other,
        };
        if let Some(active) = self.active {
            self.activate(active, window, cx);
        } else {
            window.set_window_title(&self.title());
            self.focus_handle.focus(window, cx);
        }
        window.set_window_edited(self.buffers.iter().any(|b| b.dirty));
        cx.notify();
    }

    pub fn save_buffer(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.get_mut(ix) else { return };
        let mut text = buffer.editor.read(cx).text().to_string();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        match std::fs::write(&buffer.path, &text) {
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

    // ---------------------------------------------------------------------
    // Navigation history

    fn current_location(&self, cx: &App) -> Option<HistoryEntry> {
        let buffer = self.active_buffer()?;
        Some(HistoryEntry {
            path: buffer.path.clone(),
            offset: buffer.editor.read(cx).cursor(),
        })
    }

    fn push_history(&mut self, cx: &App) {
        if let Some(entry) = self.current_location(cx)
            && self.back.last() != Some(&entry)
        {
            self.back.push(entry);
            if self.back.len() > 100 {
                self.back.remove(0);
            }
            self.forward.clear();
        }
    }

    fn go_back(&mut self, _: &GoBack, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.back.pop() {
            if let Some(current) = self.current_location(cx) {
                self.forward.push(current);
            }
            self.open_file(entry.path, Some(Target::Offset(entry.offset)), false, window, cx);
        }
    }

    fn go_forward(&mut self, _: &GoForward, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.forward.pop() {
            if let Some(current) = self.current_location(cx) {
                self.back.push(current);
            }
            self.open_file(entry.path, Some(Target::Offset(entry.offset)), false, window, cx);
        }
    }

    // ---------------------------------------------------------------------
    // Diagnostics and issues

    fn on_lsp_event(
        &mut self,
        _: &Entity<LspStore>,
        event: &LspStoreEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            LspStoreEvent::DiagnosticsChanged(path) => {
                self.refresh_diagnostics_for(path, cx);
                cx.notify();
            }
            LspStoreEvent::StatusChanged => {
                if self.navigator == NavigatorTab::Symbols {
                    self.refresh_outline(cx);
                }
                cx.notify();
            }
            LspStoreEvent::ApplyEdit(edit) => {
                self.apply_workspace_edit(edit.clone(), window, cx);
            }
        }
    }

    fn on_build_event(
        &mut self,
        build: &Entity<BuildController>,
        event: &BuildEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            BuildEvent::IssuesChanged => {
                let paths: Vec<PathBuf> = self.buffers.iter().map(|b| b.path.clone()).collect();
                for path in paths {
                    self.refresh_diagnostics_for(&path, cx);
                }
            }
            BuildEvent::CompileDatabaseUpdated(path) => {
                let cache_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                let args = Self::lsp_args(&self.project, &cache_dir);
                let db = path.clone();
                self.lsp.update(cx, |lsp, cx| {
                    lsp.set_extra_args(args, cx);
                    lsp.files_changed(&[db]);
                });
            }
            BuildEvent::ConfigurationChanged => {
                if let Some(id) = self.restore_destination.take() {
                    build.update(cx, |build, cx| {
                        if let Some(dest) = build.destinations.iter().find(|d| d.id == id).cloned() {
                            build.set_destination(dest, cx);
                        } else {
                            // Keep waiting until destinations load.
                            if build.destinations.is_empty() {
                                self.restore_destination = Some(id);
                            }
                        }
                    });
                }
            }
            BuildEvent::PhaseChanged => {
                let phase = build.read(cx).phase.clone();
                match phase {
                    crate::build::Phase::Running { .. } => {
                        self.show_debug_area = true;
                        self.debug_tab = DebugTab::Console;
                    }
                    crate::build::Phase::Finished { success: false, .. } => {
                        if build.read(cx).error_count() > 0 {
                            self.navigator = NavigatorTab::Issues;
                            self.show_navigator = true;
                        } else {
                            self.show_debug_area = true;
                            self.debug_tab = DebugTab::BuildLog;
                        }
                    }
                    crate::build::Phase::Finished { action: BuildAction::Test, .. } => {
                        self.navigator = NavigatorTab::Tests;
                    }
                    _ => {}
                }
                let _ = window;
            }
            BuildEvent::AttachDebugger { pid } => {
                let pid = *pid;
                self.debugger
                    .update(cx, |d, cx| d.start(DebugTarget::Attach { pid }, cx));
            }
            BuildEvent::LaunchDebugger { program, cwd } => {
                let target = DebugTarget::Launch {
                    program: program.clone(),
                    cwd: cwd.clone(),
                    args: vec![],
                };
                self.show_debug_area = true;
                self.debugger.update(cx, |d, cx| d.start(target, cx));
            }
            BuildEvent::LogUpdated | BuildEvent::ConsoleUpdated => {
                let count = self.debug_line_count(cx);
                if count > 0 {
                    self.debug_scroll
                        .scroll_to_item(count - 1, ScrollStrategy::Bottom);
                }
            }
        }
        cx.notify();
    }

    fn on_debugger_event(
        &mut self,
        _: &Entity<Debugger>,
        event: &DebuggerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DebuggerEvent::Output(line) => {
                let line = line.clone();
                self.build.update(cx, |b, cx| b.append_console(line, cx));
            }
            DebuggerEvent::BreakpointsChanged(path) => self.refresh_breakpoint_marks(path, cx),
            DebuggerEvent::Stopped { path, line } => {
                self.show_debug_area = true;
                self.debug_tab = DebugTab::Console;
                self.navigator = NavigatorTab::Debug;
                self.show_navigator = true;
                let target = Target::Line { line: line.saturating_sub(1), column: 0 };
                self.open_file(path.clone(), Some(target), false, window, cx);
                self.set_exec_mark(Some((path.clone(), *line)), cx);
                window.activate_window();
            }
            DebuggerEvent::StateChanged => {
                if !matches!(self.debugger.read(cx).state, DebugState::Stopped { .. }) {
                    self.set_exec_mark(None, cx);
                }
            }
            DebuggerEvent::Ended { exit_code } => {
                let code = *exit_code;
                self.set_exec_mark(None, cx);
                self.build.update(cx, |b, cx| b.finish_debug_run(code, cx));
            }
        }
        cx.notify();
    }

    /// Byte range covering a whole 1-based line, newline included.
    fn line_range(text: &gpui_kit::component::input::Rope, line: u32) -> Option<std::ops::Range<usize>> {
        let row = line.checked_sub(1)? as usize;
        if row >= text.lines_len() {
            return None;
        }
        let start = text.line_start_offset(row);
        let end = (text.line_end_offset(row) + 1).min(text.len());
        (end > start).then_some(start..end)
    }

    pub(super) fn refresh_breakpoint_marks(&mut self, path: &Path, cx: &mut Context<Self>) {
        let (lines, active) = {
            let debugger = self.debugger.read(cx);
            (
                debugger.breakpoints.get(path).cloned().unwrap_or_default(),
                debugger.breakpoints_active,
            )
        };
        let color = if active {
            gpui_kit::hsla(0.6, 0.85, 0.55, 0.22)
        } else {
            gpui_kit::hsla(0.6, 0.1, 0.55, 0.18)
        };
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else { return };
        let text = buffer.editor.read(cx).text().clone();
        let decorations: Vec<_> = lines
            .iter()
            .filter_map(|&line| Self::line_range(&text, line))
            .map(|range| RangeDecoration::new(range).with_style(RangeDecorationStyle::Fill).with_color(color))
            .collect();
        match &buffer.breakpoint_marks {
            Some(marks) => marks.set(decorations, cx),
            None => {
                let marks = buffer
                    .editor
                    .update(cx, |state, cx| state.create_range_decorations_collection(decorations, cx));
                buffer.breakpoint_marks = Some(marks);
            }
        }
    }

    /// Highlights the line execution stopped on (green, like Xcode).
    fn set_exec_mark(&mut self, location: Option<(PathBuf, u32)>, cx: &mut Context<Self>) {
        for buffer in &mut self.buffers {
            let decorations: Vec<RangeDecoration> = match &location {
                Some((path, line)) if *path == buffer.path => {
                    let text = buffer.editor.read(cx).text().clone();
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
            match &buffer.exec_mark {
                Some(mark) => mark.set(decorations, cx),
                None if !decorations.is_empty() => {
                    let mark = buffer
                        .editor
                        .update(cx, |state, cx| state.create_range_decorations_collection(decorations, cx));
                    buffer.exec_mark = Some(mark);
                }
                None => {}
            }
        }
    }

    pub(super) fn toggle_breakpoint(&mut self, _: &crate::actions::ToggleBreakpoint, _: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.active_buffer() else { return };
        let path = buffer.path.clone();
        let line = buffer.editor.read(cx).cursor_position().line + 1;
        self.debugger
            .update(cx, |d, cx| d.toggle_breakpoint(path, line, cx));
    }

    /// Test failures name only the file; map such paths into the project.
    fn resolve_issue_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            return path.to_path_buf();
        }
        self.files
            .iter()
            .find(|f| f.file_name() == path.file_name())
            .map(|f| self.project.root.join(f))
            .unwrap_or_else(|| path.to_path_buf())
    }

    /// Combined, de-duplicated issues: live (LSP) diagnostics plus the last
    /// build's issues.
    pub fn all_issues(&self, cx: &App) -> Vec<Issue> {
        let mut issues: Vec<Issue> = vec![];
        for (path, diagnostics) in self.lsp.read(cx).diagnostics() {
            for d in diagnostics {
                let severity = match d.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => Severity::Error,
                    Some(lsp_types::DiagnosticSeverity::WARNING) => Severity::Warning,
                    _ => continue,
                };
                issues.push(Issue {
                    severity,
                    message: d.message.lines().next().unwrap_or("").to_string(),
                    path: Some(path.clone()),
                    line: Some(d.range.start.line + 1),
                    column: Some(d.range.start.character + 1),
                    source: IssueSource::Live,
                });
            }
        }
        for issue in &self.build.read(cx).issues {
            if issue.severity == Severity::Note {
                continue;
            }
            let mut issue = issue.clone();
            issue.path = issue.path.as_deref().map(|p| self.resolve_issue_path(p));
            let issue = &issue;
            let duplicate = issues.iter().any(|i| {
                i.path == issue.path && i.line == issue.line && i.message == issue.message
            });
            if !duplicate {
                issues.push(issue.clone());
            }
        }
        issues.sort_by(|a, b| {
            a.severity
                .cmp(&b.severity)
                .then(a.path.cmp(&b.path))
                .then(a.line.cmp(&b.line))
        });
        issues
    }

    fn refresh_diagnostics_for(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter().find(|b| b.path == path) else { return };
        let live = self.lsp.read(cx).diagnostics_for(path).to_vec();
        let build_issues: Vec<Issue> = self
            .build
            .read(cx)
            .issues
            .iter()
            .filter(|i| i.path.as_deref().map(|p| self.resolve_issue_path(p)).as_deref() == Some(path))
            .cloned()
            .collect();
        buffer.editor.update(cx, |state, cx| {
            let text = state.text().clone();
            let to_pos = |p: lsp_types::Position| -> Position {
                text.offset_to_position(lsp::lsp_to_offset(&text, p))
            };
            let mut entries: Vec<EditorDiagnostic> = live
                .iter()
                .map(|d| {
                    let severity = match d.severity {
                        Some(lsp_types::DiagnosticSeverity::ERROR) => DiagnosticSeverity::Error,
                        Some(lsp_types::DiagnosticSeverity::WARNING) => DiagnosticSeverity::Warning,
                        Some(lsp_types::DiagnosticSeverity::INFORMATION) => DiagnosticSeverity::Info,
                        _ => DiagnosticSeverity::Hint,
                    };
                    let mut end = d.range.end;
                    if end == d.range.start {
                        end.character += 1;
                    }
                    EditorDiagnostic::new(to_pos(d.range.start)..to_pos(end), d.message.clone())
                        .with_severity(severity)
                        .with_source("sourcekit-lsp")
                })
                .collect();
            for issue in build_issues {
                let line = issue.line.unwrap_or(1).saturating_sub(1);
                let col = issue.column.unwrap_or(1).saturating_sub(1);
                let start = to_pos(lsp_types::Position::new(line, col));
                let end = Position::new(start.line, start.character + 1);
                let severity = match issue.severity {
                    Severity::Error => DiagnosticSeverity::Error,
                    Severity::Warning => DiagnosticSeverity::Warning,
                    Severity::Note => DiagnosticSeverity::Info,
                };
                let duplicate = live.iter().any(|d| d.range.start.line == line && d.message == issue.message);
                if !duplicate {
                    entries.push(
                        EditorDiagnostic::new(start..end, issue.message.clone())
                            .with_severity(severity)
                            .with_source("build"),
                    );
                }
            }
            if let Some(set) = state.diagnostics_mut() {
                set.clear();
                set.extend(entries);
            }
            cx.notify();
        });
    }

    pub fn open_issue(&mut self, issue: &Issue, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = issue.path.clone() else { return };
        let target = Target::Line {
            line: issue.line.unwrap_or(1).saturating_sub(1),
            column: issue.column.unwrap_or(1).saturating_sub(1),
        };
        self.open_file(path, Some(target), true, window, cx);
    }

    fn step_issue(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let issues: Vec<Issue> = self
            .all_issues(cx)
            .into_iter()
            .filter(|i| i.path.is_some())
            .collect();
        if issues.is_empty() {
            return;
        }
        let len = issues.len() as isize;
        let next = match self.issue_cursor {
            Some(ix) => ((ix as isize + delta).rem_euclid(len)) as usize,
            None if delta >= 0 => 0,
            None => (len - 1) as usize,
        };
        self.issue_cursor = Some(next);
        let issue = issues[next].clone();
        self.open_issue(&issue, window, cx);
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
        let previously_active = self.active_buffer().map(|b| b.path.clone());
        for (path, mut edits) in per_file {
            let path = path.canonicalize().unwrap_or(path);
            // Apply back to front so earlier offsets stay valid.
            edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
            // Edits land in (possibly newly opened) tabs rather than straight
            // on disk: the language server sees the new text immediately and
            // the user can review the change before saving.
            if !self.buffers.iter().any(|b| b.path == path) && path.is_file() {
                self.open_file(path.clone(), None, false, window, cx);
            }
            if let Some(buffer) = self.buffers.iter().find(|b| b.path == path) {
                buffer.editor.update(cx, |state, cx| {
                    for edit in &edits {
                        let text = state.text().clone();
                        let range = lsp::lsp_range_to_offsets(&text, edit.range);
                        state.set_selected_range(range, cx);
                        state.replace(edit.new_text.clone(), window, cx);
                    }
                });
            } else if let Ok(contents) = std::fs::read_to_string(&path) {
                let mut text = gpui_kit::component::input::Rope::from(contents.as_str());
                for edit in &edits {
                    let range = lsp::lsp_range_to_offsets(&text, edit.range);
                    let mut s = text.to_string();
                    s.replace_range(range, &edit.new_text);
                    text = gpui_kit::component::input::Rope::from(s.as_str());
                }
                std::fs::write(&path, text.to_string()).ok();
                self.lsp.read(cx).files_changed(std::slice::from_ref(&path));
            }
            touched += 1;
        }
        if let Some(ix) = previously_active.and_then(|p| self.buffers.iter().position(|b| b.path == p)) {
            self.activate(ix, window, cx);
        }
        if touched > 1 {
            window.push_notification(
                Notification::info(format!("Updated {touched} files")),
                cx,
            );
        }
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // Modals

    pub fn show_modal<V>(&mut self, view: Entity<V>, window: &mut Window, cx: &mut Context<Self>)
    where
        V: Render + EventEmitter<DismissEvent> + Focusable,
    {
        let subscription = cx.subscribe_in(&view, window, |this, _, _: &DismissEvent, window, cx| {
            this.dismiss_modal(window, cx);
        });
        view.read(cx).focus_handle(cx).focus(window, cx);
        self.modal = Some(Modal {
            view: view.into(),
            _subscription: subscription,
        });
        cx.notify();
    }

    pub fn dismiss_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    // ---------------------------------------------------------------------
    // Find in workspace

    pub fn run_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.find.input.read(cx).value().to_string();
        let _ = window;
        self.find.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        if query.is_empty() {
            self.find.results = Arc::new(vec![]);
            self.find.searching = false;
            cx.notify();
            return;
        }
        let regex = match crate::search::build_regex(&query, &self.find.options) {
            Ok(regex) => regex,
            Err(err) => {
                self.find.error = Some(err.to_string());
                cx.notify();
                return;
            }
        };
        self.find.error = None;
        self.find.title = None;
        self.find.searching = true;
        let cancel = Arc::new(AtomicBool::new(false));
        self.find.cancel = cancel.clone();
        let root = self.project.root.clone();
        let files = self.files.clone();
        self.find._task = Some(cx.spawn(async move |this, cx| {
            let results = cx
                .background_spawn(async move { crate::search::search(&root, &files, &regex, &cancel) })
                .await;
            this.update(cx, |this, cx| {
                this.find.results = Arc::new(results);
                this.find.searching = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // Source control (read-only status)

    pub fn refresh_scm(&mut self, cx: &mut Context<Self>) {
        let root = self.project.root.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut status = std::process::Command::new("/usr/bin/git");
                    status
                        .args(["status", "--porcelain=v1", "-uall"])
                        .current_dir(&root);
                    let mut branch = std::process::Command::new("/usr/bin/git");
                    branch
                        .args(["rev-parse", "--abbrev-ref", "HEAD"])
                        .current_dir(&root);
                    let toplevel = std::process::Command::new("/usr/bin/git")
                        .args(["rev-parse", "--show-toplevel"])
                        .current_dir(&root)
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
                    let status = crate::build::process::run_quiet(status).await.ok();
                    let branch = crate::build::process::run_quiet(branch).await.ok();
                    (status, branch, toplevel)
                })
                .await;
            this.update(cx, |this, cx| {
                let (status, branch, toplevel) = result;
                if let (Some((true, out)), Some(top)) = (status, toplevel) {
                    this.scm_status = out
                        .lines()
                        .filter(|l| l.len() > 3)
                        .map(|l| {
                            // "XY path" or "R  old -> new"; show the most
                            // significant status letter and the current path.
                            let code = l[..2].trim();
                            let status = if code == "??" { "??" } else { &code[..1] };
                            let path = l[3..].rsplit(" -> ").next().unwrap_or(&l[3..]);
                            (status.to_string(), top.join(path.trim_matches('"')))
                        })
                        .collect();
                }
                this.scm_branch = branch
                    .filter(|(ok, _)| *ok)
                    .map(|(_, b)| b.trim().to_string());
                cx.notify();
            })
            .ok();
        });
        self._tasks.push(task);
    }
}

impl Workspace {
    /// A JSON summary of UI state for the debug harness and tests.
    pub fn debug_state(&self, cx: &App) -> serde_json::Value {
        let build = self.build.read(cx);
        let active = self.active_buffer().map(|b| {
            let state = b.editor.read(cx);
            let pos = state.cursor_position();
            serde_json::json!({
                "path": b.path,
                "dirty": b.dirty,
                "line": pos.line + 1,
                "column": pos.character + 1,
                "selection": state.selected_text().to_string(),
                "text": (state.text().len() < 8000).then(|| state.text().to_string()),
                "line_text": gpui_kit::component::input::RopeExt::slice_line(state.text(), pos.line as usize).to_string(),
                "diagnostics": state.diagnostics().map(|d| d.len()).unwrap_or(0),
                "completion_menu_open": state.completion_menu_state().open,
                "completion_items": state.completion_menu_state().items.iter().take(5).map(|i| i.label.clone()).collect::<Vec<_>>(),
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
        serde_json::json!({
            "project": self.project.name,
            "kind": self.project.kind.label(),
            "files": self.files.len(),
            "buffers": self.buffers.iter().map(|b| b.title()).collect::<Vec<_>>(),
            "active": active,
            "modal_open": self.modal.is_some(),
            "navigator": format!("{:?}", self.navigator),
            "show_navigator": self.show_navigator,
            "show_debug_area": self.show_debug_area,
            "lsp": format!("{:?}", self.lsp.read(cx).status()),
            "lsp_progress": self.lsp.read(cx).progress_message(),
            "scheme": build.scheme,
            "configuration": build.configuration,
            "destination": build.destination.as_ref().map(|d| format!("{} ({})", d.name, d.runtime)),
            "destinations": build.destinations.len(),
            "phase": format!("{:?}", build.phase),
            "indexing": build.indexing,
            "issues": issues.iter().map(|i| format!("{:?} {}:{} {}", i.severity, i.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), i.line.unwrap_or(0), i.message)).collect::<Vec<_>>(),
            "tests": build.tests.iter().map(|t| format!("{:?} {}", t.status, t.name)).collect::<Vec<_>>(),
            "console_tail": build.console.iter().rev().take(8).rev().cloned().collect::<Vec<_>>(),
            "log_lines": build.log.len(),
            "find_results": self.find.results.iter().map(|f| f.matches.len()).sum::<usize>(),
            "outline": self.outline.iter().map(|o| o.name.clone()).collect::<Vec<_>>(),
            "scm_branch": self.scm_branch,
            "scm_changes": self.scm_status.len(),
            "font_size": self.font_size,
            "memory": memory,
            "history": { "back": self.back.len(), "forward": self.forward.len() },
            "debugger": {
                "state": format!("{:?}", self.debugger.read(cx).state),
                "frames": self.debugger.read(cx).frames.iter().take(4).map(|f| format!("{} {}:{}", f.name, f.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), f.line)).collect::<Vec<_>>(),
                "variables": self.debugger.read(cx).variables.iter().take(8).map(|v| format!("{} = {}", v.name, v.value)).collect::<Vec<_>>(),
                "breakpoints": self.debugger.read(cx).breakpoints.values().map(|l| l.len()).sum::<usize>(),
            },
        })
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

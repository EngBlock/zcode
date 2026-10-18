//! Keeps a project's build graph current for its build server.
//!
//! Nothing happens until sourcekit-lsp connects (a Swift or C-family file
//! opens), so opening a project costs nothing. Then the graph is made from
//! the project model and the compilation database at once, and again with
//! what Xcode says once it has (seconds on a big app); and remade, off the
//! main thread, whenever a `project.pbxproj`, a synchronized folder or the
//! compilation database changes, or the scheme or destination does (which
//! asks Xcode again).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, Context, Entity, EventEmitter, Task, WeakEntity};
use serde_json::{Value, json};

use super::graph::{BuildGraph, Inputs, Origin};
use super::inputs::{self, Queried, Query, Stamps};
use super::server::{self, Shared};
use crate::build::compiledb;
use crate::build::{BuildController, BuildEvent};
use crate::project::{self, ProjectInfo};

/// How often the files the graph comes from are looked at.
const POLL: Duration = Duration::from_secs(2);

/// sourcekit-lsp was told some targets' files or arguments changed.
pub struct SettingsChanged;

pub struct BuildServer {
    shared: Arc<Shared>,
    project: ProjectInfo,
    cache_dir: PathBuf,
    build: WeakEntity<BuildController>,
    /// sourcekit-lsp has connected: keep the graph current.
    active: bool,
    queried: Arc<Queried>,
    /// What Xcode was last asked about (or is being asked).
    query: Option<Query>,
    querying: bool,
    stamps: Option<Stamps>,
    synchronized: Arc<Vec<PathBuf>>,
    /// Graphs made, and how long the last took.
    graphs: usize,
    last_graph: Option<Duration>,
    rebuild_task: Option<Task<()>>,
    query_task: Option<Task<()>>,
    poll_task: Option<Task<()>>,
    _wake_task: Task<()>,
    _subscription: gpui_kit::Subscription,
}

impl EventEmitter<SettingsChanged> for BuildServer {}

impl BuildServer {
    pub fn new(project: ProjectInfo, cache_dir: PathBuf, build: &Entity<BuildController>, cx: &mut Context<Self>) -> Self {
        let store = compiledb::index_store_path(&cache_dir);
        let database = store.parent().map(|p| p.join("IndexDatabase")).unwrap_or_default();
        let (shared, woken) = Shared::new(project.root.clone(), Some((store, database)));
        server::register(&shared);
        let wake_task = cx.spawn(async move |this, cx| {
            while woken.recv().await.is_ok() {
                if this.update(cx, |this, cx| this.wake(cx)).is_err() {
                    break;
                }
            }
        });
        let subscription = cx.subscribe(build, |this, _, event, cx| match event {
            BuildEvent::CompileDatabaseUpdated(_) if this.active => this.rebuild(cx),
            BuildEvent::ConfigurationChanged if this.active => this.ask_xcode(false, cx),
            _ => {}
        });
        Self {
            shared,
            project,
            cache_dir,
            build: build.downgrade(),
            active: false,
            queried: Default::default(),
            query: None,
            querying: false,
            stamps: None,
            synchronized: Default::default(),
            graphs: 0,
            last_graph: None,
            rebuild_task: None,
            query_task: None,
            poll_task: None,
            _wake_task: wake_task,
            _subscription: subscription,
        }
    }

    pub fn graph(&self) -> Arc<BuildGraph> {
        self.shared.graph()
    }

    /// A connection started, or sourcekit-lsp saw files change.
    fn wake(&mut self, cx: &mut Context<Self>) {
        if !self.active {
            self.active = true;
            self.ask_xcode(false, cx);
            self.poll(cx);
        }
        self.rebuild(cx);
    }

    /// zcode rewrote a `project.pbxproj` (or saved an `.xcconfig`): look
    /// again now rather than at the next poll.
    pub fn project_changed(&mut self, cx: &mut Context<Self>) {
        if self.active {
            self.rebuild(cx);
            self.ask_xcode(true, cx);
        }
    }

    fn projects(&self) -> Vec<PathBuf> {
        project::xcode_projects(&self.project.kind)
    }

    /// Makes the graph again from what's on disk and what Xcode said.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let projects = self.projects();
        let root = self.project.root.clone();
        let cache_dir = self.cache_dir.clone();
        let queried = self.queried.clone();
        let shared = self.shared.clone();
        // A rebuild started since supersedes this one: its graph is the one
        // installed (this task is dropped before it gets that far).
        self.rebuild_task = Some(cx.spawn(async move |this, cx| {
            let (graph, synchronized, elapsed) = cx
                .background_spawn(async move {
                    let started = Instant::now();
                    let store = compiledb::index_store_path(&cache_dir);
                    let inputs = Inputs {
                        root,
                        model: inputs::model_targets(&projects),
                        commands: compiledb::read(&compiledb::database_path(&cache_dir)),
                        index_settings: queried.index_settings.clone(),
                        build_settings: queried.build_settings.clone(),
                        // Where the compilation database had it, next to the store.
                        index_database: store.parent().map(|p| p.join("IndexDatabase")).unwrap_or_default(),
                        index_store: store,
                        default_sdk: inputs::default_sdk(),
                    };
                    (BuildGraph::build(&inputs), inputs::synchronized_folders(&projects), started.elapsed())
                })
                .await;
            this.update(cx, |this, cx| {
                let changed = shared.set_graph(graph);
                this.graphs += 1;
                this.last_graph = Some(elapsed);
                this.synchronized = Arc::new(synchronized);
                if changed > 0 {
                    log::info!("build server: graph made in {elapsed:?}, {changed} targets changed");
                    cx.emit(SettingsChanged);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Asks Xcode for the scheme's arguments and settings, when the scheme
    /// or destination changed since it last did (or `again`).
    fn ask_xcode(&mut self, again: bool, cx: &mut Context<Self>) {
        let Some(build) = self.build.upgrade() else { return };
        let build = build.read(cx);
        let (Some(scheme), Some(destination)) = (build.scheme.clone(), build.destination.clone()) else { return };
        let query = Query { scheme, configuration: build.configuration.clone(), destination };
        if !again && self.query.as_ref() == Some(&query) {
            return;
        }
        self.query = Some(query.clone());
        self.querying = true;
        let project = self.project.clone();
        let derived_data = self.cache_dir.join("IndexDerivedData");
        self.query_task = Some(cx.spawn(async move |this, cx| {
            // A burst of edits asks once.
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let queried = cx.background_spawn(async move { inputs::query(&project, &query, &derived_data) }).await;
            this.update(cx, |this, cx| {
                this.querying = false;
                this.queried = Arc::new(queried);
                this.rebuild(cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// Looks at the projects, their synchronized folders and the database
    /// every couple of seconds; anything changed remakes the graph, a
    /// project file changing asks Xcode again too.
    fn poll(&mut self, cx: &mut Context<Self>) {
        self.poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let Ok((projects, database, synchronized)) =
                    this.update(cx, |this, _| (this.projects(), compiledb::database_path(&this.cache_dir), this.synchronized.clone()))
                else {
                    break;
                };
                let stamps = cx.background_spawn(async move { Stamps::read(&projects, &database, &synchronized) }).await;
                let alive = this.update(cx, |this, cx| {
                    let Some(before) = this.stamps.replace(stamps.clone()) else { return };
                    if before == stamps {
                        return;
                    }
                    log::info!("build server: the project changed on disk");
                    this.rebuild(cx);
                    if before.projects != stamps.projects {
                        this.ask_xcode(true, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    /// What `dump` and `bsp status` show.
    pub fn status(&self) -> Value {
        let graph = self.shared.graph();
        let count = |origin: Origin| graph.targets.iter().filter(|t| t.swift_origin == Some(origin)).count();
        let files: usize = graph.targets.iter().map(|t| t.files.len()).sum();
        json!({
            "active": self.active,
            "ready": self.shared.is_ready(),
            "targets": graph.targets.len(),
            "files": files,
            // Targets' Swift arguments, by where they came from.
            "swift_arguments": {
                "index-settings": count(Origin::IndexSettings),
                "build-log": count(Origin::BuildLog),
                "build-settings": count(Origin::BuildSettings),
                // Their files alone, until something better is known.
                "file-alone": graph.targets.iter().filter(|t| t.swift.is_none()).count(),
            },
            "querying": self.querying,
            "query": self.query.as_ref().map(|q| json!({ "scheme": q.scheme, "configuration": q.configuration, "destination": q.destination.name })),
            "query_errors": self.queried.errors,
            "graphs": self.graphs,
            "last_graph_ms": self.last_graph.map(|d| d.as_millis() as u64),
            "connections": self.shared.stats(),
        })
    }
}

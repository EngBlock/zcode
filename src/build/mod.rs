//! Build, run, test and clean — the Product menu.

pub mod compiledb;
pub mod log;
pub mod process;
pub mod sim_tests;
pub mod xcode;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::{AppContext as _, Context, EventEmitter, Task};
use smol::channel;

use crate::project::{ProjectInfo, ProjectKind};
use crate::testing::TestRequest;
use crate::testing::plans::{self, TestPlan};
use log::{Issue, LogLine, Severity, SuiteEvent, TestResult, TestStatus};
use process::ProcessHandle;
use xcode::{BuildSettings, Destination, Platform, SchemeList};

/// Lines kept in memory per log; older lines are dropped from the view (the
/// full log is still written to disk).
const MAX_LOG_LINES: usize = 50_000;
const MAX_CONSOLE_LINES: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildAction {
    Build,
    Run,
    Test,
    Clean,
}

impl BuildAction {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Build | Self::Run => "Build",
            Self::Test => "Test",
            Self::Clean => "Clean",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Preparing(BuildAction),
    Building { action: BuildAction, started: Instant },
    Launching,
    Running { started: Instant },
    Finished {
        action: BuildAction,
        success: bool,
        duration: Duration,
    },
    Stopped,
}

impl Phase {
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Preparing(_) | Self::Building { .. } | Self::Launching | Self::Running { .. }
        )
    }
}

#[derive(Clone, Debug)]
pub struct Report {
    pub action: BuildAction,
    pub scheme: String,
    pub destination: String,
    pub finished_at: SystemTime,
    pub duration: Duration,
    pub success: bool,
    pub errors: usize,
    pub warnings: usize,
    pub log_path: PathBuf,
}

pub enum BuildEvent {
    /// Scheme list, destinations or selection changed.
    ConfigurationChanged,
    LogUpdated,
    ConsoleUpdated,
    IssuesChanged,
    PhaseChanged,
    /// Test results changed.
    TestsChanged,
    /// A compilation database was (re)written for sourcekit-lsp.
    CompileDatabaseUpdated(PathBuf),
    /// Run with debugging: a simulator app is waiting for the debugger.
    AttachDebugger { pid: i32 },
    /// Run with debugging: launch this executable under the debugger.
    LaunchDebugger { program: PathBuf, cwd: PathBuf },
}

pub struct BuildController {
    pub project: ProjectInfo,
    pub schemes: SchemeList,
    pub schemes_loading: bool,
    pub schemes_error: Option<String>,
    pub scheme: Option<String>,
    pub configuration: String,
    pub all_destinations: Vec<Destination>,
    pub destinations: Vec<Destination>,
    pub destination: Option<Destination>,
    settings: HashMap<(String, String), BuildSettings>,
    pub log: Vec<LogLine>,
    pub console: Vec<String>,
    pub issues: Vec<Issue>,
    /// The latest result of every test that has run, across runs.
    pub tests: Vec<TestResult>,
    /// Bumped whenever `tests` changes.
    pub tests_generation: u64,
    /// The scheme's test plans, and the one chosen (`None`: the default).
    pub test_plans: Vec<TestPlan>,
    pub test_plan: Option<String>,
    /// The selection of the current or last test run, for Test Again.
    pub last_test: Option<TestRequest>,
    /// Open Swift Testing suites, innermost last: its console names a
    /// test's suite only by printing the test between the suite's start
    /// and finish.
    suite_stack: Vec<String>,
    track_suites: bool,
    pub phase: Phase,
    pub reports: Vec<Report>,
    process: ProcessHandle,
    /// Background "index build" that produces the compilation database for
    /// projects that have never been built (like Xcode's prepare-for-indexing).
    pub indexing: bool,
    index_process: ProcessHandle,
    index_attempted: bool,
    _index_task: Option<Task<()>>,
    run_target: Option<(Destination, String)>,
    /// Whether the current run should attach the debugger.
    debug_run: bool,
    awaiting_attach: Option<String>,
    /// Events raised where no `Context` is available, emitted on next flush.
    pending_events: Vec<BuildEvent>,
    /// An action requested before the scheme list finished loading.
    queued: Option<(BuildAction, bool, Option<TestRequest>)>,
    cache_dir: PathBuf,
    task: Option<Task<()>>,
    _load_task: Option<Task<()>>,
    _plans_task: Option<Task<()>>,
}

impl EventEmitter<BuildEvent> for BuildController {}

impl BuildController {
    pub fn new(project: ProjectInfo, cache_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            project,
            schemes: SchemeList::default(),
            schemes_loading: false,
            schemes_error: None,
            scheme: None,
            configuration: "Debug".into(),
            all_destinations: vec![],
            destinations: vec![],
            destination: None,
            settings: HashMap::new(),
            log: vec![],
            console: vec![],
            issues: vec![],
            tests: vec![],
            tests_generation: 0,
            test_plans: vec![],
            test_plan: None,
            last_test: None,
            suite_stack: vec![],
            track_suites: false,
            phase: Phase::Idle,
            reports: vec![],
            process: ProcessHandle::default(),
            indexing: false,
            index_process: ProcessHandle::default(),
            index_attempted: false,
            _index_task: None,
            run_target: None,
            debug_run: false,
            awaiting_attach: None,
            pending_events: vec![],
            queued: None,
            cache_dir,
            task: None,
            _load_task: None,
            _plans_task: None,
        };
        this.reload(cx);
        this
    }

    pub fn can_build(&self) -> bool {
        !matches!(self.project.kind, ProjectKind::Folder)
    }

    /// Loads schemes and destinations in the background.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if !self.can_build() {
            return;
        }
        self.schemes_loading = true;
        let project = self.project.clone();
        self._load_task = Some(cx.spawn(async move |this, cx| {
            let (schemes, simulators) = cx
                .background_spawn(async move {
                    let schemes = xcode::list_schemes(&project);
                    let simulators = if project.kind.is_xcode() {
                        xcode::list_simulators().unwrap_or_default()
                    } else {
                        vec![]
                    };
                    (schemes, simulators)
                })
                .await;
            this.update(cx, |this, cx| {
                this.schemes_loading = false;
                match schemes {
                    Ok(schemes) => {
                        this.schemes_error = None;
                        if this
                            .scheme
                            .as_ref()
                            .is_none_or(|s| !schemes.schemes.contains(s))
                        {
                            this.scheme = schemes.schemes.first().cloned();
                        }
                        this.schemes = schemes;
                    }
                    Err(err) => this.schemes_error = Some(format!("{err:#}")),
                }
                let mut all = vec![Destination::my_mac()];
                all.extend(simulators);
                this.all_destinations = all;
                this.destinations = this.all_destinations.clone();
                if this.destination.is_none() {
                    this.destination = this.default_destination();
                }
                cx.emit(BuildEvent::ConfigurationChanged);
                cx.notify();
                this.refresh_supported_destinations(cx);
                this.load_test_plans(cx);
                if let Some((action, debug, tests)) = this.queued.take() {
                    this.debug_run = debug;
                    this.start_with(action, tests, cx);
                }
            })
            .ok();
        }));
    }

    fn default_destination(&self) -> Option<Destination> {
        match self.project.kind {
            ProjectKind::SwiftPackage => Some(Destination::my_mac()),
            _ => self
                .destinations
                .iter()
                .find(|d| d.platform == Platform::IOSSimulator && d.booted)
                .or_else(|| {
                    self.destinations
                        .iter()
                        .find(|d| d.platform == Platform::IOSSimulator && d.name.starts_with("iPhone"))
                })
                .or_else(|| self.destinations.first())
                .cloned(),
        }
    }

    /// Narrows destinations to what the scheme's SUPPORTED_PLATFORMS allows.
    fn refresh_supported_destinations(&mut self, cx: &mut Context<Self>) {
        let (Some(scheme), true) = (self.scheme.clone(), self.project.kind.is_xcode()) else {
            if matches!(self.project.kind, ProjectKind::SwiftPackage) {
                self.destinations = vec![Destination::my_mac()];
                self.destination = Some(Destination::my_mac());
            }
            return;
        };
        let project = self.project.clone();
        let configuration = self.configuration.clone();
        self._load_task = Some(cx.spawn(async move |this, cx| {
            let settings = cx
                .background_spawn(async move {
                    xcode::show_build_settings(&project, &scheme, &configuration, None)
                        .map(|all| (scheme.clone(), xcode::runnable_settings(&all, &scheme)))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Ok((scheme, Some(settings))) = settings {
                    let supported = settings.supported_platforms();
                    this.destinations = xcode::filter_destinations(&this.all_destinations, &supported);
                    let still_valid = this
                        .destination
                        .as_ref()
                        .is_some_and(|d| this.destinations.contains(d));
                    if !still_valid {
                        this.destination = this.default_destination();
                    }
                    this.settings.insert((scheme, "*".into()), settings);
                    cx.emit(BuildEvent::ConfigurationChanged);
                    cx.notify();
                }
                this.maybe_start_index_build(cx);
            })
            .ok();
        }));
    }

    pub fn set_scheme(&mut self, scheme: String, cx: &mut Context<Self>) {
        self.scheme = Some(scheme);
        cx.emit(BuildEvent::ConfigurationChanged);
        cx.notify();
        self.refresh_supported_destinations(cx);
        self.load_test_plans(cx);
    }

    /// Reads the scheme's test plans (from its `.xcscheme`) in the background.
    pub fn load_test_plans(&mut self, cx: &mut Context<Self>) {
        let (Some(scheme), true) = (self.scheme.clone(), self.project.kind.is_xcode()) else {
            self.test_plans.clear();
            return;
        };
        let kind = self.project.kind.clone();
        self._plans_task = Some(cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async move { plans::scheme_test_plans(&kind, &scheme) }).await;
            this.update(cx, |this, cx| {
                if this.test_plan.as_ref().is_some_and(|p| !found.iter().any(|f| f.name == *p)) {
                    this.test_plan = None;
                }
                this.test_plans = found;
                cx.emit(BuildEvent::ConfigurationChanged);
                cx.notify();
            })
            .ok();
        }));
    }

    /// The plan tests run with: the chosen one, else the scheme's default.
    pub fn active_test_plan(&self) -> Option<&TestPlan> {
        match &self.test_plan {
            Some(name) => self.test_plans.iter().find(|p| p.name == *name),
            None => self.test_plans.iter().find(|p| p.is_default).or(self.test_plans.first()),
        }
    }

    pub fn set_test_plan(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        self.test_plan = name;
        cx.emit(BuildEvent::ConfigurationChanged);
        cx.notify();
    }

    /// Product ▸ Test, or a selection of tests (a gutter diamond, the Test
    /// navigator, Test Again).
    pub fn test(&mut self, request: TestRequest, cx: &mut Context<Self>) {
        self.start_with(BuildAction::Test, Some(request), cx);
    }

    /// Product ▸ Clear Test Results.
    pub fn clear_test_results(&mut self, cx: &mut Context<Self>) {
        self.tests.clear();
        self.tests_generation += 1;
        cx.emit(BuildEvent::TestsChanged);
        cx.notify();
    }

    pub fn set_destination(&mut self, destination: Destination, cx: &mut Context<Self>) {
        self.destination = Some(destination);
        cx.emit(BuildEvent::ConfigurationChanged);
        cx.notify();
    }

    pub fn set_configuration(&mut self, configuration: String, cx: &mut Context<Self>) {
        self.configuration = configuration;
        cx.emit(BuildEvent::ConfigurationChanged);
        cx.notify();
    }

    pub fn error_count(&self) -> usize {
        self.issues.iter().filter(|i| i.severity == Severity::Error).count()
    }

    pub fn warning_count(&self) -> usize {
        self.issues.iter().filter(|i| i.severity == Severity::Warning).count()
    }

    pub fn clear_console(&mut self, cx: &mut Context<Self>) {
        self.console.clear();
        cx.emit(BuildEvent::ConsoleUpdated);
        cx.notify();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.process.terminate();
        if let Some((destination, bundle_id)) = self.run_target.take()
            && destination.platform.is_simulator()
        {
            let mut cmd = Command::new("/usr/bin/xcrun");
            cmd.args(["simctl", "terminate", &destination.id, &bundle_id]);
            cx.background_spawn(async move {
                process::run_quiet(cmd).await.ok();
            })
            .detach();
        }
        if self.phase.is_busy() {
            self.task = None;
            self.phase = Phase::Stopped;
            cx.emit(BuildEvent::PhaseChanged);
            cx.notify();
        }
    }

    fn maybe_start_index_build(&mut self, cx: &mut Context<Self>) {
        let db = compiledb::database_path(&self.cache_dir);
        if self.index_attempted || db.exists() {
            return;
        }
        self.start_index_build(cx);
    }

    /// Re-runs the index build after source edits are saved so cross-file
    /// features (references, rename, Open Quickly symbols) see them — what
    /// Xcode's background indexing does. Debounced by the caller.
    pub fn refresh_index(&mut self, cx: &mut Context<Self>) {
        if self.indexing {
            self.index_process.terminate();
            self._index_task = None;
            self.indexing = false;
        }
        self.start_index_build(cx);
    }

    fn start_index_build(&mut self, cx: &mut Context<Self>) {
        if !self.project.kind.is_xcode() || self.phase.is_busy() {
            return;
        }
        let (Some(scheme), Some(destination)) = (self.scheme.clone(), self.destination.clone()) else {
            return;
        };
        self.index_attempted = true;
        self.indexing = true;
        cx.notify();
        let mut cmd = build_command(&self.project, &scheme, &self.configuration, &destination, BuildAction::Build);
        // Run at lower CPU priority so editing stays smooth.
        // A private derived-data directory guarantees a full (logged) compile,
        // the way Xcode keeps its own Index.noindex build.
        let mut niced = Command::new("/usr/bin/nice");
        niced
            .arg("-n")
            .arg("10")
            .arg(cmd.get_program())
            .args(cmd.get_args())
            .arg("-derivedDataPath")
            .arg(self.cache_dir.join("IndexDerivedData"));
        if let Some(dir) = cmd.get_current_dir() {
            niced.current_dir(dir);
        }
        cmd = niced;
        let handle = ProcessHandle::default();
        self.index_process = handle.clone();
        let cache_dir = self.cache_dir.clone();
        self._index_task = Some(cx.spawn(async move |this, cx| {
            let (tx, rx) = channel::unbounded::<String>();
            let process = cx.background_spawn(process::run_streaming(cmd, tx, handle));
            let collect = cx.background_spawn(async move {
                let mut raw = String::new();
                while let Ok(line) = rx.recv().await {
                    raw.push_str(&line);
                    raw.push('\n');
                }
                raw
            });
            let code = process.await.ok().flatten();
            let raw = collect.await;
            let db = compiledb::database_path(&cache_dir);
            let changed = cx
                .background_spawn({
                    let db = db.clone();
                    let store = compiledb::index_store_path(&cache_dir);
                    async move {
                        let commands = compiledb::with_index_store(compiledb::with_canonical_paths(compiledb::extract(&raw)), &store);
                        compiledb::merge_into(&db, commands).unwrap_or(false)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.indexing = false;
                ::log::info!("index build finished (exit {code:?}, database updated: {changed})");
                if changed {
                    cx.emit(BuildEvent::CompileDatabaseUpdated(db));
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Product ▸ Run, under the debugger when `debug` (breakpoints exist).
    pub fn run(&mut self, debug: bool, cx: &mut Context<Self>) {
        self.debug_run = debug;
        self.start(BuildAction::Run, cx);
    }

    /// Ends a run that the debugger owned (Mac executables launched by lldb-dap).
    pub fn finish_debug_run(&mut self, exit_code: Option<i64>, cx: &mut Context<Self>) {
        if let Phase::Running { started } = self.phase.clone() {
            self.push_console_line(match exit_code {
                Some(code) => format!("Program ended with exit code: {code}"),
                None => "Debugging session ended".to_string(),
            });
            self.phase = Phase::Finished {
                action: BuildAction::Run,
                success: true,
                duration: started.elapsed(),
            };
            cx.emit(BuildEvent::ConsoleUpdated);
            cx.emit(BuildEvent::PhaseChanged);
            cx.notify();
        }
    }

    /// Debugger output goes to the same console as program output.
    pub fn append_console(&mut self, line: String, cx: &mut Context<Self>) {
        self.push_console_line(line);
        cx.emit(BuildEvent::ConsoleUpdated);
        cx.notify();
    }

    /// Product ▸ Build / Run / Test / Clean.
    pub fn start(&mut self, action: BuildAction, cx: &mut Context<Self>) {
        self.start_with(action, None, cx);
    }

    /// Starts `action`; a test run runs `tests` (everything when `None`).
    fn start_with(&mut self, action: BuildAction, tests: Option<TestRequest>, cx: &mut Context<Self>) {
        if !self.can_build() {
            return;
        }
        let tests = (action == BuildAction::Test).then(|| tests.unwrap_or_default());
        if self.indexing {
            // The user's build supersedes (and will produce) the index data.
            self.index_process.terminate();
            self.indexing = false;
            self._index_task = None;
        }
        if self.phase.is_busy() {
            self.stop(cx);
        }
        let Some(scheme) = self.scheme.clone() else {
            if self.schemes_loading {
                // Run as soon as the project's schemes are known.
                self.queued = Some((action, self.debug_run, tests));
                self.phase = Phase::Preparing(action);
                cx.emit(BuildEvent::PhaseChanged);
            } else {
                self.push_log_line("error: No scheme selected".into());
            }
            cx.notify();
            return;
        };
        let destination = self
            .destination
            .clone()
            .unwrap_or_else(Destination::my_mac);
        self.log.clear();
        self.issues.clear();
        if let Some(request) = &tests {
            // Results of tests this run doesn't include stay, as in Xcode.
            if request.is_all() {
                self.tests.clear();
            } else {
                self.tests.retain(|t| !request.only.iter().any(|s| s.covers(t)));
            }
            self.tests_generation += 1;
            self.suite_stack.clear();
            self.last_test = Some(request.clone());
            cx.emit(BuildEvent::TestsChanged);
        }
        let test_plan = self
            .active_test_plan()
            .filter(|_| tests.is_some() && self.project.kind.is_xcode())
            .map(|p| p.name.clone());
        if action == BuildAction::Run {
            self.console.clear();
            cx.emit(BuildEvent::ConsoleUpdated);
        }
        self.phase = Phase::Preparing(action);
        cx.emit(BuildEvent::IssuesChanged);
        cx.emit(BuildEvent::LogUpdated);
        cx.emit(BuildEvent::PhaseChanged);
        cx.notify();

        let project = self.project.clone();
        let configuration = self.configuration.clone();
        let handle = ProcessHandle::default();
        self.process = handle.clone();
        let cache_dir = self.cache_dir.clone();
        let debug = action == BuildAction::Run && std::mem::take(&mut self.debug_run);
        let cached_settings = self
            .settings
            .get(&(scheme.clone(), destination.id.clone()))
            .cloned();

        self.task = Some(cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let tool_action = if action == BuildAction::Run {
                BuildAction::Build
            } else {
                action
            };
            // Unit tests on a simulator run in-process (see `sim_tests`):
            // build-for-testing here, then inject each bundle into its host.
            let direct_tests = action == BuildAction::Test
                && project.kind.is_xcode()
                && destination.platform.is_simulator()
                && !tests.as_ref().is_some_and(TestRequest::only_ui_tests);
            let mut cmd = if direct_tests {
                xcodebuild_command(&project, &scheme, &configuration, &destination, "build-for-testing")
            } else {
                build_command(&project, &scheme, &configuration, &destination, tool_action)
            };
            if let Some(plan) = &test_plan {
                cmd.args(["-testPlan", plan]);
            }
            if let (Some(request), false) = (&tests, direct_tests) {
                match project.kind {
                    ProjectKind::SwiftPackage => cmd.args(request.swiftpm_args()),
                    _ => cmd.args(request.xcodebuild_args()),
                };
            }
            // Swift Testing's console order tells suites apart when tests
            // run one at a time: always when XCTest hosts them (Xcode
            // projects), and with `swift test --no-parallel`.
            let track_suites = project.kind.is_xcode() || tests.as_ref().is_some_and(|r| r.serial);

            // Resolve run settings concurrently with the build.
            let settings_task = (action == BuildAction::Run && project.kind.is_xcode()).then(|| {
                let project = project.clone();
                let scheme = scheme.clone();
                let configuration = configuration.clone();
                let destination = destination.clone();
                cx.background_spawn(async move {
                    if let Some(settings) = cached_settings {
                        return Ok(settings);
                    }
                    let all = xcode::show_build_settings(
                        &project,
                        &scheme,
                        &configuration,
                        Some(&destination),
                    )?;
                    xcode::runnable_settings(&all, &scheme)
                        .ok_or_else(|| anyhow::anyhow!("scheme {scheme} has no runnable target"))
                })
            });

            this.update(cx, |this, cx| {
                this.phase = Phase::Building { action, started };
                this.track_suites = track_suites;
                this.push_log_line(format!("$ {}", display_command(&cmd)));
                cx.emit(BuildEvent::PhaseChanged);
                cx.notify();
            })
            .ok();

            let (code, mut raw_log) = stream_into(&this, cx, cmd, handle.clone(), Sink::Log).await;
            let mut success = code == Some(0);
            if direct_tests && success && this.read_with(cx, |this, _| this.phase != Phase::Stopped).unwrap_or(false) {
                let request = tests.clone().unwrap_or_default();
                let (ok, output) = run_simulator_tests(
                    &this, cx, &project, &scheme, &configuration, &destination, &request, &cache_dir, &handle,
                )
                .await;
                success = ok;
                raw_log.push_str(&output);
            }
            let duration = started.elapsed();

            // Persist the log and extract compiler invocations for the LSP.
            let log_path = cache_dir.join("logs").join(format!(
                "{}-{}.log",
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default(),
                action.verb().to_lowercase()
            ));
            let is_xcode = project.kind.is_xcode();
            let db_path = compiledb::database_path(&cache_dir);
            let db_changed = cx
                .background_spawn({
                    let log_path = log_path.clone();
                    let db_path = db_path.clone();
                    async move {
                        if let Some(dir) = log_path.parent() {
                            std::fs::create_dir_all(dir).ok();
                        }
                        std::fs::write(&log_path, &raw_log).ok();
                        if is_xcode && tool_action != BuildAction::Clean {
                            let commands = compiledb::extract(&raw_log);
                            let store = compiledb::index_store_path(&cache_dir);
                            let commands = compiledb::with_index_store(compiledb::with_canonical_paths(commands), &store);
                            compiledb::merge_into(&db_path, commands).unwrap_or(false)
                        } else {
                            false
                        }
                    }
                })
                .await;

            let continue_to_run = this
                .update(cx, |this, cx| {
                    if db_changed {
                        cx.emit(BuildEvent::CompileDatabaseUpdated(db_path.clone()));
                    }
                    let stopped = this.phase == Phase::Stopped;
                    this.reports.insert(
                        0,
                        Report {
                            action,
                            scheme: scheme.clone(),
                            destination: destination.name.clone(),
                            finished_at: SystemTime::now(),
                            duration,
                            success,
                            errors: this.error_count(),
                            warnings: this.warning_count(),
                            log_path: log_path.clone(),
                        },
                    );
                    if stopped {
                        return false;
                    }
                    if action != BuildAction::Run || !success {
                        this.phase = Phase::Finished {
                            action: tool_action,
                            success,
                            duration,
                        };
                        cx.emit(BuildEvent::PhaseChanged);
                        cx.notify();
                        return false;
                    }
                    this.phase = Phase::Launching;
                    cx.emit(BuildEvent::PhaseChanged);
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !continue_to_run {
                return;
            }

            // Launch.
            let launch = match &project.kind {
                ProjectKind::SwiftPackage => {
                    let root = project.root.clone();
                    let scheme = scheme.clone();
                    let configuration = configuration.to_lowercase();
                    cx.background_spawn(async move {
                        let mut cmd = Command::new("/usr/bin/swift");
                        cmd.args(["build", "-c", &configuration, "--show-bin-path"])
                            .current_dir(&root);
                        let (_, out) = process::run_quiet(cmd).await?;
                        let bin = PathBuf::from(out.lines().last().unwrap_or("").trim()).join(&scheme);
                        let mut run = Command::new(bin);
                        run.current_dir(&root);
                        anyhow::Ok((run, None))
                    })
                    .await
                }
                _ => {
                    let settings = match settings_task {
                        Some(task) => task.await,
                        None => Err(anyhow::anyhow!("no build settings")),
                    };
                    match settings {
                        Ok(settings) => {
                            this.update(cx, |this, _| {
                                this.settings.insert(
                                    (scheme.clone(), destination.id.clone()),
                                    settings.clone(),
                                );
                            })
                            .ok();
                            let destination = destination.clone();
                            let console = channel::unbounded::<String>();
                            let lines_tx = console.0.clone();
                            let prepared = cx
                                .background_spawn(async move {
                                    prepare_launch(&destination, &settings, debug, lines_tx).await
                                })
                                .await;
                            // Surface preparation messages in the console.
                            let mut messages = vec![];
                            while let Ok(line) = console.1.try_recv() {
                                messages.push(line);
                            }
                            this.update(cx, |this, cx| {
                                for message in messages {
                                    this.push_console_line(message);
                                }
                                cx.emit(BuildEvent::ConsoleUpdated);
                                cx.notify();
                            })
                            .ok();
                            prepared
                        }
                        Err(err) => Err(err),
                    }
                }
            };

            let (cmd, target) = match launch {
                Ok(launch) => launch,
                Err(err) => {
                    this.update(cx, |this, cx| {
                        this.push_console_line(format!("Launch failed: {err:#}"));
                        this.issues.push(Issue {
                            severity: Severity::Error,
                            message: format!("Launch failed: {err:#}"),
                            path: None,
                            line: None,
                            column: None,
                            source: log::IssueSource::Build,
                        });
                        this.phase = Phase::Finished {
                            action: BuildAction::Run,
                            success: false,
                            duration: started.elapsed(),
                        };
                        cx.emit(BuildEvent::IssuesChanged);
                        cx.emit(BuildEvent::ConsoleUpdated);
                        cx.emit(BuildEvent::PhaseChanged);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };

            let run_started = Instant::now();
            let launch_under_debugger = debug && target.is_none();
            this.update(cx, |this, cx| {
                this.awaiting_attach = if debug { target.clone() } else { None };
                this.run_target = target.map(|bundle| (destination.clone(), bundle));
                this.phase = Phase::Running {
                    started: run_started,
                };
                if launch_under_debugger {
                    let program = PathBuf::from(cmd.get_program());
                    let cwd = cmd
                        .get_current_dir()
                        .map(PathBuf::from)
                        .or_else(|| program.parent().map(PathBuf::from))
                        .unwrap_or_default();
                    this.push_console_line(format!("Debugging {}", program.display()));
                    cx.emit(BuildEvent::LaunchDebugger { program, cwd });
                }
                cx.emit(BuildEvent::PhaseChanged);
                cx.notify();
            })
            .ok();
            if launch_under_debugger {
                // lldb-dap owns the process; the workspace ends the run.
                return;
            }

            let (code, _) = stream_into(&this, cx, cmd, handle, Sink::Console).await;
            this.update(cx, |this, cx| {
                if matches!(this.phase, Phase::Running { .. }) {
                    this.push_console_line(match code {
                        Some(code) => format!("Program ended with exit code: {code}"),
                        None => "Program terminated".to_string(),
                    });
                    this.phase = Phase::Finished {
                        action: BuildAction::Run,
                        success: true,
                        duration: run_started.elapsed(),
                    };
                    this.run_target = None;
                    cx.emit(BuildEvent::ConsoleUpdated);
                    cx.emit(BuildEvent::PhaseChanged);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn push_log_line(&mut self, raw: String) {
        let parsed = log::parse_line(&raw);
        if let Some(issue) = parsed.issue
            && !self.issues.contains(&issue)
        {
            self.issues.push(issue);
        }
        match parsed.suite {
            Some(SuiteEvent::Started(name)) => self.suite_stack.push(name),
            Some(SuiteEvent::Finished(name)) => {
                if let Some(ix) = self.suite_stack.iter().rposition(|s| *s == name) {
                    self.suite_stack.truncate(ix);
                }
            }
            None => {}
        }
        if let Some(mut test) = parsed.test {
            if self.track_suites && test.suites.is_empty() && test.module.is_none() {
                test.suites = self.suite_stack.clone();
            }
            match self.tests.iter_mut().find(|t| t.same_test(&test)) {
                // Keep the module when only the first report named it.
                Some(existing) => {
                    test.module = test.module.or(existing.module.take());
                    *existing = test;
                }
                None => self.tests.push(test),
            }
            self.tests_generation += 1;
        }
        self.log.push(log::prettify(&raw));
        if self.log.len() > MAX_LOG_LINES {
            self.log.drain(..self.log.len() - MAX_LOG_LINES);
        }
    }

    fn push_console_line(&mut self, line: String) {
        // `simctl launch` prints "<bundle id>: <pid>" once the app is
        // waiting for a debugger.
        if let Some(bundle) = self.awaiting_attach.clone()
            && let Some(pid) = line
                .strip_prefix(&format!("{bundle}: "))
                .and_then(|p| p.trim().parse::<i32>().ok())
        {
            self.awaiting_attach = None;
            self.pending_events.push(BuildEvent::AttachDebugger { pid });
        }
        self.console.push(line);
        if self.console.len() > MAX_CONSOLE_LINES {
            self.console.drain(..self.console.len() - MAX_CONSOLE_LINES);
        }
    }

    pub fn failed_tests(&self) -> usize {
        self.tests
            .iter()
            .filter(|t| t.status == TestStatus::Failed)
            .count()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Sink {
    Log,
    Console,
}

/// Streams a process into the build log or console, batching UI updates to
/// at most ~30 per second. Returns the exit code and the raw output.
async fn stream_into(
    this: &gpui_kit::WeakEntity<BuildController>,
    cx: &mut gpui_kit::AsyncApp,
    cmd: Command,
    handle: ProcessHandle,
    sink: Sink,
) -> (Option<i32>, String) {
    let (tx, rx) = channel::unbounded::<String>();
    let process = cx.background_spawn(process::run_streaming(cmd, tx, handle));
    let mut raw = String::new();
    while let Ok(first) = rx.recv().await {
        let mut batch = vec![first];
        cx.background_executor()
            .timer(Duration::from_millis(33))
            .await;
        while let Ok(line) = rx.try_recv() {
            batch.push(line);
        }
        if sink == Sink::Log {
            for line in &batch {
                raw.push_str(line);
                raw.push('\n');
            }
        }
        let alive = this
            .update(cx, |this, cx| {
                let had_issues = this.issues.len();
                for line in batch {
                    match sink {
                        Sink::Log => this.push_log_line(line),
                        Sink::Console => this.push_console_line(line),
                    }
                }
                match sink {
                    Sink::Log => cx.emit(BuildEvent::LogUpdated),
                    Sink::Console => cx.emit(BuildEvent::ConsoleUpdated),
                }
                for event in std::mem::take(&mut this.pending_events) {
                    cx.emit(event);
                }
                if this.issues.len() != had_issues {
                    cx.emit(BuildEvent::IssuesChanged);
                }
                cx.notify();
            })
            .is_ok();
        if !alive {
            break;
        }
    }
    match process.await {
        Ok(code) => (code, raw),
        Err(err) => {
            this.update(cx, |this, cx| {
                this.push_log_line(format!("error: {err:#}"));
                cx.emit(BuildEvent::LogUpdated);
                cx.emit(BuildEvent::IssuesChanged);
            })
            .ok();
            (Some(-1), raw)
        }
    }
}

fn display_command(cmd: &Command) -> String {
    std::iter::once(cmd.get_program().to_string_lossy().to_string())
        .chain(cmd.get_args().map(|a| {
            let a = a.to_string_lossy();
            if a.contains(' ') {
                format!("'{a}'")
            } else {
                a.to_string()
            }
        }))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Runs the unit-test bundles of a just-built scheme inside the simulator and
/// streams their output into the build log. Returns (success, raw output).
#[allow(clippy::too_many_arguments)]
async fn run_simulator_tests(
    this: &gpui_kit::WeakEntity<BuildController>,
    cx: &mut gpui_kit::AsyncApp,
    project: &ProjectInfo,
    scheme: &str,
    configuration: &str,
    destination: &Destination,
    request: &TestRequest,
    cache_dir: &std::path::Path,
    handle: &ProcessHandle,
) -> (bool, String) {
    let say = |this: &gpui_kit::WeakEntity<BuildController>, cx: &mut gpui_kit::AsyncApp, line: String| {
        this.update(cx, |this, cx| {
            this.push_log_line(line);
            cx.emit(BuildEvent::LogUpdated);
            cx.emit(BuildEvent::IssuesChanged);
            cx.notify();
        })
        .ok();
    };
    let targets = {
        let (project, scheme, configuration, destination) =
            (project.clone(), scheme.to_string(), configuration.to_string(), destination.clone());
        cx.background_spawn(async move {
            let settings = xcode::show_build_settings(&project, &scheme, &configuration, Some(&destination))?;
            let build_dir = settings
                .iter()
                .find_map(|s| s.get("BUILD_DIR").map(PathBuf::from))
                .ok_or_else(|| anyhow::anyhow!("BUILD_DIR not found"))?;
            let xctestrun = sim_tests::find_xctestrun(&build_dir, &scheme)
                .ok_or_else(|| anyhow::anyhow!("no .xctestrun in {}", build_dir.display()))?;
            // Boot the simulator if needed (an error just means it's booted).
            let mut boot = Command::new("/usr/bin/xcrun");
            boot.args(["simctl", "boot", &destination.id]);
            boot.output().ok();
            sim_tests::parse_xctestrun(&xctestrun)
        })
        .await
    };
    let targets = match targets {
        Ok(targets) => targets,
        Err(err) => {
            say(this, cx, format!("error: Could not prepare tests: {err:#}"));
            return (false, String::new());
        }
    };
    let mut ok = true;
    let mut raw = String::new();
    let mut ran = 0;
    for target in targets {
        // An explicit selection replaces the plan's; otherwise the bundle's
        // own selection (scheme or test plan) applies.
        let scope = match request.scope_for_bundle(&target.name, &target.module) {
            None => continue,
            Some(entries) if entries.is_empty() => sim_tests::Scope::for_target(&target),
            Some(entries) => sim_tests::Scope { entries, invert: false },
        };
        if target.is_ui_test {
            say(this, cx, format!("note: Skipping UI test bundle {} (UI tests run through xcodebuild test)", target.name));
            continue;
        }
        let title = match &target.configuration {
            Some(configuration) => format!("{} ({configuration})", target.name),
            None => target.name.clone(),
        };
        say(this, cx, format!("Testing {title}…"));
        this.update(cx, |this, _| this.suite_stack.clear()).ok();
        let udid = destination.id.clone();
        let scope_file = (!scope.is_all()).then(|| cache_dir.join("tests").join(format!("{}.scope.plist", target.module)));
        let prepared = cx
            .background_spawn({
                let target = target.clone();
                let scope_file = scope_file.clone();
                async move {
                    if let Some(file) = &scope_file {
                        if let Some(dir) = file.parent() {
                            std::fs::create_dir_all(dir)?;
                        }
                        std::fs::write(file, scope.plist())?;
                    }
                    sim_tests::test_command(&udid, &target, scope_file.as_deref())
                }
            })
            .await;
        let cmd = match prepared {
            Ok(cmd) => cmd,
            Err(err) => {
                say(this, cx, format!("error: {}: {err:#}", target.name));
                ok = false;
                continue;
            }
        };
        ran += 1;
        let (code, output) = stream_into(this, cx, cmd, handle.clone(), Sink::Log).await;
        raw.push_str(&output);
        let finished = output.contains("Test run with")
            || output.contains("Test Suite 'All tests'")
            || output.contains("Test Suite 'Selected tests'");
        if code != Some(0) || !finished {
            ok = false;
            say(this, cx, format!("error: {} did not finish running tests", target.name));
        }
    }
    if ran == 0 && !request.is_all() {
        say(this, cx, "error: The selected tests are in no test bundle of this scheme".into());
        ok = false;
    }
    // Earlier results of tests outside this run don't count.
    let failed = this
        .read_with(cx, |this, _| {
            this.tests
                .iter()
                .filter(|t| t.status == TestStatus::Failed && (request.is_all() || request.only.iter().any(|s| s.covers(t))))
                .count()
        })
        .unwrap_or(0);
    ok &= failed == 0;
    say(this, cx, if ok { "** TEST SUCCEEDED **".into() } else { "** TEST FAILED **".into() });
    (ok, raw)
}

fn xcodebuild_command(
    project: &ProjectInfo,
    scheme: &str,
    configuration: &str,
    destination: &Destination,
    verb: &str,
) -> Command {
    let mut cmd = Command::new("/usr/bin/xcodebuild");
    cmd.current_dir(&project.root)
        .args(project.kind.xcodebuild_args())
        .args(["-scheme", scheme, "-configuration", configuration])
        .args(["-destination", &destination.xcodebuild_specifier()])
        .args(["-skipMacroValidation", "-skipPackagePluginValidation"])
        .arg(verb);
    cmd
}

pub fn build_command(
    project: &ProjectInfo,
    scheme: &str,
    configuration: &str,
    destination: &Destination,
    action: BuildAction,
) -> Command {
    match &project.kind {
        ProjectKind::SwiftPackage => {
            let mut cmd = Command::new("/usr/bin/swift");
            cmd.current_dir(&project.root);
            match action {
                BuildAction::Build | BuildAction::Run => {
                    cmd.args(["build", "-c", &configuration.to_lowercase()]);
                    if !scheme.is_empty() {
                        cmd.args(["--product", scheme]);
                    }
                }
                BuildAction::Test => {
                    cmd.arg("test");
                }
                BuildAction::Clean => {
                    cmd.args(["package", "clean"]);
                }
            }
            cmd
        }
        _ => xcodebuild_command(
            project,
            scheme,
            configuration,
            destination,
            match action {
                BuildAction::Build | BuildAction::Run => "build",
                BuildAction::Test => "test",
                BuildAction::Clean => "clean",
            },
        ),
    }
}

/// Boots/installs as needed and returns the command whose output is the
/// app's console, plus the bundle id when running in a simulator.
async fn prepare_launch(
    destination: &Destination,
    settings: &BuildSettings,
    debug: bool,
    messages: channel::Sender<String>,
) -> anyhow::Result<(Command, Option<String>)> {
    let say = |m: String| {
        messages.try_send(m).ok();
    };
    match destination.platform {
        Platform::MacOS => {
            let exe = settings
                .executable_path()
                .ok_or_else(|| anyhow::anyhow!("missing EXECUTABLE_PATH"))?;
            say(format!("Launching {}", exe.display()));
            Ok((Command::new(exe), None))
        }
        _ => {
            let app = settings
                .product_path()
                .ok_or_else(|| anyhow::anyhow!("missing TARGET_BUILD_DIR"))?;
            let bundle_id = settings
                .bundle_identifier()
                .ok_or_else(|| anyhow::anyhow!("missing PRODUCT_BUNDLE_IDENTIFIER"))?
                .to_string();
            let udid = destination.id.clone();

            say(format!("Booting {} ({})…", destination.name, destination.runtime));
            let mut boot = Command::new("/usr/bin/xcrun");
            boot.args(["simctl", "boot", &udid]);
            let (ok, out) = process::run_quiet(boot).await?;
            if !ok && !out.contains("current state: Booted") {
                anyhow::bail!("simctl boot failed: {}", out.trim());
            }

            let simulator_app = crate::toolchain::simulator_app();
            match &simulator_app {
                Some(app) => {
                    let mut open = Command::new("/usr/bin/open");
                    open.arg("-g").arg("-a").arg(app).args(["--args", "-CurrentDeviceUDID", &udid]);
                    if let Ok((false, out)) = process::run_quiet(open).await {
                        say(format!("warning: Could not open {}: {}", app.display(), out.trim()));
                    }
                }
                None => say("warning: No Simulator app found in the active Xcode".to_string()),
            }

            say(format!("Installing {}…", app.file_name().unwrap_or_default().to_string_lossy()));
            let mut install = Command::new("/usr/bin/xcrun");
            install.args(["simctl", "install", &udid]).arg(&app);
            let (ok, out) = process::run_quiet(install).await?;
            if !ok {
                anyhow::bail!("simctl install failed: {}", out.trim());
            }

            if let Some(app) = &simulator_app {
                let mut bring_front = Command::new("/usr/bin/open");
                bring_front.arg("-a").arg(app);
                process::run_quiet(bring_front).await.ok();
            }

            say(format!("Launching {bundle_id}"));
            let mut launch = Command::new("/usr/bin/xcrun");
            launch.args(["simctl", "launch", "--console-pty", "--terminate-running-process"]);
            if debug {
                launch.arg("--wait-for-debugger");
            }
            launch.args([&udid, &bundle_id]);
            Ok((launch, Some(bundle_id)))
        }
    }
}

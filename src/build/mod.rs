//! Build, run, test and clean — the Product menu.

pub mod compiledb;
pub mod devices;
pub mod log;
pub mod process;
pub mod profile;
pub mod sim_tests;
pub mod xcode;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::{AppContext as _, Context, EventEmitter, Task};
use smol::channel;

use crate::project::{ProjectInfo, ProjectKind};
use crate::scheme::{self, Launch, SchemeOptions};
use crate::testing::TestRequest;
use crate::testing::plans::{self, TestPlan};
use log::{Issue, LogLine, Severity, SuiteEvent, TestResult, TestStatus};
use process::ProcessHandle;
use xcode::{BuildSettings, Destination, Platform, SchemeList};

/// Lines kept in memory per log; older lines are dropped from the view (the
/// full log is still written to disk).
const MAX_LOG_LINES: usize = 50_000;
const MAX_CONSOLE_LINES: usize = 20_000;
/// How often connected devices are listed while the window is active.
const DEVICE_POLL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildAction {
    Build,
    Run,
    Test,
    /// Build with the Profile action's configuration and record under
    /// Instruments (`profile`).
    Profile,
    Clean,
}

impl BuildAction {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Build | Self::Run => "Build",
            Self::Test => "Test",
            Self::Profile => "Profile",
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
    /// Run with debugging: an app launched suspended on a device.
    AttachDeviceDebugger(devices::Attach),
    /// Run with debugging: launch this executable under the debugger.
    LaunchDebugger { program: PathBuf, cwd: PathBuf, args: Vec<String>, env: Vec<(String, String)> },
    /// Options kept in the session (packages, file-less schemes) changed.
    StoredSchemeOptionsChanged,
    /// Instruments' templates are listed (or couldn't be).
    TemplatesLoaded,
}

pub struct BuildController {
    pub project: ProjectInfo,
    pub schemes: SchemeList,
    pub schemes_loading: bool,
    pub schemes_error: Option<String>,
    pub scheme: Option<String>,
    /// The Run action's build configuration (Build and Run use it).
    pub configuration: String,
    /// The current scheme's Edit Scheme options, as last read.
    pub scheme_options: Option<scheme::Loaded>,
    /// Options of schemes without a scheme file, persisted in the session.
    pub stored_options: BTreeMap<String, SchemeOptions>,
    pub all_destinations: Vec<Destination>,
    pub destinations: Vec<Destination>,
    pub destination: Option<Destination>,
    /// The scheme's SUPPORTED_PLATFORMS (empty until known).
    supported_platforms: Vec<String>,
    /// Devices are polled only while the project's window is active.
    window_active: bool,
    _device_poll: Option<Task<()>>,
    _devices_task: Option<Task<()>>,
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
    /// Bumped by every build, run, test or clean started, so whoever waits
    /// for one can tell it from the next.
    pub generation: u64,
    pub reports: Vec<Report>,
    process: ProcessHandle,
    /// Background "index build" that produces the compilation database for
    /// projects that have never been built (like Xcode's prepare-for-indexing).
    pub indexing: bool,
    index_process: ProcessHandle,
    index_attempted: bool,
    _index_task: Option<Task<()>>,
    run_target: Option<RunTarget>,
    /// Whether the current run should attach the debugger.
    debug_run: bool,
    awaiting_attach: Option<String>,
    /// Finds a device app's process for the debugger to attach to.
    _attach_task: Option<Task<()>>,
    /// Events raised where no `Context` is available, emitted on next flush.
    pending_events: Vec<BuildEvent>,
    /// An action requested before the scheme list finished loading.
    queued: Option<(BuildAction, bool, Option<TestRequest>)>,
    cache_dir: PathBuf,
    /// What the next or current Profile action records.
    profile_request: Option<profile::ProfileRequest>,
    pub recording: Option<profile::Recording>,
    pub last_profile: Option<profile::Outcome>,
    pub templates: profile::Templates,
    /// The template last recorded with, first in the chooser.
    pub last_template: Option<String>,
    _templates_task: Option<Task<()>>,
    task: Option<Task<()>>,
    _load_task: Option<Task<()>>,
    _plans_task: Option<Task<()>>,
}

impl EventEmitter<BuildEvent> for BuildController {}

/// A closed window stops what it built and ran, as Xcode does.
impl Drop for BuildController {
    fn drop(&mut self) {
        self.end_processes();
    }
}

impl BuildController {
    pub fn new(project: ProjectInfo, cache_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            project,
            schemes: SchemeList::default(),
            schemes_loading: false,
            schemes_error: None,
            scheme: None,
            configuration: "Debug".into(),
            scheme_options: None,
            stored_options: BTreeMap::new(),
            all_destinations: vec![],
            destinations: vec![],
            destination: None,
            supported_platforms: vec![],
            window_active: true,
            _device_poll: None,
            _devices_task: None,
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
            generation: 0,
            reports: vec![],
            process: ProcessHandle::default(),
            indexing: false,
            index_process: ProcessHandle::default(),
            index_attempted: false,
            _index_task: None,
            run_target: None,
            debug_run: false,
            awaiting_attach: None,
            _attach_task: None,
            pending_events: vec![],
            queued: None,
            cache_dir,
            profile_request: None,
            recording: None,
            last_profile: None,
            templates: profile::Templates::default(),
            last_template: None,
            _templates_task: None,
            task: None,
            _load_task: None,
            _plans_task: None,
        };
        this.reload(cx);
        this.poll_devices(cx);
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
            let (schemes, simulators, devices) = cx
                .background_spawn(async move {
                    if !project.kind.is_xcode() {
                        return (xcode::list_schemes(&project), vec![], vec![]);
                    }
                    std::thread::scope(|scope| {
                        let devices = scope.spawn(|| devices::list().unwrap_or_default());
                        let simulators = scope.spawn(|| xcode::list_simulators().unwrap_or_default());
                        let schemes = xcode::list_schemes(&project);
                        (schemes, simulators.join().unwrap_or_default(), devices.join().unwrap_or_default())
                    })
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
                if this.project.kind.is_xcode() {
                    all.extend(devices);
                    all.extend(Platform::DEVICES.map(Destination::any_device));
                }
                all.extend(simulators);
                this.all_destinations = all;
                this.filter_destinations();
                cx.emit(BuildEvent::ConfigurationChanged);
                cx.notify();
                this.refresh_supported_destinations(cx);
                this.load_scheme(cx);
                if let Some((action, debug, tests)) = this.queued.take() {
                    this.debug_run = debug;
                    this.begin(action, tests, cx);
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
                    this.supported_platforms = settings.supported_platforms();
                    this.filter_destinations();
                    this.settings.insert((scheme, "*".into()), settings);
                    cx.emit(BuildEvent::ConfigurationChanged);
                    cx.notify();
                }
                this.maybe_start_index_build(cx);
            })
            .ok();
        }));
    }

    /// Narrows destinations to the scheme's platforms, keeping the chosen
    /// one (refreshed, by id) while it is still there.
    fn filter_destinations(&mut self) {
        self.destinations = xcode::filter_destinations(&self.all_destinations, &self.supported_platforms);
        let current = self.destination.as_ref().and_then(|d| self.destinations.iter().find(|n| n.id == d.id)).cloned();
        self.destination = current.or_else(|| self.default_destination());
    }

    /// Whether the scheme can run on a device (unknown counts as yes).
    fn wants_devices(&self) -> bool {
        self.project.kind.is_xcode()
            && (self.supported_platforms.is_empty()
                || Platform::DEVICES.iter().any(|p| self.supported_platforms.iter().any(|s| s == p.sdk())))
    }

    /// The project's window became active or inactive. Devices are listed
    /// right away on activation (one may have been plugged in meanwhile).
    pub fn set_window_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.window_active = active;
        if active {
            self.refresh_devices(cx);
        }
    }

    /// Lists devices every few seconds while the window is active; devicectl
    /// has no way to watch for connections.
    fn poll_devices(&mut self, cx: &mut Context<Self>) {
        if !self.project.kind.is_xcode() {
            return;
        }
        self._device_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(DEVICE_POLL).await;
                if this.update(cx, |this, cx| this.refresh_devices(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Re-lists connected devices in the background.
    pub fn refresh_devices(&mut self, cx: &mut Context<Self>) {
        // The first list comes with the simulators (`reload`).
        let loaded = !self.all_destinations.is_empty();
        if !loaded || !self.window_active || !self.wants_devices() || self._devices_task.is_some() {
            return;
        }
        self._devices_task = Some(cx.spawn(async move |this, cx| {
            let devices = cx.background_spawn(async move { devices::list() }).await;
            this.update(cx, |this, cx| {
                this._devices_task = None;
                match devices {
                    Ok(devices) => this.set_devices(devices, cx),
                    Err(err) => ::log::debug!("devicectl list devices: {err:#}"),
                }
            })
            .ok();
        }));
    }

    fn set_devices(&mut self, devices: Vec<Destination>, cx: &mut Context<Self>) {
        let current: Vec<&Destination> = self.all_destinations.iter().filter(|d| d.device.is_some()).collect();
        if current.len() == devices.len() && current.iter().zip(&devices).all(|(a, b)| *a == b) {
            return;
        }
        // Devices sit right after My Mac, as in Xcode's menu.
        self.all_destinations.retain(|d| d.device.is_none());
        let at = self.all_destinations.iter().position(|d| d.platform != Platform::MacOS).unwrap_or(self.all_destinations.len());
        self.all_destinations.splice(at..at, devices);
        self.filter_destinations();
        cx.emit(BuildEvent::ConfigurationChanged);
        cx.notify();
    }

    pub fn set_scheme(&mut self, scheme: String, cx: &mut Context<Self>) {
        self.scheme = Some(scheme);
        cx.emit(BuildEvent::ConfigurationChanged);
        cx.notify();
        self.refresh_supported_destinations(cx);
        self.load_scheme(cx);
    }

    /// Product ▸ Scheme ▸ Next / Previous Scheme.
    pub fn step_scheme(&mut self, delta: isize, cx: &mut Context<Self>) {
        let schemes = &self.schemes.schemes;
        if schemes.is_empty() {
            return;
        }
        let len = schemes.len() as isize;
        let current = self.scheme.as_ref().and_then(|s| schemes.iter().position(|n| n == s));
        let next = match current {
            Some(ix) => (ix as isize + delta).rem_euclid(len) as usize,
            None => 0,
        };
        let scheme = schemes[next].clone();
        self.set_scheme(scheme, cx);
    }

    /// Reads the scheme's Edit Scheme options and test plans (from its
    /// `.xcscheme`) in the background.
    pub fn load_scheme(&mut self, cx: &mut Context<Self>) {
        let Some(scheme) = self.scheme.clone() else {
            self.test_plans.clear();
            self.scheme_options = None;
            return;
        };
        let kind = self.project.kind.clone();
        let stored = self.stored_options.get(&scheme).cloned();
        self._plans_task = Some(cx.spawn(async move |this, cx| {
            let (loaded, found) = cx
                .background_spawn(async move {
                    let loaded = scheme::load(&kind, &scheme, stored.as_ref());
                    let plans = if kind.is_xcode() { plans::scheme_test_plans(&kind, &scheme) } else { vec![] };
                    (loaded, plans)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.test_plan.as_ref().is_some_and(|p| !found.iter().any(|f| f.name == *p)) {
                    this.test_plan = None;
                }
                this.test_plans = found;
                if this.scheme.as_deref() == Some(loaded.scheme.as_str()) {
                    this.set_loaded_options(loaded);
                }
                cx.emit(BuildEvent::ConfigurationChanged);
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_loaded_options(&mut self, mut loaded: scheme::Loaded) {
        if loaded.source == scheme::Source::Session && !self.stored_options.contains_key(&loaded.scheme) {
            // The configuration chosen before the scheme had stored options.
            loaded.options.run.configuration = self.configuration.clone();
        }
        if let Some(error) = &loaded.error {
            ::log::warn!("scheme {}: {error}", loaded.scheme);
        }
        if self.configuration != loaded.options.run.configuration {
            self.configuration = loaded.options.run.configuration.clone();
            self.settings.clear();
        }
        self.scheme_options = Some(loaded);
    }

    /// The current scheme's options, re-read first when its file changed on
    /// disk (Xcode or a checkout edited it).
    pub fn fresh_scheme_options(&mut self, scheme: &str) -> scheme::Loaded {
        let current = self.scheme_options.as_ref().filter(|l| l.scheme == scheme && !l.is_stale());
        if let Some(loaded) = current {
            return loaded.clone();
        }
        let loaded = scheme::load(&self.project.kind, scheme, self.stored_options.get(scheme));
        self.set_loaded_options(loaded);
        self.scheme_options.clone().expect("just set")
    }

    /// Product ▸ Scheme ▸ Edit Scheme… applied: writes `options` to the
    /// scheme file, or keeps them for the session.
    pub fn save_scheme_options(&mut self, options: SchemeOptions, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let Some(scheme) = self.scheme.clone() else { anyhow::bail!("No scheme selected") };
        let mut loaded = self.fresh_scheme_options(&scheme);
        match &loaded.source {
            scheme::Source::File(_) => loaded.mtime = scheme::save(&loaded, &options)?,
            scheme::Source::Session => {
                self.stored_options.insert(scheme.clone(), options.clone());
                cx.emit(BuildEvent::StoredSchemeOptionsChanged);
            }
        }
        loaded.options = options;
        self.set_loaded_options(loaded);
        cx.emit(BuildEvent::ConfigurationChanged);
        cx.notify();
        Ok(())
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

    /// The toolbar's Build Configuration: the scheme's Run action's.
    pub fn set_configuration(&mut self, configuration: String, cx: &mut Context<Self>) {
        let Some(scheme) = self.scheme.clone() else {
            self.configuration = configuration;
            self.settings.clear();
            cx.emit(BuildEvent::ConfigurationChanged);
            cx.notify();
            return;
        };
        let mut options = self.fresh_scheme_options(&scheme).options;
        options.run.configuration = configuration;
        if let Err(err) = self.save_scheme_options(options, cx) {
            self.push_log_line(format!("error: Could not save the scheme: {err:#}"));
            cx.emit(BuildEvent::LogUpdated);
        }
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
        if self.stop_recording(cx) {
            return;
        }
        // devicectl passes the SIGTERM on to the app it runs.
        self.process.terminate();
        self._attach_task = None;
        match self.run_target.take() {
            Some(RunTarget::Simulator { udid, bundle_id }) => {
                let mut cmd = Command::new("/usr/bin/xcrun");
                cmd.args(["simctl", "terminate", &udid, &bundle_id]);
                cx.background_spawn(async move {
                    process::run_quiet(cmd).await.ok();
                })
                .detach();
            }
            // In case the console's devicectl couldn't (it may be gone).
            Some(RunTarget::Device { udid, installed, result, .. }) => {
                cx.background_spawn(async move {
                    if let Ok(Some(pid)) = devices::find_process(&udid, &installed) {
                        devices::terminate(&udid, pid).ok();
                    }
                    // The run no longer reads devicectl's result once it exits.
                    smol::Timer::after(Duration::from_secs(5)).await;
                    std::fs::remove_file(result).ok();
                })
                .detach();
            }
            None => {}
        }
        if self.phase.is_busy() {
            self.task = None;
            self.end_recording();
            self.phase = Phase::Stopped;
            cx.emit(BuildEvent::PhaseChanged);
            cx.notify();
        }
    }

    /// Quitting or closing the window: everything this window started ends
    /// without waiting for the main loop (`kill` follows once quitting has
    /// waited a moment).
    pub fn end_processes(&mut self) {
        self.process.terminate();
        self.end_recording();
        self.index_process.terminate();
        if let Some(RunTarget::Simulator { udid, bundle_id }) = self.run_target.take() {
            // simctl keeps going after zcode has quit.
            Command::new("/usr/bin/xcrun")
                .args(["simctl", "terminate", &udid, &bundle_id])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map(|mut simctl| std::thread::spawn(move || simctl.wait()))
                .ok();
        }
    }

    /// SIGKILL what `end_processes` asked to end and is still there.
    pub fn kill_processes(&self) {
        self.process.kill();
        self.index_process.kill();
    }

    /// The pid of the run's program (a Mac app or package product,
    /// `simctl` or `devicectl`), for the control channel.
    pub fn run_pid(&self) -> Option<i32> {
        matches!(self.phase, Phase::Running { .. }).then(|| self.process.pid()).filter(|pid| *pid > 0)
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
        if destination.platform.is_device() {
            // Compiling is all indexing needs; don't sign or provision.
            niced.arg("CODE_SIGNING_ALLOWED=NO");
        }
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
        self.generation += 1;
        self.begin(action, tests, cx);
    }

    /// `start_with` without counting a new one: also what an action queued
    /// until the schemes were known runs as.
    fn begin(&mut self, action: BuildAction, tests: Option<TestRequest>, cx: &mut Context<Self>) {
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
            // A recording ends without saving when something replaces it.
            self.end_recording();
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
        let profile = (action == BuildAction::Profile).then(|| self.profile_request.take().unwrap_or(profile::ProfileRequest {
            template: None,
            build: true,
            time_limit: None,
        }));
        if matches!(action, BuildAction::Run | BuildAction::Profile) {
            self.console.clear();
            cx.emit(BuildEvent::ConsoleUpdated);
        }
        self.phase = Phase::Preparing(action);
        cx.emit(BuildEvent::IssuesChanged);
        cx.emit(BuildEvent::LogUpdated);
        cx.emit(BuildEvent::PhaseChanged);
        cx.notify();

        let project = self.project.clone();
        let loaded = self.fresh_scheme_options(&scheme);
        let options = loaded.options.clone();
        // Tests build with the Test action's configuration, the rest with
        // the Run action's.
        let configuration = match action {
            BuildAction::Test => options.test.configuration.clone(),
            BuildAction::Profile => options.profile.configuration.clone(),
            _ => self.configuration.clone(),
        };
        // Instruments takes no sanitizers.
        let diagnostics = match action {
            BuildAction::Test => options.test.diagnostics.clone(),
            BuildAction::Clean | BuildAction::Profile => scheme::Diagnostics { main_thread_checker: false, ..Default::default() },
            _ => options.run.diagnostics.clone(),
        };
        // xcodebuild reads a scheme file's Test action itself; other runners
        // get the test arguments and environment from here.
        let test_launch = (tests.is_some() && !matches!(loaded.source, scheme::Source::File(_))).then(|| options.test_launch());
        let handle = ProcessHandle::default();
        self.process = handle.clone();
        let cache_dir = self.cache_dir.clone();
        // Edit Scheme ▸ Run ▸ Debug executable off: never under the debugger.
        let debug = action == BuildAction::Run && std::mem::take(&mut self.debug_run) && options.debug_executable;
        // Settings depend on the configuration; Run's are cached.
        let cached_settings = self
            .settings
            .get(&(scheme.clone(), destination.id.clone()))
            .cloned()
            .filter(|_| action == BuildAction::Run);

        self.task = Some(cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let tool_action = match action {
                BuildAction::Run | BuildAction::Profile => BuildAction::Build,
                action => action,
            };
            let records = profile.as_ref().is_some_and(|p| p.template.is_some());
            let skip_build = profile.as_ref().is_some_and(|p| !p.build);
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
            cmd.args(match project.kind {
                ProjectKind::SwiftPackage => diagnostics.swiftpm_args(),
                _ => diagnostics.xcodebuild_args(),
            });
            if let Some(plan) = &test_plan {
                cmd.args(["-testPlan", plan]);
            }
            if let (Some(launch), false) = (&test_launch, direct_tests) {
                // `swift test` passes its environment on; `xcodebuild test`
                // passes on what is prefixed with TEST_RUNNER_. Neither
                // takes launch arguments.
                match project.kind {
                    ProjectKind::SwiftPackage => cmd.envs(launch.environment.iter().cloned()),
                    _ => cmd.envs(launch.environment.iter().map(|(k, v)| (format!("TEST_RUNNER_{k}"), v.clone()))),
                };
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
            let settings_task = ((action == BuildAction::Run || records) && project.kind.is_xcode()).then(|| {
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

            // A device has to be there to build for it (and to run). Its
            // state may have changed since the last poll.
            if destination.device.is_some() && tool_action != BuildAction::Clean {
                let fresh = cx.background_spawn(async move { devices::list() }).await;
                let problem = match &fresh {
                    Ok(all) => match all.iter().find(|d| d.id == destination.id) {
                        Some(device) => devices::unavailable_message(device),
                        None => Some(format!("{} is not connected.", destination.name)),
                    },
                    Err(_) => None,
                };
                if let Some(problem) = problem {
                    this.update(cx, |this, cx| {
                        if let Ok(all) = fresh {
                            this.set_devices(all, cx);
                        }
                        this.fail(action, problem, started, cx);
                    })
                    .ok();
                    return;
                }
            }

            // Profile Without Building records what was built last.
            let (code, mut raw_log) = if skip_build {
                (Some(0), String::new())
            } else {
                this.update(cx, |this, cx| {
                    this.phase = Phase::Building { action, started };
                    this.track_suites = track_suites;
                    this.push_log_line(format!("$ {}", display_command(&cmd)));
                    cx.emit(BuildEvent::PhaseChanged);
                    cx.notify();
                })
                .ok();
                stream_into(&this, cx, cmd, handle.clone(), Sink::Log).await
            };
            let mut success = code == Some(0);
            if direct_tests && success && this.read_with(cx, |this, _| this.phase != Phase::Stopped).unwrap_or(false) {
                let request = tests.clone().unwrap_or_default();
                let (ok, output) = run_simulator_tests(
                    &this, cx, &project, &scheme, &configuration, &destination, &request, test_launch.as_ref(), &cache_dir, &handle,
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
            let db_changed = if skip_build {
                false
            } else {
                cx
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
                    .await
            };

            let continue_to_run = this
                .update(cx, |this, cx| {
                    if db_changed {
                        cx.emit(BuildEvent::CompileDatabaseUpdated(db_path.clone()));
                    }
                    let stopped = this.phase == Phase::Stopped;
                    if !skip_build {
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
                    }
                    if stopped {
                        return false;
                    }
                    if !(action == BuildAction::Run || records) || !success {
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
            if let Some(request) = profile {
                let template = request.template.unwrap_or_default();
                let job = profile::Job {
                    project, scheme, configuration, destination, options, settings: settings_task, template,
                    time_limit: request.time_limit, handle, started,
                };
                profile::record(&this, cx, job).await;
                return;
            }

            // Launch, with the Run action's arguments, environment and
            // Main Thread Checker (under the debugger, as in Xcode).
            let main_thread_checker = debug && options.run.diagnostics.main_thread_checker;
            let launch = match &project.kind {
                ProjectKind::SwiftPackage => {
                    let root = project.root.clone();
                    let scheme = scheme.clone();
                    let configuration = configuration.to_lowercase();
                    let sanitizers = diagnostics.swiftpm_args();
                    let mut launch = options.run_launch(None);
                    cx.background_spawn(async move {
                        let mut cmd = Command::new("/usr/bin/swift");
                        cmd.args(["build", "-c", &configuration, "--show-bin-path"])
                            .args(&sanitizers)
                            .current_dir(&root);
                        let (_, out) = process::run_quiet(cmd).await?;
                        let bin = PathBuf::from(out.lines().last().unwrap_or("").trim()).join(&scheme);
                        if main_thread_checker && let Some(library) = xcode::main_thread_checker(&Destination::my_mac()) {
                            launch.insert_library(&library);
                        }
                        anyhow::Ok((launch_command(bin, &launch, &root, Some(&root)), None))
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
                            let (lines_tx, lines) = channel::unbounded::<String>();
                            let launch = options.run_launch(Some(&settings.values));
                            let root = project.root.clone();
                            let prepared = cx.background_spawn(async move {
                                let options = LaunchOptions { launch, debug, main_thread_checker, root };
                                prepare_launch(&destination, &settings, options, lines_tx).await
                            });
                            // Preparation messages reach the console as they
                            // come ("Unlock it to continue…" while waiting).
                            while let Ok(line) = lines.recv().await {
                                this.update(cx, |this, cx| {
                                    this.push_console_line(line);
                                    cx.emit(BuildEvent::ConsoleUpdated);
                                    cx.notify();
                                })
                                .ok();
                            }
                            prepared.await
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
                        this.fail(BuildAction::Run, format!("Launch failed: {err:#}"), started, cx);
                    })
                    .ok();
                    return;
                }
            };

            let run_started = Instant::now();
            let launch_under_debugger = debug && target.is_none();
            this.update(cx, |this, cx| {
                this.awaiting_attach = match &target {
                    Some(RunTarget::Simulator { bundle_id, .. }) if debug => Some(bundle_id.clone()),
                    _ => None,
                };
                if let (Some(RunTarget::Device { udid, installed, stale_pid, attach, .. }), true) = (&target, debug) {
                    let watch = devices::ProcessWatch { udid: udid.clone(), installed: installed.clone(), stale_pid: *stale_pid };
                    this.attach_on_device(watch, attach.clone(), cx);
                }
                this.run_target = target.clone();
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
                    let args = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
                    let env = cmd
                        .get_envs()
                        .filter_map(|(k, v)| Some((k.to_string_lossy().to_string(), v?.to_string_lossy().to_string())))
                        .collect();
                    this.push_console_line(format!("Debugging {}", program.display()));
                    cx.emit(BuildEvent::LaunchDebugger { program, cwd, args, env });
                }
                cx.emit(BuildEvent::PhaseChanged);
                cx.notify();
            })
            .ok();
            if launch_under_debugger {
                // lldb-dap owns the process; the workspace ends the run.
                return;
            }

            // simctl gives the app its own pseudo-terminal (`--console-pty`);
            // devicectl bridges the app's output.
            let sink = match &target {
                Some(RunTarget::Simulator { .. }) => Sink::Console,
                Some(RunTarget::Device { .. }) => Sink::DeviceConsole,
                None => Sink::Terminal,
            };
            let (code, _) = stream_into(&this, cx, cmd, handle, sink).await;
            // devicectl's exit status isn't the app's; its JSON result is.
            let ended = match &target {
                Some(RunTarget::Device { result, .. }) => {
                    let result = result.clone();
                    cx.background_spawn(async move { devices::read_launch_result(&result) }).await
                }
                _ => match code {
                    Some(code) => devices::Ended::Exited(code),
                    None => devices::Ended::Terminated,
                },
            };
            this.update(cx, |this, cx| {
                if matches!(this.phase, Phase::Running { .. }) {
                    if let devices::Ended::Failed(message) = &ended {
                        this.push_console_line(format!("Launch failed: {message}"));
                        this.fail(BuildAction::Run, format!("Launch failed: {message}"), run_started, cx);
                        this.run_target = None;
                        return;
                    }
                    this.push_console_line(match ended {
                        devices::Ended::Exited(code) => format!("Program ended with exit code: {code}"),
                        devices::Ended::Signaled(signal) => format!("Program terminated by signal {signal}"),
                        _ => "Program terminated".to_string(),
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

    /// Ends `action` unsuccessfully with an error in the Issue navigator.
    fn fail(&mut self, action: BuildAction, message: String, started: Instant, cx: &mut Context<Self>) {
        self.push_log_line(format!("error: {message}"));
        let issue = Issue {
            severity: Severity::Error,
            message,
            path: None,
            line: None,
            column: None,
            source: log::IssueSource::Build,
        };
        if !self.issues.contains(&issue) {
            self.issues.push(issue);
        }
        self.phase = Phase::Finished {
            action,
            success: false,
            duration: started.elapsed(),
        };
        cx.emit(BuildEvent::IssuesChanged);
        cx.emit(BuildEvent::LogUpdated);
        cx.emit(BuildEvent::ConsoleUpdated);
        cx.emit(BuildEvent::PhaseChanged);
        cx.notify();
    }

    /// Waits for the app launched suspended on a device to show up among
    /// its processes, then has the debugger attach.
    fn attach_on_device(&mut self, watch: devices::ProcessWatch, mut attach: devices::Attach, cx: &mut Context<Self>) {
        self._attach_task = Some(cx.spawn(async move |this, cx| {
            let deadline = Instant::now() + Duration::from_secs(60);
            let pid = loop {
                let found = cx.background_spawn({
                    let watch = watch.clone();
                    async move { watch.find() }
                });
                match found.await {
                    Ok(Some(pid)) => break Some(pid),
                    _ if Instant::now() > deadline => break None,
                    _ => cx.background_executor().timer(Duration::from_millis(500)).await,
                }
            };
            this.update(cx, |this, cx| {
                this._attach_task = None;
                match pid {
                    Some(pid) => {
                        this.push_console_line(format!("Attaching to process {pid}…"));
                        attach.pid = pid;
                        cx.emit(BuildEvent::AttachDeviceDebugger(attach));
                    }
                    None => this.push_console_line("warning: The app's process didn't appear on the device; not debugging".into()),
                }
                cx.emit(BuildEvent::ConsoleUpdated);
                cx.notify();
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

/// What a run launched, for Stop and the debugger.
#[derive(Clone, Debug)]
enum RunTarget {
    Simulator { udid: String, bundle_id: String },
    Device {
        udid: String,
        /// For the debugger (its pid is found once launched).
        attach: devices::Attach,
        /// The bundle's path on the device.
        installed: String,
        /// An earlier instance's process, which `--terminate-existing` ends.
        stale_pid: Option<i32>,
        /// devicectl's `--json-output` for the launch.
        result: PathBuf,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum Sink {
    Log,
    Console,
    /// The console, from `devicectl --console` (its own lines dropped).
    DeviceConsole,
    /// The console, from a program on a pseudo-terminal.
    Terminal,
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
    let process = match sink {
        Sink::Terminal => cx.background_spawn(process::run_streaming_tty(cmd, tx, handle)),
        _ => cx.background_spawn(process::run_streaming(cmd, tx, handle)),
    };
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
            for line in &mut batch {
                if let std::borrow::Cow::Owned(plain) = log::strip_ansi(line) {
                    *line = plain;
                }
            }
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
                        Sink::DeviceConsole if devices::is_console_chatter(&line) => {}
                        Sink::Console | Sink::DeviceConsole | Sink::Terminal => this.push_console_line(line),
                    }
                }
                match sink {
                    Sink::Log => cx.emit(BuildEvent::LogUpdated),
                    Sink::Console | Sink::DeviceConsole | Sink::Terminal => cx.emit(BuildEvent::ConsoleUpdated),
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

pub fn display_command(cmd: &Command) -> String {
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
    extra: Option<&Launch>,
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
    for mut target in targets {
        // A scheme without a file has no test options in the .xctestrun.
        if let Some(extra) = extra {
            target.arguments.extend(extra.arguments.iter().cloned());
            for (key, value) in &extra.environment {
                if !target.environment.iter().any(|(k, _)| k == key) {
                    target.environment.push((key.clone(), value.clone()));
                }
            }
        }
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
        .args(["-skipMacroValidation", "-skipPackagePluginValidation"]);
    if destination.platform.is_device() {
        // Automatic signing, as Xcode does it: create or update profiles,
        // and register a connected device with the team.
        cmd.arg("-allowProvisioningUpdates");
        if destination.device.is_some() {
            cmd.arg("-allowProvisioningDeviceRegistration");
        }
    }
    cmd.arg(verb);
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
                BuildAction::Build | BuildAction::Run | BuildAction::Profile => {
                    cmd.args(["build", "-c", &configuration.to_lowercase()]);
                    if !scheme.is_empty() {
                        cmd.args(["--product", scheme]);
                    }
                }
                BuildAction::Test => {
                    cmd.arg("test");
                    if !configuration.eq_ignore_ascii_case("debug") {
                        cmd.args(["-c", &configuration.to_lowercase()]);
                    }
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
                BuildAction::Build | BuildAction::Run | BuildAction::Profile => "build",
                BuildAction::Test => "test",
                BuildAction::Clean => "clean",
            },
        ),
    }
}

/// `program` with the scheme's arguments, environment and working
/// directory (relative ones are in `root`), else `default_cwd`.
fn launch_command(program: PathBuf, launch: &Launch, root: &Path, default_cwd: Option<&Path>) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(&launch.arguments).envs(launch.environment.iter().cloned());
    match (&launch.working_directory, default_cwd) {
        (Some(dir), _) => {
            cmd.current_dir(root.join(dir));
        }
        (None, Some(dir)) => {
            cmd.current_dir(dir);
        }
        (None, None) => {}
    }
    cmd
}

/// How the Run action launches its product.
struct LaunchOptions {
    launch: Launch,
    debug: bool,
    main_thread_checker: bool,
    /// The project folder, for relative working directories.
    root: PathBuf,
}

/// Boots/installs as needed and returns the command whose output is the
/// app's console, plus what it runs in a simulator or on a device.
async fn prepare_launch(
    destination: &Destination,
    settings: &BuildSettings,
    options: LaunchOptions,
    messages: channel::Sender<String>,
) -> anyhow::Result<(Command, Option<RunTarget>)> {
    let say = |m: String| {
        messages.try_send(m).ok();
    };
    let LaunchOptions { mut launch, debug, main_thread_checker, root } = options;
    if main_thread_checker {
        match xcode::main_thread_checker(destination) {
            Some(library) => launch.insert_library(&library),
            None if destination.platform.is_device() => say("note: Main Thread Checker isn't inserted on devices".to_string()),
            None => say("warning: Main Thread Checker not found".to_string()),
        }
    }
    match destination.platform {
        _ if destination.is_any_device() => {
            anyhow::bail!("{} only builds. Choose a device or simulator to run on.", destination.name)
        }
        platform if platform.is_device() => {
            let app = settings
                .product_path()
                .ok_or_else(|| anyhow::anyhow!("missing TARGET_BUILD_DIR"))?;
            let Some(device) = destination.device.clone() else {
                anyhow::bail!("{} is not a device", destination.name)
            };
            let udid = destination.id.clone();
            let installed = install_on_device(destination, &app, &messages).await?;
            let stale_pid = smol::unblock({
                let (udid, path) = (udid.clone(), installed.path.clone());
                move || devices::find_process(&udid, &path)
            })
            .await
            .ok()
            .flatten();

            let cached_symbols = devices::has_cached_symbols(destination);
            if debug && !cached_symbols {
                say(format!(
                    "note: {}'s system symbols for {} ({}) aren't on this Mac, so lldb reads them from the device: attaching is slow and system frames have no names. Xcode copies them the first time it prepares the device for development.",
                    destination.name, destination.runtime, device.os_build
                ));
            }
            say(format!("Launching {} on {}", installed.bundle_id, destination.name));
            let result = devices::json_output_path();
            let command = devices::launch_command(&udid, &installed.bundle_id, &launch, debug, &result);
            let target = RunTarget::Device {
                udid,
                attach: devices::Attach { identifier: device.identifier, pid: 0, app, cached_symbols },
                installed: installed.path,
                stale_pid,
                result,
            };
            Ok((command, Some(target)))
        }
        Platform::MacOS => {
            let exe = settings
                .executable_path()
                .ok_or_else(|| anyhow::anyhow!("missing EXECUTABLE_PATH"))?;
            say(format!("Launching {}", exe.display()));
            // Xcode's default working directory: the built products, not
            // the bundle (an instrumented app writes default.profraw there).
            let products = settings.get("TARGET_BUILD_DIR").map(PathBuf::from);
            Ok((launch_command(exe, &launch, &root, products.as_deref()), None))
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
            install_on_simulator(destination, &app, &messages).await?;

            say(format!("Launching {bundle_id}"));
            let mut command = Command::new("/usr/bin/xcrun");
            command.args(["simctl", "launch", "--console-pty", "--terminate-running-process"]);
            if debug {
                command.arg("--wait-for-debugger");
            }
            command.args([&udid, &bundle_id]).args(&launch.arguments);
            // simctl passes SIMCTL_CHILD_-prefixed variables to the app.
            command.envs(launch.environment.iter().map(|(k, v)| (format!("SIMCTL_CHILD_{k}"), v.clone())));
            Ok((command, Some(RunTarget::Simulator { udid, bundle_id })))
        }
    }
}

/// Boots the simulator, opens Simulator.app on it and installs `app`.
async fn install_on_simulator(destination: &Destination, app: &Path, messages: &channel::Sender<String>) -> anyhow::Result<()> {
    let say = |m: String| {
        messages.try_send(m).ok();
    };
    let udid = destination.id.as_str();
    say(format!("Booting {} ({})…", destination.name, destination.runtime));
    let mut boot = Command::new("/usr/bin/xcrun");
    boot.args(["simctl", "boot", udid]);
    let (ok, out) = process::run_quiet(boot).await?;
    if !ok && !out.contains("current state: Booted") {
        anyhow::bail!("simctl boot failed: {}", out.trim());
    }

    let simulator_app = crate::toolchain::simulator_app();
    match &simulator_app {
        Some(app) => {
            let mut open = Command::new("/usr/bin/open");
            open.arg("-g").arg("-a").arg(app).args(["--args", "-CurrentDeviceUDID", udid]);
            if let Ok((false, out)) = process::run_quiet(open).await {
                say(format!("warning: Could not open {}: {}", app.display(), out.trim()));
            }
        }
        None => say("warning: No Simulator app found in the active Xcode".to_string()),
    }

    say(format!("Installing {}…", app.file_name().unwrap_or_default().to_string_lossy()));
    let mut install = Command::new("/usr/bin/xcrun");
    install.args(["simctl", "install", udid]).arg(app);
    let (ok, out) = process::run_quiet(install).await?;
    if !ok {
        anyhow::bail!("simctl install failed: {}", out.trim());
    }

    if let Some(app) = &simulator_app {
        let mut bring_front = Command::new("/usr/bin/open");
        bring_front.arg("-a").arg(app);
        process::run_quiet(bring_front).await.ok();
    }
    Ok(())
}

/// Installs `app` on the device, then waits while it's locked: apps can't
/// launch on a locked device (Xcode waits too).
async fn install_on_device(
    destination: &Destination,
    app: &Path,
    messages: &channel::Sender<String>,
) -> anyhow::Result<devices::Installed> {
    let say = |m: String| {
        messages.try_send(m).ok();
    };
    let udid = destination.id.clone();
    say(format!(
        "Installing {} on {}…",
        app.file_name().unwrap_or_default().to_string_lossy(),
        destination.name
    ));
    let installed = smol::unblock({
        let (udid, app) = (udid.clone(), app.to_path_buf());
        move || devices::install(&udid, &app)
    })
    .await
    .map_err(|err| anyhow::anyhow!("Could not install on {}: {err:#}", destination.name))?;

    let mut said = false;
    while smol::unblock({
        let udid = udid.clone();
        move || devices::is_locked(&udid)
    })
    .await
    .unwrap_or(false)
    {
        if !said {
            say(format!("{} is locked. Unlock it to continue…", destination.name));
            said = true;
        }
        smol::Timer::after(Duration::from_secs(2)).await;
    }
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcode::{Device, Transport};

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect()
    }

    #[test]
    fn device_builds_sign_automatically() {
        let project = ProjectInfo {
            root: "/p".into(),
            name: "App".into(),
            kind: ProjectKind::XcodeProject("/p/App.xcodeproj".into()),
        };
        let device = Destination {
            id: "00008150-0000000000000001".into(),
            name: "iPhone".into(),
            platform: Platform::IOS,
            runtime: "iOS 27.2".into(),
            booted: true,
            device: Some(Device {
                identifier: "CORE".into(),
                model: "iPhone 17 Pro".into(),
                product_type: "iPhone18,1".into(),
                os_build: "24B5084k".into(),
                transport: Some(Transport::Network),
                connected: true,
                paired: true,
                developer_mode: Some(true),
                booted: true,
            }),
        };
        let test = args(&build_command(&project, "App", "Debug", &device, BuildAction::Test));
        assert!(test.windows(2).any(|w| w == ["-destination", "id=00008150-0000000000000001"]), "{test:?}");
        assert!(test.ends_with(&["-allowProvisioningUpdates", "-allowProvisioningDeviceRegistration", "test"].map(String::from)), "{test:?}");
        // Any iOS Device has no device to register.
        let generic = args(&build_command(&project, "App", "Debug", &Destination::any_device(Platform::IOS), BuildAction::Build));
        assert!(generic.ends_with(&["generic/platform=iOS", "-skipMacroValidation", "-skipPackagePluginValidation", "-allowProvisioningUpdates", "build"].map(String::from)), "{generic:?}");
        // Simulators and the Mac don't provision.
        let mac = args(&build_command(&project, "App", "Debug", &Destination::my_mac(), BuildAction::Build));
        assert!(!mac.iter().any(|a| a.starts_with("-allowProvisioning")), "{mac:?}");
    }
}

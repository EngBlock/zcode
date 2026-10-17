//! Product ▸ Profile (⌘I): the scheme's product built with its Profile
//! action's configuration (Release unless the scheme says otherwise) and
//! recorded under Instruments with `xctrace record --launch`, on this Mac,
//! a simulator or a device, until Stop or a time limit. The trace then
//! opens in Instruments, and the console lists a Time Profiler recording's
//! heaviest functions. Xcode hands its Profile action to Instruments the
//! same way, asking for a template unless the scheme names one.

pub mod summary;
pub mod xctrace;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, AsyncApp, Context, Task, WeakEntity};
use smol::channel;

use super::process::{self, AppProcess, ProcessHandle};
use super::xcode::{BuildSettings, Destination, Platform};
use super::{BuildAction, BuildController, BuildEvent, Phase, Sink, install_on_device, install_on_simulator, stream_into};
use crate::project::{ProjectInfo, ProjectKind};
use crate::scheme::{Launch, SchemeOptions};
pub use summary::Summary;
pub use xctrace::Template;

/// What Product ▸ Profile was asked to do.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileRequest {
    /// The template to record with; `None` only builds (Build For ▸
    /// Profiling).
    pub template: Option<String>,
    /// Profile Without Building (⌃⌘I) doesn't.
    pub build: bool,
    /// Seconds (`profiling.time_limit`); `None` records until Stop.
    pub time_limit: Option<u32>,
}

/// The recording in progress.
#[derive(Clone, Debug)]
pub struct Recording {
    pub template: String,
    pub trace: PathBuf,
    /// Stop asked xctrace to end the recording, or it ended: the trace is
    /// being saved, opened and summarized.
    pub stopping: bool,
    /// The Mac executable recorded, and its processes from before.
    app: Option<(PathBuf, Vec<AppProcess>)>,
}

impl Recording {
    /// Ends what the recording launched on the Mac (quitting, or after
    /// xctrace has exited).
    pub(super) fn end_app(&self) {
        let Some((executable, before)) = self.app.clone() else { return };
        std::thread::spawn(move || {
            for app in process::processes_running(&executable).into_iter().filter(|p| !before.contains(p)) {
                app.terminate(Duration::from_secs(2));
            }
        });
    }
}

/// How the last recording ended.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub template: String,
    pub trace: Option<PathBuf>,
    /// Opened in Instruments.
    pub opened: bool,
    pub summary: Option<Summary>,
    pub error: Option<String>,
}

/// Instruments' templates, listed once per zcode run when first needed.
#[derive(Clone, Debug, Default)]
pub enum Templates {
    #[default]
    Unknown,
    Loading,
    Loaded(Vec<Template>),
    Failed(String),
}

impl BuildController {
    /// Product ▸ Profile and its variants.
    pub fn profile(&mut self, request: ProfileRequest, cx: &mut Context<Self>) {
        if let Some(template) = &request.template {
            self.last_template = Some(template.clone());
        }
        self.profile_request = Some(request);
        self.start(BuildAction::Profile, cx);
    }

    /// The current scheme's Profile action's instrument, when it names one.
    pub fn scheme_template(&mut self) -> Option<String> {
        let scheme = self.scheme.clone()?;
        xctrace::scheme_template(&self.fresh_scheme_options(&scheme).options.profile.template)
    }

    /// Lists Instruments' templates (`BuildEvent::TemplatesLoaded` once
    /// they're known).
    pub fn load_templates(&mut self, cx: &mut Context<Self>) {
        if matches!(self.templates, Templates::Loading | Templates::Loaded(_)) {
            return;
        }
        self.templates = Templates::Loading;
        self._templates_task = Some(cx.spawn(async move |this, cx| {
            let listed = cx.background_spawn(async { xctrace::list_templates() }).await;
            this.update(cx, |this, cx| {
                this.templates = match listed {
                    Ok(templates) => Templates::Loaded(templates),
                    Err(err) => Templates::Failed(format!("{err:#}")),
                };
                cx.emit(BuildEvent::TemplatesLoaded);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Profiling couldn't start (no templates, no xctrace).
    pub fn profile_failed(&mut self, message: String, cx: &mut Context<Self>) {
        self.last_profile = Some(Outcome { error: Some(message.clone()), ..Outcome::default() });
        self.fail(BuildAction::Profile, message, Instant::now(), cx);
    }

    /// Stop while recording: xctrace ends the recording and saves the
    /// trace, as Instruments' Stop does (a second Stop ends it outright).
    /// Whether that's what Stop did.
    pub(super) fn stop_recording(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(recording) = self.recording.as_mut() else { return false };
        if recording.stopping || !self.process.is_running() {
            self.recording = None;
            return false;
        }
        recording.stopping = true;
        self.process.interrupt();
        self.push_console_line("Stopping the recording…".into());
        cx.emit(BuildEvent::ConsoleUpdated);
        cx.emit(BuildEvent::PhaseChanged);
        cx.notify();
        true
    }
}

/// A built product to record.
pub(super) struct Job {
    pub project: ProjectInfo,
    pub scheme: String,
    pub configuration: String,
    pub destination: Destination,
    pub options: SchemeOptions,
    /// The runnable target's build settings (Xcode projects).
    pub settings: Option<Task<anyhow::Result<BuildSettings>>>,
    pub template: String,
    pub time_limit: Option<u32>,
    pub handle: ProcessHandle,
    pub started: Instant,
}

/// What `xctrace` launches, and where.
struct Target {
    /// The app bundle or executable; a device's app by bundle identifier.
    program: OsString,
    device: Option<String>,
    launch: Launch,
    /// A Mac executable, whose processes the recording started must end
    /// with it.
    executable: Option<PathBuf>,
    /// The process the summary is about.
    process: Option<String>,
}

/// Finds (and installs) the product, records it until Stop, the time limit
/// or its exit, then opens and summarizes the trace.
pub(super) async fn record(this: &WeakEntity<BuildController>, cx: &mut AsyncApp, job: Job) {
    let Job { project, scheme, configuration, destination, options, settings, template, time_limit, handle, started } = job;
    let settings = match settings {
        Some(task) => match task.await {
            Ok(settings) => {
                this.update(cx, |this, _| {
                    this.settings.insert((scheme.clone(), destination.id.clone()), settings.clone());
                })
                .ok();
                Some(settings)
            }
            Err(err) => return fail(this, cx, format!("Profiling failed: {err:#}"), started),
        },
        None => None,
    };
    let (lines_tx, lines) = channel::unbounded::<String>();
    let prepared = cx.background_spawn({
        let destination = destination.clone();
        async move { prepare(&project, &scheme, &configuration, &destination, &options, settings, lines_tx).await }
    });
    // Installing reports to the console as it goes.
    while let Ok(line) = lines.recv().await {
        this.update(cx, |this, cx| {
            this.push_console_line(line);
            cx.emit(BuildEvent::ConsoleUpdated);
            cx.notify();
        })
        .ok();
    }
    let target = match prepared.await {
        Ok(target) => target,
        Err(err) => return fail(this, cx, format!("Profiling failed: {err:#}"), started),
    };

    let dir = crate::paths::traces_dir();
    let trace = xctrace::trace_path(&dir, &scheme_name(this, cx), &template);
    let executable = target.executable.clone();
    let before = cx
        .background_spawn(async move {
            std::fs::create_dir_all(&dir).ok();
            xctrace::prune(&dir);
            executable.map(|e| process::processes_running(&e)).unwrap_or_default()
        })
        .await;
    let cmd = xctrace::record_command(&xctrace::Recording {
        template: &template,
        device: target.device.as_deref(),
        output: &trace,
        time_limit,
        launch: &target.launch,
        target: &target.program,
        forward_output: target.device.is_none(),
    });
    let recording_started = Instant::now();
    this.update(cx, |this, cx| {
        let app = target.executable.clone().map(|executable| (executable, before.clone()));
        this.recording = Some(Recording { template: template.clone(), trace: trace.clone(), stopping: false, app });
        this.phase = Phase::Running { started: recording_started };
        this.push_console_line(format!("$ {}", super::display_command(&cmd)));
        this.push_console_line(match time_limit {
            Some(limit) => format!("Recording {template} for {limit} s on {}; Stop (⌘.) ends it sooner", destination.name),
            None => format!("Recording {template} on {}; Stop (⌘.) ends the recording", destination.name),
        });
        cx.emit(BuildEvent::ConsoleUpdated);
        cx.emit(BuildEvent::PhaseChanged);
        cx.notify();
    })
    .ok();

    // xctrace's own messages and, on the Mac, the app's output.
    let (code, _) = stream_into(this, cx, cmd, handle, Sink::Terminal).await;

    // xctrace ends what it launched; on the Mac, make sure nothing it
    // started is left (an instance that was running before stays).
    this.update(cx, |this, cx| {
        if let Some(recording) = &mut this.recording {
            recording.stopping = true;
            recording.end_app();
        }
        cx.emit(BuildEvent::PhaseChanged);
        cx.notify();
    })
    .ok();

    let saved = trace.exists();
    let (opened, summary) = if saved {
        let process = target.process.clone();
        cx.background_spawn({
            let trace = trace.clone();
            async move { (xctrace::open_trace(&trace), summary::summarize(&trace, process.as_deref())) }
        })
        .await
    } else {
        (Err(anyhow::anyhow!("no trace")), Ok(None))
    };
    this.update(cx, |this, cx| {
        let mut outcome = Outcome { template: template.clone(), ..Outcome::default() };
        if saved {
            this.push_console_line(format!("Saved {}", trace.display()));
            match &opened {
                Ok(()) => this.push_console_line("Opened the trace in Instruments".into()),
                Err(err) => this.push_console_line(format!("warning: Could not open Instruments: {err:#}")),
            }
            match summary {
                Ok(Some(summary)) => {
                    for line in summary.lines(10) {
                        this.push_console_line(line);
                    }
                    outcome.summary = Some(summary);
                }
                Ok(None) => {}
                Err(err) => this.push_console_line(format!("note: No summary of the samples: {err:#}")),
            }
            outcome.trace = Some(trace.clone());
            outcome.opened = opened.is_ok();
        }
        let success = saved && code == Some(0);
        if !saved {
            outcome.error = Some(format!("xctrace saved no trace (exit code {})", code.map_or("none".into(), |c| c.to_string())));
        } else if !success {
            outcome.error = Some("xctrace reported problems with the recording (see the console)".into());
        }
        if let Some(error) = &outcome.error {
            this.push_log_line(format!("error: {error}"));
            cx.emit(BuildEvent::LogUpdated);
            cx.emit(BuildEvent::IssuesChanged);
        }
        this.last_profile = Some(outcome);
        this.recording = None;
        this.phase = Phase::Finished { action: BuildAction::Profile, success, duration: recording_started.elapsed() };
        cx.emit(BuildEvent::ConsoleUpdated);
        cx.emit(BuildEvent::PhaseChanged);
        cx.notify();
    })
    .ok();
}

fn scheme_name(this: &WeakEntity<BuildController>, cx: &mut AsyncApp) -> String {
    this.read_with(cx, |this, _| this.scheme.clone()).ok().flatten().unwrap_or_default()
}

fn fail(this: &WeakEntity<BuildController>, cx: &mut AsyncApp, message: String, started: Instant) {
    this.update(cx, |this, cx| {
        this.push_console_line(message.clone());
        this.recording = None;
        this.last_profile = Some(Outcome { error: Some(message.clone()), ..Outcome::default() });
        this.fail(BuildAction::Profile, message, started, cx);
    })
    .ok();
}

/// The product to launch, installed on a simulator or device first.
async fn prepare(
    project: &ProjectInfo,
    scheme: &str,
    configuration: &str,
    destination: &Destination,
    options: &SchemeOptions,
    settings: Option<BuildSettings>,
    messages: channel::Sender<String>,
) -> anyhow::Result<Target> {
    let root = &project.root;
    let absolute = |mut launch: Launch, default: Option<PathBuf>| {
        launch.working_directory = launch.working_directory.map(|dir| root.join(dir)).or(default);
        launch
    };
    if matches!(project.kind, ProjectKind::SwiftPackage) {
        let mut cmd = Command::new("/usr/bin/swift");
        cmd.args(["build", "-c", &configuration.to_lowercase(), "--show-bin-path"]).current_dir(root);
        let (_, out) = process::run_quiet(cmd).await?;
        let executable = PathBuf::from(out.lines().last().unwrap_or("").trim()).join(scheme);
        return Ok(Target {
            program: executable.clone().into(),
            device: None,
            launch: absolute(options.profile_launch(None), Some(root.clone())),
            executable: Some(executable),
            process: Some(scheme.to_string()),
        });
    }
    let settings = settings.ok_or_else(|| anyhow::anyhow!("no build settings"))?;
    let launch = options.profile_launch(Some(&settings.values));
    let product = settings.product_path().ok_or_else(|| anyhow::anyhow!("missing TARGET_BUILD_DIR"))?;
    let process = settings.get("EXECUTABLE_NAME").map(str::to_string);
    match destination.platform {
        _ if destination.is_any_device() => {
            anyhow::bail!("{} only builds. Choose a device or simulator to profile on.", destination.name)
        }
        Platform::MacOS => {
            let executable = settings.executable_path().ok_or_else(|| anyhow::anyhow!("missing EXECUTABLE_PATH"))?;
            // Instruments launches an app by its bundle, a tool by itself.
            let program = if settings.is_application() { product } else { executable.clone() };
            let products = settings.get("TARGET_BUILD_DIR").map(PathBuf::from);
            Ok(Target { program: program.into(), device: None, launch: absolute(launch, products), executable: Some(executable), process })
        }
        platform if platform.is_device() => {
            let installed = install_on_device(destination, &product, &messages).await?;
            Ok(Target {
                program: installed.bundle_id.into(),
                device: Some(destination.id.clone()),
                launch: absolute(launch, None),
                executable: None,
                process,
            })
        }
        _ => {
            install_on_simulator(destination, &product, &messages).await?;
            Ok(Target {
                program: product.into(),
                device: Some(destination.id.clone()),
                launch: absolute(launch, None),
                executable: None,
                process,
            })
        }
    }
}

//! Running a web app's `package.json` scripts (`crate::build::web`) with
//! the toolbar's Run: each in a terminal of the debug area's Terminal tab,
//! named for it (`web: dev`), with the package manager its lockfile
//! implies, in its folder, with the login shell's `PATH` and its
//! `node_modules/.bin`. A script runs beside the scheme's build and app,
//! so a dev server keeps running while the app runs against it and the
//! other way round; Stop (⌘.) ends the chosen target's run. What a script
//! prints is read as it comes: its dev server's URL (opened once in the
//! browser, shown in the activity view) and its errors and warnings, which
//! join the Issue navigator as build issues until it runs again.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::*;

use super::terminals::PanelTerminal;
use super::{DebugTab, Workspace};
use crate::build::log::{Issue, IssueSource, Severity};
use crate::build::web::{self, OutputParser, ScriptId};
use crate::terminal::{Spawn, TerminalView};

/// Lines of a script's output kept for the MCP tools (the terminal keeps
/// its own scrollback).
const MAX_OUTPUT_LINES: usize = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunState {
    Running,
    /// It ended by itself, with its exit code when it had one.
    Exited(Option<i32>),
    /// Stop ended it.
    Stopped,
}

pub(super) struct WebRun {
    pub id: ScriptId,
    /// `web: dev`.
    pub name: String,
    /// The package manager it runs with (`npm`).
    pub manager: &'static str,
    pub view: Entity<TerminalView>,
    pub started: Instant,
    pub state: RunState,
    /// Its URL and issues.
    pub parser: OutputParser,
    /// Why it failed to run, as an issue (npm missing, a nonzero exit).
    pub failure: Option<Issue>,
    pub output: VecDeque<String>,
    pub total_lines: usize,
    _lines: Task<()>,
}

impl WebRun {
    pub fn is_running(&self) -> bool {
        self.state == RunState::Running
    }

    /// Its issues, the failure first.
    pub fn issues(&self) -> impl Iterator<Item = &Issue> {
        self.failure.iter().chain(&self.parser.issues)
    }
}

#[derive(Default)]
pub(super) struct WebRuns {
    pub runs: Vec<WebRun>,
    /// Every `package.json` the scan found (with scripts or not yet), and
    /// when each was last read.
    manifests: Vec<(PathBuf, Option<SystemTime>)>,
    /// Run was asked for before the scripts were read.
    queued_run: bool,
    /// The URLs opened in the browser: running a dev server again doesn't
    /// open another tab.
    opened: std::collections::HashSet<String>,
    _watch: Option<Task<()>>,
    _discover: Option<Task<()>>,
}

impl Workspace {
    // ---------------------------------------------------------------------
    // The web apps and their scripts

    /// Reads the web apps among the project's files (after a scan) and
    /// follows their `package.json` files.
    pub(super) fn refresh_web_apps(&mut self, cx: &mut Context<Self>) {
        let root = self.project.root.clone();
        let files = self.files.clone();
        self.web._discover = Some(cx.spawn(async move |this, cx| {
            let (apps, manifests) = cx
                .background_spawn(async move {
                    let manifests: Vec<(PathBuf, Option<SystemTime>)> = files
                        .iter()
                        .filter(|f| f.file_name().is_some_and(|n| n == "package.json"))
                        .map(|f| root.join(f))
                        .map(|path| {
                            let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                            (path, mtime)
                        })
                        .collect();
                    (web::discover(&root, &files), manifests)
                })
                .await;
            this.update(cx, |this, cx| {
                this.web.manifests = manifests;
                this.build.update(cx, |build, cx| build.set_web_apps(apps, cx));
                this.follow_web_manifests(cx);
                if std::mem::take(&mut this.web.queued_run) {
                    this.run_chosen_script_later(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Reads the web apps again when one of the `package.json` files
    /// changes (scripts added, renamed or removed).
    fn follow_web_manifests(&mut self, cx: &mut Context<Self>) {
        if self.web._watch.is_some() || self.web.manifests.is_empty() {
            return;
        }
        let changes = crate::fs_watch::Changes::new(self.web.manifests.iter().map(|(path, _)| path.clone()).collect());
        self.web._watch = Some(cx.spawn(async move |this, cx| {
            loop {
                changes.wait(Duration::from_secs(10), cx.background_executor()).await;
                let Ok(manifests) = this.update(cx, |this, _| this.web.manifests.clone()) else { break };
                changes.set_paths(manifests.iter().map(|(path, _)| path.clone()).collect());
                let changed = cx
                    .background_spawn(async move {
                        manifests.iter().any(|(path, mtime)| std::fs::metadata(path).and_then(|m| m.modified()).ok() != *mtime)
                    })
                    .await;
                if changed && this.update(cx, |this, cx| this.refresh_web_apps(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    // ---------------------------------------------------------------------
    // Running and stopping

    /// Run (⌘R) with a script chosen.
    pub(super) fn run_chosen_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let build = self.build.read(cx);
        let Some(id) = build.script.clone() else { return };
        if !build.web_apps_loaded {
            // As soon as the scripts are read.
            self.web.queued_run = true;
            return;
        }
        if let Err(message) = self.run_script(id, window, cx) {
            self.notify_error(message, window, cx);
        }
    }

    fn run_chosen_script_later(&mut self, cx: &mut Context<Self>) {
        let window = self.window;
        let workspace = cx.weak_entity();
        cx.defer(move |cx| {
            window
                .update(cx, |_, window, cx| workspace.update(cx, |this, cx| this.run_chosen_script(window, cx)))
                .ok();
        });
    }

    /// Runs `id` in a terminal of the debug area, shown without taking the
    /// keyboard (as Run shows the console). Its earlier run ends first and
    /// its terminal takes the earlier one's place; other runs, the
    /// scheme's build and app go on.
    pub fn run_script(&mut self, id: ScriptId, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (app, script) = self.build.read(cx).web_script(&id).map(|(a, s)| (a.clone(), s.clone())).ok_or_else(|| {
            format!("There’s no script “{}” in {}’s package.json.", id.script, id.folder.join("package.json").display())
        })?;
        // As Xcode saves before it builds.
        self.save_all(window, cx);
        let name = app.script_name(&script.name);
        let mut spawn = Spawn::script(app.dir.clone(), &app.manager.run_command(&script.name));
        spawn.path = Some(app.bin_dirs(&self.project.root));
        spawn.shell = self.settings.current.terminal_shell.clone();
        let (tap, lines) = smol::channel::bounded(4_096);
        let font_size = self.font_size;
        let view = cx.new(|cx| TerminalView::run(spawn, font_size, name.clone(), tap, cx));
        let subscriptions = self.terminal_subscriptions(&view, window, cx);

        // The earlier run of the script, ended, its terminal replaced.
        let earlier = self.web.runs.iter().position(|r| r.id == id);
        let earlier_view = earlier.map(|ix| self.web.runs[ix].view.clone());
        if let Some(old) = &earlier_view {
            old.update(cx, |view, cx| view.end(cx));
        }
        let slot = earlier_view.and_then(|old| self.terminals.panel.iter().position(|t| t.view == old));
        let terminal = PanelTerminal::new(view.clone(), subscriptions, true);
        let ix = match slot {
            Some(ix) => {
                self.terminals.panel[ix] = terminal;
                ix
            }
            None => {
                self.terminals.panel.push(terminal);
                self.terminals.panel.len() - 1
            }
        };
        self.terminals.active = ix;
        self.show_debug_area = true;
        self.debug_tab = DebugTab::Terminal;

        let view_id = view.entity_id();
        let task = cx.spawn(async move |this, cx| {
            while let Ok(first) = lines.recv().await {
                let mut batch = vec![first];
                batch.extend(std::iter::from_fn(|| lines.try_recv().ok()));
                if this.update(cx, |this, cx| this.web_output(view_id, batch, cx)).is_err() {
                    break;
                }
                // At most ~30 updates a second, however fast it prints.
                cx.background_executor().timer(Duration::from_millis(33)).await;
            }
        });
        let run = WebRun {
            id,
            name,
            manager: app.manager.name(),
            view,
            started: Instant::now(),
            state: RunState::Running,
            parser: OutputParser::new(app.dir.clone()),
            failure: None,
            output: VecDeque::new(),
            total_lines: 0,
            _lines: task,
        };
        let had_issues = earlier.is_some_and(|ix| self.web.runs[ix].issues().next().is_some());
        match earlier {
            Some(ix) => self.web.runs[ix] = run,
            None => self.web.runs.push(run),
        }
        if had_issues {
            self.web_issues_changed(cx);
        }
        self.follow_activity(cx);
        cx.notify();
        Ok(())
    }

    /// What a run printed: kept for the MCP tools, read for its URL and
    /// issues.
    fn web_output(&mut self, view: EntityId, lines: Vec<String>, cx: &mut Context<Self>) {
        let Some(run) = self.web.runs.iter_mut().find(|r| r.view.entity_id() == view) else { return };
        let mut url = None;
        let mut issues = false;
        for line in lines {
            let fed = run.parser.feed(&line);
            if fed.url {
                url = run.parser.url.clone();
            }
            issues |= fed.issues;
            run.total_lines += 1;
            run.output.push_back(line);
            if run.output.len() > MAX_OUTPUT_LINES {
                run.output.pop_front();
            }
        }
        if let Some(url) = url
            && self.settings.current.open_dev_server_url
            && self.web.opened.insert(url.clone())
        {
            self.open_web_url(&url, cx);
        }
        if issues {
            self.web_issues_changed(cx);
        }
        cx.notify();
    }

    /// Opens a dev server's URL in the browser. `ZCODE_OPEN_URL_LOG` (the
    /// e2e checks) appends it to that file instead.
    pub(super) fn open_web_url(&mut self, url: &str, cx: &mut Context<Self>) {
        match std::env::var_os("ZCODE_OPEN_URL_LOG") {
            Some(log) => {
                use std::io::Write as _;
                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
                    writeln!(file, "{url}").ok();
                }
            }
            None => cx.open_url(url),
        }
    }

    /// A run's terminal said its program ended (not through Stop).
    pub(super) fn web_run_exited(&mut self, view: EntityId, cx: &mut Context<Self>) {
        let Some(run) = self.web.runs.iter_mut().find(|r| r.view.entity_id() == view) else { return };
        if run.state != RunState::Running {
            return;
        }
        let code = run.view.read(cx).exit().flatten();
        run.state = RunState::Exited(code);
        let message = match code {
            Some(0) | None => None,
            Some(127) => Some(format!("{}: {} wasn’t found on your PATH (the login shell’s). Install it, or run the script in a terminal.", run.name, run.manager)),
            Some(code) => Some(format!("{} exited with code {code}", run.name)),
        };
        run.failure = message.map(|message| Issue { severity: Severity::Error, message, path: None, line: None, column: None, source: IssueSource::Build });
        if run.failure.is_some() {
            self.web_issues_changed(cx);
        }
        cx.notify();
    }

    /// A run's terminal was closed: its processes ended with it.
    pub(super) fn web_terminal_closed(&mut self, view: EntityId) {
        if let Some(run) = self.web.runs.iter_mut().find(|r| r.view.entity_id() == view && r.is_running()) {
            run.state = RunState::Stopped;
        }
    }

    pub(super) fn web_running(&self, id: &ScriptId) -> bool {
        self.web.runs.iter().any(|r| r.id == *id && r.is_running())
    }

    pub(super) fn any_web_running(&self) -> bool {
        self.web.runs.iter().any(WebRun::is_running)
    }

    /// Stops `id`'s run; whether it was running.
    pub(super) fn stop_script(&mut self, id: &ScriptId, cx: &mut Context<Self>) -> bool {
        let Some(run) = self.web.runs.iter_mut().find(|r| r.id == *id && r.is_running()) else { return false };
        run.state = RunState::Stopped;
        run.view.update(cx, |view, cx| view.end(cx));
        cx.notify();
        true
    }

    /// Stops every running script; whether one was running.
    pub(super) fn stop_web_runs(&mut self, cx: &mut Context<Self>) -> bool {
        let running: Vec<ScriptId> = self.web.runs.iter().filter(|r| r.is_running()).map(|r| r.id.clone()).collect();
        for id in &running {
            self.stop_script(id, cx);
        }
        !running.is_empty()
    }

    /// The scripts' issues, as build issues.
    pub(super) fn web_issues(&self) -> impl Iterator<Item = &Issue> {
        self.web.runs.iter().flat_map(WebRun::issues)
    }

    fn web_issues_changed(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.buffers.iter().map(|b| b.path.clone()).collect();
        for path in paths {
            self.refresh_diagnostics_for(&path, cx);
        }
    }

    /// The run the activity view tells about: the chosen script's, else
    /// the first one running.
    pub(super) fn shown_web_run(&self, cx: &App) -> Option<&WebRun> {
        let chosen = self.build.read(cx).script.clone();
        self.web.runs.iter().find(|r| Some(&r.id) == chosen.as_ref()).or_else(|| self.web.runs.iter().find(|r| r.is_running()))
    }

    /// Edit Scheme… with a script chosen: its line in `package.json`.
    pub(super) fn show_script_definition(&mut self, app: &web::WebApp, script: &str, window: &mut Window, cx: &mut Context<Self>) {
        let path = app.dir.join("package.json");
        let needle = format!("\"{script}\"");
        let line = std::fs::read_to_string(&path).ok().and_then(|text| {
            // The scripts object's entry, not a dependency of the same name.
            let start = text.find("\"scripts\"")?;
            let at = start + text[start..].find(&needle)?;
            Some(text[..at].matches('\n').count() as u32)
        });
        let target = line.map(|line| super::Target::Line { line, column: 0 });
        self.open_file(path, target, true, window, cx);
    }

    // ---------------------------------------------------------------------
    // The control channel and MCP

    /// The `n`th running script's URL opened again, as its chip in the
    /// activity view does (the control channel's).
    pub fn open_running_web_url(&mut self, n: usize, cx: &mut Context<Self>) -> Result<String, String> {
        let url = self.web.runs.iter().filter(|r| r.is_running()).nth(n).ok_or(format!("no running script {n}"))?.parser.url.clone();
        let url = url.ok_or("it hasn’t printed a URL")?;
        self.open_web_url(&url, cx);
        Ok(url)
    }

    pub(super) fn web_summary(&self, cx: &App) -> serde_json::Value {
        let build = self.build.read(cx);
        let apps: Vec<serde_json::Value> = build
            .web_apps
            .iter()
            .map(|app| {
                serde_json::json!({
                    "label": app.label,
                    "dir": app.dir,
                    "manager": app.manager.name(),
                    "scripts": app.scripts.iter().map(|s| format!("{}: {}", s.name, s.command)).collect::<Vec<_>>(),
                })
            })
            .collect();
        let runs: Vec<serde_json::Value> = self.web.runs.iter().map(|run| self.web_run_json(run, 40, cx)).collect();
        serde_json::json!({
            "loaded": build.web_apps_loaded,
            "apps": apps,
            "chosen": build.script.as_ref().map(|id| build.choice_name(&crate::build::Choice::Script(id.clone()))),
            "runs": runs,
        })
    }

    pub(super) fn web_run_json(&self, run: &WebRun, lines: usize, cx: &App) -> serde_json::Value {
        let view = run.view.read(cx);
        serde_json::json!({
            "name": run.name,
            "state": match run.state {
                RunState::Running => "running".to_string(),
                RunState::Exited(Some(code)) => format!("exited {code}"),
                RunState::Exited(None) => "exited".to_string(),
                RunState::Stopped => "stopped".to_string(),
            },
            "pid": view.pid(),
            "url": run.parser.url,
            "issues": run.issues().map(|i| format!("{:?} {}:{}:{} {}", i.severity, i.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(), i.line.unwrap_or(0), i.column.unwrap_or(0), i.message)).collect::<Vec<_>>(),
            "seconds": run.started.elapsed().as_secs(),
            "command": view.spawn().command.as_ref().map(|(_, args)| args.join(" ")),
            "total_lines": run.total_lines,
            "lines": run.output.iter().skip(run.output.len().saturating_sub(lines)).collect::<Vec<_>>(),
        })
    }
}

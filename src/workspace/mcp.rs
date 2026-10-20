//! What zcode's MCP tools (`crate::mcp`) do to a project window: choose a
//! scheme and run destination, start and stop builds, runs and tests as
//! the Product menu does, open files, read the editor and evaluate in the
//! debugger. The tools wait for results and report them; the window shows
//! everything as if the user had done it.

use std::path::{Path, PathBuf};

use gpui_kit::*;
use serde_json::{Value, json};
use zcode_editor::RopeExt as _;

use super::{Target, Workspace};
use crate::build::web::ScriptId;
use crate::build::{BuildAction, Choice};
use crate::build::log::{Issue, Severity};
use crate::build::xcode::Destination;

impl Workspace {
    /// Chooses a scheme and a run destination (by id or name) as the
    /// toolbar's menus do; either may be left as it is.
    pub fn mcp_select(&mut self, scheme: Option<&str>, destination: Option<&str>, cx: &mut Context<Self>) -> Result<(), String> {
        let build = self.build.clone();
        if let Some(scheme) = scheme {
            // A scheme, or a web app's script (`web: dev`).
            let choice = build.read(cx).find_choice(scheme).ok_or_else(|| format!("No scheme named “{scheme}”, nor a web app’s script. XcodeListSchemes lists them."))?;
            if build.read(cx).choice().as_ref() != Some(&choice) {
                build.update(cx, |build, cx| build.choose(choice, cx));
            }
        }
        if let Some(wanted) = destination {
            let found = build.read(cx).destinations.iter().find(|d| d.id == wanted || d.name == wanted).cloned();
            let destination = found.ok_or_else(|| format!("No run destination “{wanted}” for this scheme. XcodeListRunDestinations lists them."))?;
            build.update(cx, |build, cx| build.set_destination(destination, cx));
        }
        Ok(())
    }

    /// Product ▸ Build, Run or Test (`tests` names some; none runs every
    /// test), as the menu starts it: files saved, the debug area shown.
    /// The run's generation, to wait for.
    pub fn mcp_start(&mut self, action: BuildAction, tests: &[String], window: &mut Window, cx: &mut Context<Self>) -> Result<u64, String> {
        if !self.build.read(cx).can_build() {
            return Err("This folder isn’t an Xcode project or a Swift package; there’s nothing to build.".into());
        }
        match action {
            BuildAction::Test if !tests.is_empty() => {
                let request = self.test_request_named(tests, cx)?;
                self.run_tests(request, window, cx);
            }
            BuildAction::Test => self.test_all(window, cx),
            action => self.start_build(action, window, cx),
        }
        let build = self.build.read(cx);
        if !build.phase.is_busy() {
            let reason = build.log.iter().rev().find(|l| l.raw.starts_with("error: ")).map(|l| l.raw.trim_start_matches("error: ").to_string());
            return Err(reason.unwrap_or_else(|| "The build didn’t start.".into()));
        }
        Ok(build.generation)
    }

    /// Product ▸ Profile with `template` for `time_limit` seconds. The
    /// run's generation, to wait for.
    pub fn mcp_profile(&mut self, template: String, time_limit: u32, window: &mut Window, cx: &mut Context<Self>) -> Result<u64, String> {
        if !self.build.read(cx).can_build() {
            return Err("This folder isn’t an Xcode project or a Swift package; there’s nothing to profile.".into());
        }
        let request = crate::build::profile::ProfileRequest { template: Some(template), build: true, time_limit: Some(time_limit) };
        self.start_recording(request, window, cx);
        let build = self.build.read(cx);
        if !build.phase.is_busy() {
            return Err("Profiling didn’t start.".into());
        }
        Ok(build.generation)
    }

    /// Product ▸ Stop: the chosen target's run, as ⌘. stops it, or
    /// `scheme`'s (a script's, or the build, the app and the debugger).
    /// Whether anything was running.
    pub fn mcp_stop(&mut self, scheme: Option<&str>, cx: &mut Context<Self>) -> Result<bool, String> {
        let choice = match scheme {
            Some(name) => Some(self.build.read(cx).find_choice(name).ok_or_else(|| format!("No scheme named “{name}”, nor a web app’s script. XcodeListSchemes lists them."))?),
            None => None,
        };
        let stopped = match choice {
            Some(Choice::Script(id)) => self.stop_script(&id, cx),
            Some(Choice::Scheme(_)) => {
                let running = self.build.read(cx).phase.is_busy() || self.debugger.read(cx).state.is_active();
                self.debugger.update(cx, |d, cx| d.stop(cx));
                self.build.update(cx, |b, cx| b.stop(cx));
                running
            }
            None => {
                let running = self.build.read(cx).phase.is_busy() || self.debugger.read(cx).state.is_active() || self.any_web_running();
                self.stop(cx);
                running
            }
        };
        cx.notify();
        Ok(stopped)
    }

    /// Stops the scheme's build, its app and the debugger (a tool call
    /// cancelled while it builds), leaving scripts be.
    pub fn mcp_stop_build(&mut self, cx: &mut Context<Self>) {
        self.debugger.update(cx, |d, cx| d.stop(cx));
        self.build.update(cx, |b, cx| b.stop(cx));
        cx.notify();
    }

    /// Run with a script chosen: starts it (again). Its run, to follow.
    pub fn mcp_run_script(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<ScriptId, String> {
        let id = self.build.read(cx).script.clone().ok_or("No script is chosen.")?;
        self.run_script(id.clone(), window, cx)?;
        Ok(id)
    }

    /// A script's run as the tools report it: its state, URL, issues and
    /// the last `lines` of its output.
    pub fn mcp_web_run(&self, id: &ScriptId, lines: usize, cx: &App) -> Option<Value> {
        let run = self.web.runs.iter().find(|r| r.id == *id)?;
        let mut value = self.web_run_json(run, lines, cx);
        value["running"] = run.is_running().into();
        value["issues"] = run.issues().map(issue_json).collect::<Vec<_>>().into();
        Some(value)
    }

    /// Every script run: name, state and URL.
    pub fn mcp_web_runs(&self, cx: &App) -> Vec<Value> {
        self.web
            .runs
            .iter()
            .map(|run| {
                let value = self.web_run_json(run, 0, cx);
                json!({ "name": value["name"], "state": value["state"], "running": run.is_running(), "url": value["url"], "total_lines": run.total_lines })
            })
            .collect()
    }

    /// Whether `id` is running.
    pub fn mcp_script_running(&self, id: &ScriptId) -> bool {
        self.web_running(id)
    }

    /// Opens `path` (relative to the project, or absolute) at a 1-based
    /// line and column.
    pub fn mcp_open(&mut self, path: &str, line: Option<u32>, column: Option<u32>, window: &mut Window, cx: &mut Context<Self>) -> Result<PathBuf, String> {
        let path = self.mcp_path(path);
        // An asset catalog or a set in it opens the catalog's editor.
        if !path.is_file() && crate::xcassets::locate(&path).is_none() {
            return Err(format!("No file at {}", path.display()));
        }
        let target = line.map(|l| Target::Line { line: l.saturating_sub(1), column: column.unwrap_or(1).saturating_sub(1) });
        self.open_file(path.clone(), target, true, window, cx);
        Ok(path.canonicalize().unwrap_or(path))
    }

    pub fn mcp_path(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() { path.to_path_buf() } else { self.project.root.join(path) }
    }

    /// The focused pane's editor: its file, cursor and selection (1-based
    /// lines and columns), and its text when asked.
    pub fn mcp_active_editor(&self, include_text: bool, cx: &App) -> Value {
        let Some((item, buffer)) = self.active_item().zip(self.active_buffer()) else {
            return json!({ "path": Value::Null, "message": "No file is open in the focused editor pane." });
        };
        let state = item.editor.read(cx);
        let text = state.text();
        let range = state.selected_range();
        let point = |offset: usize| {
            let position = text.offset_to_position(offset);
            json!({ "line": position.line + 1, "column": position.character + 1 })
        };
        let cursor = state.cursor_position();
        let mut value = json!({
            "path": buffer.path,
            "language": buffer.language.to_string(),
            "unsaved_changes": buffer.dirty,
            "read_only": buffer.read_only,
            "line": cursor.line + 1,
            "column": cursor.character + 1,
            "selection": { "start": point(range.start), "end": point(range.end) },
            "selected_text": state.selected_text().to_string(),
            "line_count": text.lines_len(),
        });
        if include_text {
            value["text"] = text.to_string().into();
        }
        value
    }

    /// Runs an lldb command (`po x`, `p x`, `v`, `bt`, `expr …`) in the
    /// paused frame, shown in the console as if typed at its prompt.
    pub fn mcp_evaluate(&mut self, command: &str, cx: &mut Context<Self>) -> Result<Task<String>, String> {
        if let Some(reason) = self.mcp_not_paused(cx) {
            return Err(reason);
        }
        let task = self.debugger.update(cx, |debugger, cx| debugger.evaluate_command(command, cx));
        task.ok_or_else(|| "The debugger isn’t paused.".to_string())
    }

    /// Why the debugger can't evaluate now, if it can't.
    pub fn mcp_not_paused(&self, cx: &App) -> Option<String> {
        let debugger = self.debugger.read(cx);
        match (debugger.paused_frame().is_some(), debugger.state.is_active()) {
            (true, _) => None,
            (false, true) => Some("The debugger isn’t paused. Stop at a breakpoint first (or pause), then evaluate.".into()),
            (false, false) => Some("Nothing is being debugged. Run with a breakpoint set to stop in the debugger.".into()),
        }
    }

    /// The tests found in the project's source (as the Test navigator
    /// lists them, by the names RunSomeTests takes) with their latest
    /// results.
    pub fn mcp_test_list(&self, cx: &App) -> Value {
        let statuses = self.current_test_statuses(cx);
        let index = &self.tests.index;
        let tests: Vec<Value> = (0..index.items.len())
            .map(|ix| {
                let item = &index.items[ix];
                let target = index.targets[ix].as_ref().map(|t| t.name.clone());
                let components = item.components().join("/");
                json!({
                    "name": match &target {
                        Some(target) => format!("{target}/{components}"),
                        None => components,
                    },
                    "target": target,
                    "kind": if item.name.is_some() { "test" } else { "suite" },
                    "framework": format!("{:?}", item.framework),
                    "display_name": item.display_name,
                    "path": item.file,
                    "line": item.line + 1,
                    "status": statuses.get(ix).copied().flatten().map(|s| format!("{s:?}").to_lowercase()).unwrap_or_else(|| "not_run".into()),
                })
            })
            .collect();
        let build = self.build.read(cx);
        json!({
            "loaded": index.loaded,
            "tests": tests,
            "test_plan": build.active_test_plan().map(|p| p.name.clone()),
            "last_run": build.last_test.as_ref().map(|r| r.label().unwrap_or_else(|| "All tests".into())),
        })
    }

    /// The last build's errors and warnings, their paths in the project.
    pub fn mcp_build_issues(&self, cx: &App) -> Vec<Issue> {
        self.build
            .read(cx)
            .issues
            .iter()
            .filter(|i| i.severity != Severity::Note)
            .map(|i| Issue { path: i.path.as_deref().map(|p| self.resolve_issue_path(p)), ..i.clone() })
            .collect()
    }

    /// Where the debugger is paused.
    pub fn mcp_debug_location(&self, cx: &App) -> Value {
        let debugger = self.debugger.read(cx);
        let frame = debugger.frames.get(debugger.selected_frame);
        json!({
            "state": format!("{:?}", debugger.state).split([' ', '{']).next().unwrap_or(""),
            "frame": frame.map(|f| json!({ "function": f.name, "path": f.path, "line": f.line })),
        })
    }

    /// The simulator a screenshot is of: `udid`, or the run destination
    /// when it's a simulator, or a booted one. Never a device.
    pub fn mcp_simulator(&self, udid: Option<&str>, cx: &App) -> Result<Destination, String> {
        let build = self.build.read(cx);
        let simulators = || build.all_destinations.iter().filter(|d| d.platform.is_simulator());
        if let Some(udid) = udid {
            return match build.all_destinations.iter().find(|d| d.id == udid || d.name == udid) {
                Some(d) if d.platform.is_simulator() => Ok(d.clone()),
                Some(d) => Err(format!("{} isn’t a simulator; screenshots are of zcode’s window or a simulator.", d.name)),
                // A simulator zcode's list doesn't have yet: simctl knows.
                None => Ok(Destination { id: udid.to_string(), name: udid.to_string(), ..Destination::my_mac() }),
            };
        }
        if let Some(d) = build.destination.as_ref().filter(|d| d.platform.is_simulator()) {
            return Ok(d.clone());
        }
        simulators()
            .find(|d| d.booted)
            .cloned()
            .ok_or_else(|| "No simulator is booted and the run destination isn’t a simulator. Run on one first, or pass its udid.".into())
    }
}

/// An issue as the tools report it.
pub fn issue_json(issue: &Issue) -> Value {
    json!({
        "severity": match issue.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        },
        "message": issue.message,
        "path": issue.path,
        "line": issue.line,
        "column": issue.column,
        "source": format!("{:?}", issue.source).to_lowercase(),
    })
}

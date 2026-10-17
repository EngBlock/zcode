//! The tools zcode's MCP server offers: a deliberate, documented set (see
//! the README's "zcode as an MCP server"), not the control channel's test
//! commands. Each one acts on a project window as the Product menu, the
//! navigators and the editor do, through the same workspace functions.
//!
//! Which window: the agent panel's own, for its agent; else the one named
//! by `project`, the one containing the folder `zcode mcp` was started in,
//! or the frontmost. Tools that change things (`changes`) ask in the agent
//! panel first when its agent calls them.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, AsyncApp, Entity};
use serde_json::{Value, json};

use super::protocol::ToolOutput;
use super::server::Call;
use crate::build::log::{LineStyle, Severity, TestStatus};
use crate::build::{BuildAction, Phase};
use crate::workspace::{McpPermission, Workspace, issue_json};

pub enum ToolError {
    /// The tool ran and couldn't do it: a message for the model.
    Failed(String),
    /// The arguments are wrong: a protocol error.
    InvalidParams(String),
    /// The client cancelled the call: no answer is sent.
    Cancelled,
}

impl From<String> for ToolError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

type Result<T> = std::result::Result<T, ToolError>;

pub struct Tool {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// Starts, stops or evaluates something: asked about when the agent
    /// panel's agent calls it.
    pub changes: bool,
    schema: fn() -> Value,
}

/// `project`: which open project a tool means.
fn project_property() -> Value {
    json!({ "type": "string", "description": "The project's folder (or any path inside it). Defaults to the project zcode mcp was started in, else the frontmost window." })
}

fn schema(properties: Value, required: &[&str]) -> Value {
    let mut properties = properties;
    properties["project"] = project_property();
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

fn selection_properties() -> Value {
    json!({
        "scheme": { "type": "string", "description": "A scheme from list_schemes; chosen in zcode's toolbar as if picked there. Defaults to the current one." },
        "destination": { "type": "string", "description": "A run destination's id or name from list_destinations. Defaults to the current one." },
    })
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "list_schemes",
        title: "List Schemes",
        description: "The project's schemes, build configurations and test plans, and which are chosen.",
        changes: false,
        schema: || schema(json!({}), &[]),
    },
    Tool {
        name: "list_destinations",
        title: "List Run Destinations",
        description: "Where the current scheme can run: My Mac, simulators (with whether they're booted) and connected devices, and which is chosen.",
        changes: false,
        schema: || schema(json!({}), &[]),
    },
    Tool {
        name: "build",
        title: "Build",
        description: "Product ▸ Build (⌘B): saves edited files and builds the scheme for the run destination. Waits for the build (it can take minutes; progress is reported) and returns whether it succeeded, with its errors and warnings and where the full log is.",
        changes: true,
        schema: || schema(selection_properties(), &[]),
    },
    Tool {
        name: "run",
        title: "Run",
        description: "Product ▸ Run (⌘R): builds, then launches the app (on a simulator, booting it, or on the Mac), under the debugger when breakpoints are set. Returns once it's running (plus wait_seconds of its console output) or the build or launch failed. The app keeps running: call stop to end it.",
        changes: true,
        schema: || {
            let mut properties = selection_properties();
            properties["wait_seconds"] = json!({ "type": "number", "minimum": 0, "maximum": 120, "description": "How long to collect console output after launch (default 3)." });
            schema(properties, &[])
        },
    },
    Tool {
        name: "stop",
        title: "Stop",
        description: "Product ▸ Stop (⌘.): stops the build, the running app and the debugger.",
        changes: true,
        schema: || schema(json!({}), &[]),
    },
    Tool {
        name: "test",
        title: "Test",
        description: "Product ▸ Test (⌘U), or only some tests: builds and runs them and returns each test's result (passed, failed, skipped, duration) and the failures with their file and line. Waits for the run; progress is reported.",
        changes: true,
        schema: || {
            let mut properties = selection_properties();
            properties["tests"] = json!({
                "type": "array",
                "items": { "type": "string" },
                "description": "Tests to run, as the Test navigator names them: Target/Class/testMethod, Class/testMethod, Suite/test(), a class, a suite or a whole test target. Omit to run everything the scheme and test plan include.",
            });
            schema(properties, &[])
        },
    },
    Tool {
        name: "get_test_results",
        title: "Get Test Results",
        description: "The latest result of every test that has run (as the Test navigator shows them), without running anything.",
        changes: false,
        schema: || schema(json!({}), &[]),
    },
    Tool {
        name: "get_issues",
        title: "Get Issues",
        description: "The Issue navigator: live diagnostics from sourcekit-lsp for open files and the last build's errors and warnings, with file, line and column.",
        changes: false,
        schema: || {
            schema(
                json!({
                    "path": { "type": "string", "description": "Only this file's issues (relative to the project or absolute)." },
                    "severity": { "type": "string", "enum": ["error", "warning", "all"], "description": "Default all." },
                }),
                &[],
            )
        },
    },
    Tool {
        name: "get_build_log",
        title: "Get Build Log",
        description: "The current or last build's log: condensed as the debug area shows it, or raw xcodebuild / swift output; the last builds' reports with their full log files.",
        changes: false,
        schema: || {
            schema(
                json!({
                    "max_lines": { "type": "integer", "minimum": 1, "maximum": 5000, "description": "The last this many lines (default 200)." },
                    "raw": { "type": "boolean", "description": "The tool's own output rather than the condensed log." },
                    "filter": { "type": "string", "description": "Only lines containing this text." },
                }),
                &[],
            )
        },
    },
    Tool {
        name: "get_console",
        title: "Get Console Output",
        description: "The running (or last run) app's console output and the debugger's.",
        changes: false,
        schema: || schema(json!({ "max_lines": { "type": "integer", "minimum": 1, "maximum": 5000, "description": "The last this many lines (default 200)." } }), &[]),
    },
    Tool {
        name: "open_file",
        title: "Open File",
        description: "Opens a file in zcode's focused editor pane, at a line and column if given, so the user sees it.",
        changes: false,
        schema: || {
            schema(
                json!({
                    "path": { "type": "string", "description": "Relative to the project, or absolute." },
                    "line": { "type": "integer", "minimum": 1 },
                    "column": { "type": "integer", "minimum": 1 },
                }),
                &["path"],
            )
        },
    },
    Tool {
        name: "get_active_editor",
        title: "Get Active Editor",
        description: "The file in zcode's focused editor pane: its path, the cursor, the selection (1-based lines and columns) and the selected text, whether it has unsaved changes, and its text if asked (unsaved edits included).",
        changes: false,
        schema: || schema(json!({ "include_text": { "type": "boolean", "description": "Also return the editor's whole text." } }), &[]),
    },
    Tool {
        name: "debugger_evaluate",
        title: "Evaluate in Debugger",
        description: "Runs an LLDB command in the paused program's selected frame, as typed at the console's (lldb) prompt: po value, p value, v, bt, expr …. Only while the debugger is paused (at a breakpoint).",
        changes: true,
        schema: || schema(json!({ "command": { "type": "string", "description": "e.g. po items.count, v, bt" } }), &["command"]),
    },
    Tool {
        name: "screenshot",
        title: "Screenshot",
        description: "A PNG of zcode's project window, or of a simulator's screen (simctl io screenshot): the run destination when it's a simulator, else a booted one, or the udid given. Never a physical device.",
        changes: false,
        schema: || {
            schema(
                json!({
                    "target": { "type": "string", "enum": ["window", "simulator"], "description": "Default window." },
                    "udid": { "type": "string", "description": "The simulator (id or name from list_destinations)." },
                }),
                &[],
            )
        },
    },
];

pub fn tool(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

/// `tools/list`'s tools, always in this order.
pub fn definitions() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "title": tool.title,
                "description": tool.description,
                "inputSchema": (tool.schema)(),
                "annotations": {
                    "title": tool.title,
                    "readOnlyHint": !tool.changes,
                    "destructiveHint": false,
                    "idempotentHint": !tool.changes,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

pub async fn call(name: &str, args: &Value, call: &Rc<Call>, cx: &mut AsyncApp) -> Result<ToolOutput> {
    let workspace = resolve(args, call, cx)?;
    let json = |value: Value| Ok(ToolOutput::json(value));
    match name {
        "list_schemes" => {
            wait_for_schemes(&workspace, call, cx).await?;
            json(cx.update(|cx| schemes(&workspace, cx)))
        }
        "list_destinations" => {
            wait_for_schemes(&workspace, call, cx).await?;
            json(cx.update(|cx| destinations(&workspace, cx)))
        }
        "build" => product(BuildAction::Build, args, &workspace, call, cx).await,
        "run" => product(BuildAction::Run, args, &workspace, call, cx).await,
        "test" => product(BuildAction::Test, args, &workspace, call, cx).await,
        "stop" => {
            let permission = permit(call, &workspace, "stop", "Stop".into(), args, cx).await?;
            let stopped = cx.update(|cx| workspace.update(cx, |ws, cx| ws.mcp_stop(cx)));
            finish(&permission, true, cx);
            json(json!({ "stopped": stopped }))
        }
        "get_test_results" => json(cx.update(|cx| {
            let ws = workspace.read(cx);
            let build = ws.build.read(cx);
            let mut value = test_summary(build.tests.iter());
            value["last_run"] = build.last_test.as_ref().map(|r| r.label().unwrap_or_else(|| "All tests".into())).into();
            value
        })),
        "get_issues" => {
            let path = string(args, "path")?;
            let severity = string(args, "severity")?.unwrap_or_else(|| "all".into());
            if !["error", "warning", "all"].contains(&severity.as_str()) {
                return Err(ToolError::InvalidParams("severity is error, warning or all".into()));
            }
            json(cx.update(|cx| {
                let ws = workspace.read(cx);
                let only = path.map(|p| {
                    let p = ws.mcp_path(&p);
                    p.canonicalize().unwrap_or(p)
                });
                let issues: Vec<_> = ws
                    .all_issues(cx)
                    .into_iter()
                    .filter(|i| only.as_ref().is_none_or(|p| i.path.as_ref().is_some_and(|ip| ip == p || ip.canonicalize().ok().as_ref() == Some(p))))
                    .filter(|i| match severity.as_str() {
                        "error" => i.severity == Severity::Error,
                        "warning" => i.severity == Severity::Warning,
                        _ => true,
                    })
                    .collect();
                json!({
                    "error_count": issues.iter().filter(|i| i.severity == Severity::Error).count(),
                    "warning_count": issues.iter().filter(|i| i.severity == Severity::Warning).count(),
                    "issues": issues.iter().map(issue_json).collect::<Vec<_>>(),
                })
            }))
        }
        "get_build_log" => {
            let max = integer(args, "max_lines")?.unwrap_or(200).clamp(1, 5000) as usize;
            let raw = boolean(args, "raw")?.unwrap_or(false);
            let filter = string(args, "filter")?;
            json(cx.update(|cx| {
                let ws = workspace.read(cx);
                let build = ws.build.read(cx);
                let lines: Vec<&str> = build
                    .log
                    .iter()
                    .filter_map(|l| if raw { Some(l.raw.as_str()) } else { l.pretty.as_deref() })
                    .filter(|l| filter.as_ref().is_none_or(|f| l.contains(f.as_str())))
                    .collect();
                let shown = &lines[lines.len().saturating_sub(max)..];
                json!({
                    "phase": phase_label(&build.phase),
                    "total_lines": lines.len(),
                    "lines": shown,
                    "reports": build.reports.iter().take(5).map(|r| json!({
                        "action": r.action.verb(),
                        "scheme": r.scheme,
                        "destination": r.destination,
                        "success": r.success,
                        "errors": r.errors,
                        "warnings": r.warnings,
                        "duration_seconds": round(r.duration.as_secs_f64()),
                        "log_path": r.log_path,
                    })).collect::<Vec<_>>(),
                })
            }))
        }
        "get_console" => {
            let max = integer(args, "max_lines")?.unwrap_or(200).clamp(1, 5000) as usize;
            json(cx.update(|cx| {
                let ws = workspace.read(cx);
                let build = ws.build.read(cx);
                let console = &build.console;
                json!({
                    "running": matches!(build.phase, Phase::Running { .. }),
                    "total_lines": console.len(),
                    "lines": &console[console.len().saturating_sub(max)..],
                })
            }))
        }
        "open_file" => {
            let path = string(args, "path")?.ok_or_else(|| ToolError::InvalidParams("path is required".into()))?;
            let (line, column) = (integer(args, "line")?, integer(args, "column")?);
            let window = cx.update(|cx| workspace.read(cx).window);
            let opened = cx
                .update(|cx| window.update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.mcp_open(&path, line.map(|l| l as u32), column.map(|c| c as u32), window, cx))))
                .map_err(|e| e.to_string())??;
            json(json!({ "path": opened, "line": line }))
        }
        "get_active_editor" => {
            let include_text = boolean(args, "include_text")?.unwrap_or(false);
            json(cx.update(|cx| workspace.read(cx).mcp_active_editor(include_text, cx)))
        }
        "debugger_evaluate" => evaluate(args, &workspace, call, cx).await,
        "screenshot" => screenshot(args, &workspace, cx).await,
        _ => Err(ToolError::InvalidParams(format!("Unknown tool: {name}"))),
    }
}

// -------------------------------------------------------------------------
// Which window, and asking first

fn resolve(args: &Value, call: &Call, cx: &mut AsyncApp) -> Result<Entity<Workspace>> {
    let project = string(args, "project")?;
    cx.update(|cx| {
        // The agent panel's agent works on the panel's project.
        if let Some(token) = &call.origin.agent
            && let Some(workspace) = super::agent_workspace(token, cx)
        {
            return workspace.upgrade().ok_or_else(|| ToolError::Failed("The agent’s project window was closed.".into()));
        }
        let open = crate::app::workspaces(cx);
        if open.is_empty() {
            return Err(ToolError::Failed("No project is open in zcode. Open one (zcode <path>) and try again.".into()));
        }
        let contains = |path: &std::path::Path| {
            let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            open.iter().filter(|w| path.starts_with(&w.read(cx).project.root)).max_by_key(|w| w.read(cx).project.root.as_os_str().len()).cloned()
        };
        if let Some(project) = project {
            return contains(std::path::Path::new(&project)).ok_or_else(|| {
                let roots: Vec<String> = open.iter().map(|w| w.read(cx).project.root.display().to_string()).collect();
                ToolError::Failed(format!("No open project contains {project}. Open in zcode: {}", roots.join(", ")))
            });
        }
        if let Some(found) = call.origin.cwd.as_deref().and_then(contains) {
            return Ok(found);
        }
        let front = cx.active_window();
        Ok(open.iter().find(|w| Some(w.read(cx).window) == front).unwrap_or(&open[0]).clone())
    })
}

/// Whether a tool that changes things may run: the agent panel asks the
/// user when its agent called it; other clients ask their users themselves.
async fn permit(call: &Call, workspace: &Entity<Workspace>, tool: &str, title: String, args: &Value, cx: &mut AsyncApp) -> Result<Option<McpPermission>> {
    let Some(token) = &call.origin.agent else { return Ok(None) };
    let ours = cx.update(|cx| super::agent_workspace(token, cx)).is_some_and(|w| w == workspace.downgrade());
    if !ours {
        return Ok(None);
    }
    let title = format!("zcode: {title}");
    let input = args.clone();
    let Some(mut permission) = cx.update(|cx| workspace.update(cx, |ws, cx| ws.ask_agent_tool_permission(tool, title, input, cx))) else {
        return Ok(None);
    };
    let answer = {
        let answer = &mut permission.answer;
        let cancelled = call.until_cancelled(cx.background_executor().clone());
        futures::pin_mut!(cancelled);
        match futures::future::select(answer, cancelled).await {
            futures::future::Either::Left((answer, _)) => Some(answer.unwrap_or(false)),
            futures::future::Either::Right(_) => None,
        }
    };
    match answer {
        Some(true) => Ok(Some(permission)),
        Some(false) => Err(ToolError::Failed(format!("The user didn’t allow {tool} in zcode."))),
        None => {
            cx.update(|cx| permission.withdraw(cx));
            Err(ToolError::Cancelled)
        }
    }
}

fn finish(permission: &Option<McpPermission>, ok: bool, cx: &mut AsyncApp) {
    if let Some(permission) = permission {
        cx.update(|cx| permission.finish(ok, cx));
    }
}

// -------------------------------------------------------------------------
// Schemes and destinations

async fn wait_for_schemes(workspace: &Entity<Workspace>, call: &Call, cx: &mut AsyncApp) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while cx.update(|cx| workspace.read(cx).build.read(cx).schemes_loading) {
        if call.cancelled() {
            return Err(ToolError::Cancelled);
        }
        if Instant::now() > deadline {
            return Err(ToolError::Failed("The project’s schemes are still loading; try again in a moment.".into()));
        }
        cx.background_executor().timer(Duration::from_millis(200)).await;
    }
    Ok(())
}

fn schemes(workspace: &Entity<Workspace>, cx: &gpui_kit::App) -> Value {
    let ws = workspace.read(cx);
    let build = ws.build.read(cx);
    json!({
        "project": { "root": ws.project.root, "name": ws.project.name, "kind": project_kind(&ws.project.kind) },
        "schemes": build.schemes.schemes,
        "scheme": build.scheme,
        "configurations": build.schemes.configurations,
        "configuration": build.configuration,
        "test_plans": build.test_plans.iter().map(|p| json!({ "name": p.name, "default": p.is_default })).collect::<Vec<_>>(),
        "test_plan": build.active_test_plan().map(|p| p.name.clone()),
        "error": build.schemes_error,
    })
}

fn project_kind(kind: &crate::project::ProjectKind) -> &'static str {
    use crate::project::ProjectKind;
    match kind {
        ProjectKind::XcodeWorkspace(_) => "xcode_workspace",
        ProjectKind::XcodeProject(_) => "xcode_project",
        ProjectKind::SwiftPackage => "swift_package",
        ProjectKind::Folder => "folder",
    }
}

fn destinations(workspace: &Entity<Workspace>, cx: &gpui_kit::App) -> Value {
    let build = workspace.read(cx).build.read(cx);
    let destination = |d: &crate::build::xcode::Destination| {
        let kind = if d.device.is_some() {
            "device"
        } else if d.platform.is_simulator() {
            "simulator"
        } else if d.id.starts_with("generic:") {
            "any_device"
        } else {
            "mac"
        };
        let mut value = json!({ "id": d.id, "name": d.name, "kind": kind, "platform": d.platform.os_name(), "runtime": d.runtime });
        if d.platform.is_simulator() {
            value["booted"] = d.booted.into();
        }
        if let Some(device) = &d.device {
            value["connected"] = device.connected.into();
            value["unavailable"] = crate::build::devices::unavailable_message(d).into();
        }
        value
    };
    json!({
        "destinations": build.destinations.iter().map(destination).collect::<Vec<_>>(),
        "destination": build.destination.as_ref().map(|d| d.id.clone()),
    })
}

// -------------------------------------------------------------------------
// Build, run, test

async fn product(action: BuildAction, args: &Value, workspace: &Entity<Workspace>, call: &Call, cx: &mut AsyncApp) -> Result<ToolOutput> {
    let scheme = string(args, "scheme")?;
    let destination = string(args, "destination")?;
    let tests = match args.get("tests") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) if action == BuildAction::Test => {
            items.iter().map(|t| t.as_str().map(str::to_string)).collect::<Option<Vec<_>>>().ok_or_else(|| ToolError::InvalidParams("tests are strings".into()))?
        }
        Some(_) => return Err(ToolError::InvalidParams("tests is an array of test names".into())),
    };
    let wait = match args.get("wait_seconds") {
        None | Some(Value::Null) => 3.,
        Some(value) => value.as_f64().ok_or_else(|| ToolError::InvalidParams("wait_seconds is a number".into()))?.clamp(0., 120.),
    };
    wait_for_schemes(workspace, call, cx).await?;
    cx.update(|cx| workspace.update(cx, |ws, cx| ws.mcp_select(scheme.as_deref(), destination.as_deref(), cx)))?;
    let scheme_name = cx.update(|cx| workspace.read(cx).build.read(cx).scheme.clone()).unwrap_or_default();
    let title = match (action, tests.len()) {
        (BuildAction::Test, 0) => format!("Test “{scheme_name}”"),
        (BuildAction::Test, 1) => format!("Test {}", tests[0]),
        (BuildAction::Test, n) => format!("Test {n} tests"),
        (BuildAction::Run, _) => format!("Run “{scheme_name}”"),
        _ => format!("Build “{scheme_name}”"),
    };
    let tool = match action {
        BuildAction::Test => "test",
        BuildAction::Run => "run",
        _ => "build",
    };
    let permission = permit(call, workspace, tool, title, args, cx).await?;
    let result = run_product(action, &tests, wait, workspace, call, cx).await;
    finish(&permission, result.as_ref().is_ok_and(|o| o.structured["success"] == true), cx);
    result
}

async fn run_product(action: BuildAction, tests: &[String], wait: f64, workspace: &Entity<Workspace>, call: &Call, cx: &mut AsyncApp) -> Result<ToolOutput> {
    let window = cx.update(|cx| workspace.read(cx).window);
    let reports_before = cx.update(|cx| workspace.read(cx).build.read(cx).reports.len());
    let generation = cx
        .update(|cx| window.update(cx, |_, window, cx| workspace.update(cx, |ws, cx| ws.mcp_start(action, tests, window, cx))))
        .map_err(|e| e.to_string())??;
    let started = Instant::now();
    let mut last_message = String::new();
    let mut last_sent = Instant::now();
    let state = |cx: &mut AsyncApp| {
        cx.update(|cx| {
            let build = workspace.read(cx).build.read(cx);
            let step = build.log.iter().rev().find(|l| l.style == LineStyle::Step).and_then(|l| l.pretty.clone());
            let message = match &build.phase {
                Phase::Building { action: BuildAction::Test, .. } if !build.tests.is_empty() => {
                    let failed = build.tests.iter().filter(|t| t.status == TestStatus::Failed).count();
                    format!("Testing: {} tests run, {failed} failed", build.tests.len())
                }
                phase => match step {
                    Some(step) if matches!(phase, Phase::Building { .. }) => format!("{}: {step}", phase_label(phase)),
                    _ => phase_label(phase).to_string(),
                },
            };
            (build.generation, build.phase.clone(), message)
        })
    };
    let phase = loop {
        cx.background_executor().timer(Duration::from_millis(250)).await;
        let (current, phase, message) = state(cx);
        if current != generation {
            return Err(ToolError::Failed("Another build, run or test started in zcode and replaced this one.".into()));
        }
        if call.cancelled() {
            cx.update(|cx| workspace.update(cx, |ws, cx| ws.mcp_stop(cx)));
            return Err(ToolError::Cancelled);
        }
        let done = match action {
            BuildAction::Run => matches!(phase, Phase::Running { .. }) || !phase.is_busy(),
            _ => !phase.is_busy(),
        };
        if done {
            break phase;
        }
        if message != last_message || last_sent.elapsed() > Duration::from_secs(5) {
            call.progress(&message);
            last_message = message;
            last_sent = Instant::now();
        }
    };
    // A run: what the app prints at first.
    if action == BuildAction::Run && matches!(phase, Phase::Running { .. }) && wait > 0. {
        call.progress("Running");
        let until = Instant::now() + Duration::from_secs_f64(wait);
        while Instant::now() < until {
            cx.background_executor().timer(Duration::from_millis(200)).await;
            if call.cancelled() {
                cx.update(|cx| workspace.update(cx, |ws, cx| ws.mcp_stop(cx)));
                return Err(ToolError::Cancelled);
            }
            if !cx.update(|cx| workspace.read(cx).build.read(cx).phase.is_busy()) {
                break;
            }
        }
    }
    Ok(ToolOutput::json(cx.update(|cx| product_result(action, reports_before, started, workspace, cx))))
}

fn product_result(action: BuildAction, reports_before: usize, started: Instant, workspace: &Entity<Workspace>, cx: &gpui_kit::App) -> Value {
    let ws = workspace.read(cx);
    let build = ws.build.read(cx);
    let (status, success) = match &build.phase {
        Phase::Running { .. } => ("running", true),
        Phase::Finished { action: BuildAction::Run, success, .. } if action == BuildAction::Run => (if *success { "finished" } else { "failed" }, *success),
        Phase::Finished { success: true, .. } => ("succeeded", true),
        Phase::Finished { success: false, .. } => ("failed", false),
        Phase::Stopped => ("stopped", false),
        _ => ("unknown", false),
    };
    let issues = ws.mcp_build_issues(cx);
    let errors: Vec<Value> = issues.iter().filter(|i| i.severity == Severity::Error).map(issue_json).collect();
    let warnings: Vec<Value> = issues.iter().filter(|i| i.severity == Severity::Warning).map(issue_json).collect();
    let report = (build.reports.len() > reports_before).then(|| build.reports.first()).flatten();
    let mut value = json!({
        "action": action.verb().to_lowercase(),
        "status": status,
        "success": success,
        "scheme": build.scheme,
        "destination": build.destination.as_ref().map(|d| d.name.clone()),
        "duration_seconds": round(started.elapsed().as_secs_f64()),
        "error_count": errors.len(),
        "warning_count": warnings.len(),
        "errors": errors,
        "warnings": warnings.into_iter().take(100).collect::<Vec<_>>(),
        "log_path": report.map(|r| r.log_path.clone()),
    });
    match action {
        BuildAction::Run => {
            value["action"] = "run".into();
            value["debugging"] = ws.debugger.read(cx).state.is_active().into();
            let console = &build.console;
            value["console"] = console[console.len().saturating_sub(200)..].into();
            if status == "running" {
                value["note"] = "The app keeps running: get_console reads its output, stop ends it.".into();
            }
        }
        BuildAction::Test => {
            let request = build.last_test.clone().unwrap_or_default();
            let summary = test_summary(build.tests.iter().filter(|t| request.is_all() || request.only.iter().any(|s| s.covers(t))));
            for key in ["passed", "failed", "skipped", "total", "tests"] {
                value[key] = summary[key].clone();
            }
            value["selection"] = request.label().unwrap_or_else(|| "All tests".into()).into();
            value["failures"] = value["errors"].clone();
            // A failed test fails the run even when it built.
            if summary["failed"].as_u64().unwrap_or(0) > 0 {
                value["success"] = false.into();
                value["status"] = "failed".into();
            }
        }
        _ => {}
    }
    value
}

fn test_summary<'a>(tests: impl Iterator<Item = &'a crate::build::log::TestResult>) -> Value {
    let tests: Vec<_> = tests.collect();
    let count = |status| tests.iter().filter(|t| t.status == status).count();
    json!({
        "passed": count(TestStatus::Passed),
        "failed": count(TestStatus::Failed),
        "skipped": count(TestStatus::Skipped),
        "total": tests.len(),
        "tests": tests.iter().map(|t| json!({
            "name": t.full_name(),
            "module": t.module,
            "status": format!("{:?}", t.status).to_lowercase(),
            "duration_seconds": t.duration,
        })).collect::<Vec<_>>(),
    })
}

fn phase_label(phase: &Phase) -> &'static str {
    match phase {
        Phase::Idle => "idle",
        Phase::Preparing(_) => "preparing",
        Phase::Building { action: BuildAction::Test, .. } => "testing",
        Phase::Building { action: BuildAction::Clean, .. } => "cleaning",
        Phase::Building { .. } => "building",
        Phase::Launching => "launching",
        Phase::Running { .. } => "running",
        Phase::Finished { success: true, .. } => "succeeded",
        Phase::Finished { success: false, .. } => "failed",
        Phase::Stopped => "stopped",
    }
}

// -------------------------------------------------------------------------
// The debugger and screenshots

async fn evaluate(args: &Value, workspace: &Entity<Workspace>, call: &Call, cx: &mut AsyncApp) -> Result<ToolOutput> {
    let command = string(args, "command")?.filter(|c| !c.trim().is_empty()).ok_or_else(|| ToolError::InvalidParams("command is required".into()))?;
    // Not worth asking about when it can't run.
    if let Some(reason) = cx.update(|cx| workspace.read(cx).mcp_not_paused(cx)) {
        return Err(reason.into());
    }
    let permission = permit(call, workspace, "debugger_evaluate", format!("Evaluate `{command}`"), args, cx).await?;
    let task = cx.update(|cx| workspace.update(cx, |ws, cx| ws.mcp_evaluate(&command, cx)));
    let task = match task {
        Ok(task) => task,
        Err(message) => {
            finish(&permission, false, cx);
            return Err(message.into());
        }
    };
    let cancelled = call.until_cancelled(cx.background_executor().clone());
    futures::pin_mut!(cancelled);
    let output = match futures::future::select(task, cancelled).await {
        futures::future::Either::Left((output, _)) => output,
        futures::future::Either::Right(_) => return Err(ToolError::Cancelled),
    };
    finish(&permission, true, cx);
    Ok(ToolOutput::json(cx.update(|cx| {
        let mut value = workspace.read(cx).mcp_debug_location(cx);
        value["command"] = command.into();
        value["output"] = output.into();
        value
    })))
}

async fn screenshot(args: &Value, workspace: &Entity<Workspace>, cx: &mut AsyncApp) -> Result<ToolOutput> {
    let target = string(args, "target")?.unwrap_or_else(|| "window".into());
    let (png, mut value) = match target.as_str() {
        "window" => {
            let window = cx.update(|cx| workspace.read(cx).window);
            let (number, scale) = cx
                .update(|cx| window.update(cx, |_, window, _| crate::screenshot::window_number(window).map(|n| (n, window.scale_factor()))))
                .ok()
                .flatten()
                .ok_or_else(|| ToolError::Failed("Couldn’t find the project’s window.".into()))?;
            (crate::screenshot::window_png(number, scale)?, json!({ "target": "window" }))
        }
        "simulator" => {
            let udid = string(args, "udid")?;
            let simulator = cx.update(|cx| workspace.read(cx).mcp_simulator(udid.as_deref(), cx))?;
            let id = simulator.id.clone();
            let png = cx.background_spawn(async move { crate::screenshot::simulator_png(&id) }).await?;
            (png, json!({ "target": "simulator", "simulator": { "udid": simulator.id, "name": simulator.name } }))
        }
        _ => return Err(ToolError::InvalidParams("target is window or simulator".into())),
    };
    if let Some((width, height)) = png_size(&png) {
        value["width"] = width.into();
        value["height"] = height.into();
    }
    Ok(ToolOutput { structured: value, images: vec![png] })
}

/// A PNG's width and height, from its header.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        return None;
    }
    let be = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
    Some((be(16), be(20)))
}

// -------------------------------------------------------------------------
// Arguments

fn string(args: &Value, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(ToolError::InvalidParams(format!("{key} is a string"))),
    }
}

fn integer(args: &Value, key: &str) -> Result<Option<i64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_i64().filter(|n| *n >= 0).map(Some).ok_or_else(|| ToolError::InvalidParams(format!("{key} is a whole number"))),
    }
}

fn boolean(args: &Value, key: &str) -> Result<Option<bool>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_bool().map(Some).ok_or_else(|| ToolError::InvalidParams(format!("{key} is true or false"))),
    }
}

fn round(seconds: f64) -> f64 {
    (seconds * 100.).round() / 100.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_an_object_schema_and_hints() {
        let definitions = definitions();
        let names: Vec<&str> = definitions.iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "list_schemes", "list_destinations", "build", "run", "stop", "test", "get_test_results", "get_issues", "get_build_log", "get_console",
                "open_file", "get_active_editor", "debugger_evaluate", "screenshot",
            ]
        );
        for definition in &definitions {
            let schema = &definition["inputSchema"];
            assert_eq!(schema["type"], "object", "{definition}");
            assert!(schema["properties"]["project"].is_object(), "{definition}");
            let name = definition["name"].as_str().unwrap();
            assert_eq!(definition["annotations"]["readOnlyHint"], !tool(name).unwrap().changes);
        }
        let changing: Vec<&str> = TOOLS.iter().filter(|t| t.changes).map(|t| t.name).collect();
        assert_eq!(changing, ["build", "run", "stop", "test", "debugger_evaluate"]);
        assert_eq!(definitions[10]["inputSchema"]["required"], json!(["path"]));
    }

    #[test]
    fn arguments_are_checked() {
        let args = json!({ "path": "A.swift", "line": 3, "raw": true, "bad": 1.5 });
        assert_eq!(string(&args, "path").ok().flatten().as_deref(), Some("A.swift"));
        assert!(matches!(string(&args, "line"), Err(ToolError::InvalidParams(_))));
        assert_eq!(integer(&args, "line").ok().flatten(), Some(3));
        assert!(matches!(integer(&args, "bad"), Err(ToolError::InvalidParams(_))));
        assert_eq!(boolean(&args, "raw").ok().flatten(), Some(true));
        assert!(matches!(boolean(&args, "missing"), Ok(None)));
    }

    #[test]
    fn png_sizes_come_from_the_header() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((640, 480)));
        assert_eq!(png_size(b"not a png at all, really not"), None);
    }
}

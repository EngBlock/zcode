//! What the workspace does when its models report something: the language
//! server, builds and runs, the debugger and the project editor.

use std::path::{Path, PathBuf};

use gpui_kit::*;

use super::{DebugTab, NavigatorTab, ProjectSettings, ProjectSettingsEvent, Target, Workspace};
use crate::build::{BuildAction, BuildController, BuildEvent};
use crate::debugger::{DebugState, DebugTarget, Debugger, DebuggerEvent};
use crate::lsp::{LspStore, LspStoreEvent};

impl Workspace {
    pub(super) fn on_project_settings_event(
        &mut self,
        _: &Entity<ProjectSettings>,
        event: &ProjectSettingsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ProjectSettingsEvent::OpenFile(path) => self.open_file(path.clone(), None, true, window, cx),
            // Schemes, configurations and supported platforms may have changed.
            ProjectSettingsEvent::Changed => {
                self.build.update(cx, |build, cx| build.reload(cx));
                self.refresh_project_index(cx);
            }
            // An Info.plist or entitlements file made (or undone): the
            // navigator lists it.
            ProjectSettingsEvent::FilesChanged => self.rescan(cx),
        }
    }

    pub(super) fn on_lsp_event(
        &mut self,
        _: &Entity<LspStore>,
        event: &LspStoreEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            LspStoreEvent::DiagnosticsChanged(path) => {
                self.refresh_diagnostics_for(path, cx);
                self.semantic_tokens_diagnosed(path, cx);
                cx.notify();
            }
            // Opened at a server, first or again after it was closed while
            // hidden: its tokens and hints are asked for (again).
            LspStoreEvent::DocumentOpened(path) => {
                self.schedule_semantic_tokens(path, std::time::Duration::ZERO, cx);
                self.inlay_hints_opened(path, cx);
            }
            LspStoreEvent::StatusChanged => cx.notify(),
            // Ready (again, after a restart): what it offers may have changed.
            LspStoreEvent::ServerReady => {
                self.refresh_semantic_tokens(cx);
                self.refresh_inlay_hints(cx);
                cx.notify();
            }
            LspStoreEvent::ApplyEdit(edit) => {
                self.apply_workspace_edit(edit.clone(), window, cx);
            }
            LspStoreEvent::RefreshInlayHints => self.refresh_inlay_hints(cx),
            LspStoreEvent::RefreshSemanticTokens => self.refresh_semantic_tokens(cx),
        }
    }

    pub(super) fn on_build_event(
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
                // Through the build server, its graph follows the database
                // (`crate::bsp::model`); otherwise sourcekit-lsp reads it.
                let cache_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                let launch = Self::sourcekit_launch(&self.project, &cache_dir, &self.settings.current);
                let db = path.clone();
                self.lsp.update(cx, |lsp, cx| {
                    let through_database = launch.build_server.is_none();
                    lsp.set_sourcekit(launch, cx);
                    if through_database {
                        lsp.files_changed(&[db], cx);
                    }
                });
            }
            BuildEvent::TestsChanged => {}
            BuildEvent::TemplatesLoaded => self.show_template_chooser(window, cx),
            BuildEvent::Archived(path) => {
                // Opening a tab from inside the build's update: after it.
                let path = path.clone();
                cx.defer_in(window, move |this, window, cx| this.archived(path, window, cx));
            }
            BuildEvent::Distributed => self.distributed(cx),
            BuildEvent::StoredSchemeOptionsChanged => self.save_session(cx),
            BuildEvent::ConfigurationChanged => {
                // A package's test targets arrive with its schemes.
                self.assign_test_targets(cx);
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
                self.follow_activity(cx);
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
                // Builds resolve packages too.
                if matches!(phase, crate::build::Phase::Finished { .. }) {
                    self.refresh_packages(cx);
                }
                let _ = window;
            }
            BuildEvent::AttachDebugger { pid } => {
                let pid = *pid;
                self.debugger
                    .update(cx, |d, cx| d.start(DebugTarget::Attach { pid }, cx));
            }
            BuildEvent::AttachDeviceDebugger(attach) => {
                let target = DebugTarget::AttachDevice(attach.clone());
                self.debugger.update(cx, |d, cx| d.start(target, cx));
            }
            BuildEvent::LaunchDebugger { program, cwd, args, env } => {
                let target = DebugTarget::Launch {
                    program: program.clone(),
                    cwd: cwd.clone(),
                    args: args.clone(),
                    env: env.clone(),
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

    pub(super) fn on_debugger_event(
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
            DebuggerEvent::BreakpointsChanged => {
                let paths: Vec<PathBuf> = self.buffers.iter().map(|b| b.path.clone()).collect();
                for path in paths {
                    self.anchor_breakpoints(&path, false, cx);
                }
            }
            DebuggerEvent::Stopped { path, line } => {
                // The console's `(lldb)` prompt appears while paused.
                self.console_input(window, cx);
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
}

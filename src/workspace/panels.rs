//! Toolbar, debug area, status bar, and the workspace's top-level render
//! (the editor area is `pane_view.rs`).

use std::ops::Range;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Selectable as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, TitleBar, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::assets::IconName;

use super::RemoteAction;

use super::edit_scheme::SchemeAction;
use super::pickers::{self, destination_icon};
use super::{NavigatorTab, SplitAxis, Workspace};
use crate::actions::*;
use crate::build::log::{LineStyle, Severity};
use crate::build::{BuildAction, Phase};
use crate::debugger::DebugState;
use crate::lsp::LspStatus;
use crate::lsp::hierarchy::HierarchyKind;
use crate::perf::Perf;
use crate::picker::Picker;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugTab {
    Console,
    BuildLog,
    /// The debug area's terminals (`terminals.rs`).
    Terminal,
}

const LOG_LINE_HEIGHT: f32 = 18.;

impl Workspace {
    // ------------------------------------------------------------------
    // Toolbar

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let build = self.build.read(cx);
        let busy = build.phase.is_busy();
        let can_build = build.can_build();
        let scheme_label = build
            .scheme
            .clone()
            .unwrap_or_else(|| if build.schemes_loading { "Loading…".into() } else { "No Scheme".into() });
        let destination = build.destination.clone();

        let build_entity = self.build.clone();
        let scheme_button = Button::new("scheme")
            .label(scheme_label)
            .icon(IconName::Hammer)
            .ghost()
            .small()
            .disabled(!can_build)
            .dropdown_menu(move |menu, _, cx| {
                let state = build_entity.read(cx);
                let mut menu = menu.label("Schemes");
                for scheme in state.schemes.schemes.clone() {
                    let checked = state.scheme.as_ref() == Some(&scheme);
                    let build = build_entity.clone();
                    let name = scheme.clone();
                    menu = menu.item(PopupMenuItem::new(scheme).checked(checked).on_click(move |_, _, cx| {
                        build.update(cx, |b, cx| b.set_scheme(name.clone(), cx));
                    }));
                }
                menu = menu.separator().label("Build Configuration");
                for configuration in state.schemes.configurations.clone() {
                    let checked = state.configuration == configuration;
                    let build = build_entity.clone();
                    let name = configuration.clone();
                    menu = menu.item(PopupMenuItem::new(configuration).checked(checked).on_click(
                        move |_, _, cx| {
                            build.update(cx, |b, cx| b.set_configuration(name.clone(), cx));
                        },
                    ));
                }
                menu
            });

        let build_entity = self.build.clone();
        let destination_button = Button::new("destination")
            .label(
                destination
                    .as_ref()
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| "No Destination".into()),
            )
            .when_some(destination.as_ref(), |b, d| b.icon(destination_icon(d)))
            .ghost()
            .small()
            .disabled(!can_build)
            .dropdown_menu(move |menu, _, cx| {
                let state = build_entity.read(cx);
                let current = state.destination.as_ref().map(|d| d.id.clone());
                let mut menu = menu.scrollable(true).max_h(px(480.));
                let mut last_group = String::new();
                for destination in state.destinations.clone() {
                    let group = destination.group();
                    if group != last_group {
                        menu = menu.label(group.clone());
                        last_group = group;
                    }
                    let checked = current.as_ref() == Some(&destination.id);
                    let build = build_entity.clone();
                    let label = destination.menu_label();
                    let icon = destination_icon(&destination);
                    menu = menu.item(PopupMenuItem::new(label).icon(icon).checked(checked).on_click(
                        move |_, _, cx| {
                            build.update(cx, |b, cx| b.set_destination(destination.clone(), cx));
                        },
                    ));
                }
                menu.separator().item(PopupMenuItem::new("Refresh Destinations").on_click({
                    let build = build_entity.clone();
                    move |_, _, cx| build.update(cx, |b, cx| b.reload(cx))
                }))
            });

        TitleBar::new()
            .h(px(40.))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("toggle-navigator")
                            .icon(IconName::PanelLeft)
                            .ghost()
                            .small()
                            .selected(self.show_navigator)
                            .tooltip("Show/Hide Navigator")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_navigator = !this.show_navigator;
                                cx.notify();
                            })),
                    )
                    .child(div().w(px(8.)))
                    .child(
                        Button::new("run")
                            .icon(IconName::Play)
                            .ghost()
                            .small()
                            .disabled(!can_build)
                            .tooltip("Run (⌘R)")
                            .on_click(cx.listener(|this, _, window, cx| this.start_build(BuildAction::Run, window, cx))),
                    )
                    .child(
                        Button::new("stop")
                            .icon(IconName::Square)
                            .ghost()
                            .small()
                            .disabled(!busy)
                            .tooltip("Stop (⌘.)")
                            .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                    )
                    .child(div().w(px(8.)))
                    .child(scheme_button)
                    .child(Icon::new(IconName::ChevronRight).xsmall().text_color(theme.muted_foreground))
                    .child(destination_button),
            )
            .child(self.render_activity(cx))
            .child(
                h_flex()
                    .gap_1()
                    .pr_2()
                    .child(
                        Button::new("quick-actions")
                            .icon(IconName::Command)
                            .ghost()
                            .small()
                            .tooltip("Quick Actions (⇧⌘A)")
                            .on_click(cx.listener(|this, _, window, cx| this.quick_actions(&QuickActions, window, cx))),
                    )
                    .child(
                        Button::new("toggle-debug")
                            .icon(IconName::PanelBottom)
                            .ghost()
                            .small()
                            .selected(self.show_debug_area)
                            .tooltip("Show/Hide Debug Area (⇧⌘Y)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_debug_area = !this.show_debug_area;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("toggle-inspector")
                            .icon(IconName::PanelRight)
                            .ghost()
                            .small()
                            .selected(self.show_inspector)
                            .tooltip("Show/Hide Inspector")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_inspector(cx))),
                    )
                    .child(
                        Button::new("toggle-agent")
                            .icon(IconName::Bot)
                            .ghost()
                            .small()
                            .selected(self.agent.shown)
                            .tooltip("Show Coding Assistant (⌘0)")
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_agent_panel(window, cx))),
                    ),
            )
    }

    /// Xcode's activity view: project, current activity, issue counts.
    fn render_activity(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let build = self.build.read(cx);
        let scheme = build.scheme.clone().unwrap_or_else(|| self.project.name.clone());
        let (status, busy): (String, bool) = match &build.phase {
            // Package work, unless a build or run is under way.
            phase if !phase.is_busy() && self.packages.activity.is_some() => {
                (self.packages.activity.clone().unwrap_or_default(), true)
            }
            Phase::Idle if build.indexing => ("Preparing editor functionality…".into(), true),
            Phase::Idle => ("Ready".into(), false),
            Phase::Preparing(action) => (format!("Preparing to {}…", action.verb().to_lowercase()), true),
            Phase::Building { action, started } => (
                format!(
                    "{} {} — {:.0}s",
                    match action {
                        BuildAction::Test => "Testing",
                        BuildAction::Clean => "Cleaning",
                        _ => "Building",
                    },
                    match action {
                        BuildAction::Test => build.last_test.as_ref().and_then(|r| r.label()).unwrap_or(scheme),
                        _ => scheme,
                    },
                    started.elapsed().as_secs_f32()
                ),
                true,
            ),
            Phase::Launching => (format!("Launching {scheme}…"), true),
            Phase::Running { .. } if matches!(self.debugger.read(cx).state, DebugState::Stopped { .. }) => (
                match &self.debugger.read(cx).state {
                    DebugState::Stopped { description, reason, .. } => format!(
                        "Paused {scheme} — {}",
                        description.clone().unwrap_or_else(|| reason.clone())
                    ),
                    _ => String::new(),
                },
                false,
            ),
            Phase::Running { .. } => (
                format!(
                    "Running {scheme} on {}",
                    build.destination.as_ref().map(|d| d.name.as_str()).unwrap_or("My Mac")
                ),
                true,
            ),
            Phase::Finished { action, success, duration } => (
                format!(
                    "{} {} | {:.1}s",
                    match action {
                        BuildAction::Run => "Finished running",
                        BuildAction::Test => "Test",
                        BuildAction::Clean => "Clean",
                        BuildAction::Build => "Build",
                    },
                    match (action, success) {
                        (BuildAction::Run, _) => String::new(),
                        (_, true) => "Succeeded".into(),
                        (_, false) => "Failed".into(),
                    },
                    duration.as_secs_f32()
                ),
                false,
            ),
            Phase::Stopped => ("Stopped".into(), false),
        };
        let issues = self.all_issues(cx);
        let errors = issues.iter().filter(|i| i.severity == Severity::Error).count();
        let warnings = issues.iter().filter(|i| i.severity == Severity::Warning).count();
        h_flex()
            .id("activity")
            .w(px(460.))
            .h(px(26.))
            .px_3()
            .gap_2()
            .rounded(theme.radius_lg)
            .bg(theme.secondary)
            .border_1()
            .border_color(theme.border)
            .text_xs()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(self.project.name.clone()))
            .child(div().text_color(theme.muted_foreground).child("|"))
            .when(busy, |el| el.child(Spinner::new().xsmall()))
            .child(div().flex_1().truncate().child(status))
            .when(warnings > 0, |el| {
                el.child(
                    h_flex()
                        .gap_0p5()
                        .child(Icon::new(IconName::TriangleAlert).xsmall().text_color(theme.warning))
                        .child(warnings.to_string()),
                )
            })
            .when(errors > 0, |el| {
                el.child(
                    h_flex()
                        .gap_0p5()
                        .child(Icon::new(IconName::CircleX).xsmall().text_color(theme.danger))
                        .child(errors.to_string()),
                )
            })
            .on_click(cx.listener(|this, _, window, cx| {
                this.show_navigator_tab(NavigatorTab::Issues, window, cx);
            }))
    }

    pub(super) fn start_build(&mut self, action: BuildAction, window: &mut Window, cx: &mut Context<Self>) {
        if action == BuildAction::Test {
            self.test_all(window, cx);
            return;
        }
        // Like Xcode, save edited files before building. The build itself
        // refreshes the index, so skip the scheduled background refresh.
        self.save_all(window, cx);
        self.index_refresh = None;
        if action == BuildAction::Run || self.build.read(cx).log.is_empty() {
            self.show_debug_area = true;
        }
        if action != BuildAction::Run {
            self.debug_tab = DebugTab::BuildLog;
        }
        if action == BuildAction::Run {
            // Like Xcode: run under the debugger when breakpoints are active.
            let debug = self.debugger.read(cx).wants_debugging();
            self.build.update(cx, |b, cx| b.run(debug, cx));
        } else {
            self.build.update(cx, |b, cx| b.start(action, cx));
        }
        cx.notify();
    }

    pub(super) fn stop(&mut self, cx: &mut Context<Self>) {
        self.debugger.update(cx, |d, cx| d.stop(cx));
        self.build.update(cx, |b, cx| b.stop(cx));
    }

    // ------------------------------------------------------------------
    // Debug area

    fn render_debug_area(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tab = self.debug_tab;
        let debug_state = self.debugger.read(cx).state.clone();
        let debugging = debug_state.is_active();
        let stopped = matches!(debug_state, DebugState::Stopped { .. });
        let header = h_flex()
            .h(px(28.))
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .when(debugging, |el| {
                el.child(
                    Button::new("dbg-continue")
                        .icon(if stopped { IconName::Play } else { IconName::Pause })
                        .xsmall()
                        .ghost()
                        .tooltip(if stopped { "Continue (⌃⌘Y)" } else { "Pause (⌃⌘Y)" })
                        .on_click(cx.listener(|this, _, _, cx| this.debugger.update(cx, |d, cx| d.continue_or_pause(cx)))),
                )
                .child(
                    Button::new("dbg-over")
                        .icon(IconName::Redo)
                        .xsmall()
                        .ghost()
                        .disabled(!stopped)
                        .tooltip("Step Over (F6)")
                        .on_click(cx.listener(|this, _, _, cx| this.debugger.update(cx, |d, cx| d.step_over(cx)))),
                )
                .child(
                    Button::new("dbg-into")
                        .icon(IconName::ArrowDownToLine)
                        .xsmall()
                        .ghost()
                        .disabled(!stopped)
                        .tooltip("Step Into (F7)")
                        .on_click(cx.listener(|this, _, _, cx| this.debugger.update(cx, |d, cx| d.step_into(cx)))),
                )
                .child(
                    Button::new("dbg-out")
                        .icon(IconName::ArrowUpFromLine)
                        .xsmall()
                        .ghost()
                        .disabled(!stopped)
                        .tooltip("Step Out (F8)")
                        .on_click(cx.listener(|this, _, _, cx| this.debugger.update(cx, |d, cx| d.step_out(cx)))),
                )
                .child(div().w(px(1.)).h(px(14.)).bg(theme.border))
            })
            .child(
                Button::new("tab-console")
                    .label("Console")
                    .xsmall()
                    .ghost()
                    .selected(tab == DebugTab::Console)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.debug_tab = DebugTab::Console;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("tab-log")
                    .label("Build Log")
                    .xsmall()
                    .ghost()
                    .selected(tab == DebugTab::BuildLog)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.debug_tab = DebugTab::BuildLog;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("tab-terminal")
                    .label("Terminal")
                    .xsmall()
                    .ghost()
                    .selected(tab == DebugTab::Terminal)
                    .tooltip("Activate Terminal (⌃⌘`)")
                    .on_click(cx.listener(|this, _, window, cx| this.activate_terminal(window, cx))),
            )
            .when(tab == DebugTab::Terminal, |el| el.child(div().w(px(1.)).h(px(14.)).bg(theme.border)).child(self.render_terminal_tabs(cx)))
            .child(div().flex_1())
            .when(tab == DebugTab::BuildLog, |el| {
                el.child(
                    Button::new("raw-log")
                        .label(if self.raw_log { "All Output" } else { "Summary" })
                        .xsmall()
                        .ghost()
                        .tooltip("Switch between the condensed log and xcodebuild's full output")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.raw_log = !this.raw_log;
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new("clear-console")
                    .icon(IconName::Trash)
                    .xsmall()
                    .ghost()
                    .tooltip("Clear Console (⌘K)")
                    .on_click(cx.listener(|this, _, _, cx| match this.debug_tab {
                        DebugTab::Terminal => {
                            if let Some(terminal) = this.terminals.panel.get(this.terminals.active) {
                                terminal.view.update(cx, |view, cx| view.clear(cx));
                            }
                        }
                        _ => this.build.update(cx, |b, cx| b.clear_console(cx)),
                    })),
            )
            .child(
                Button::new("hide-debug")
                    .icon(IconName::X)
                    .xsmall()
                    .ghost()
                    .tooltip("Hide Debug Area (⇧⌘Y)")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_debug_area = false;
                        cx.notify();
                    })),
            );

        let count = self.debug_line_count(cx);
        let prompt = self.render_console_prompt(cx);
        let body = if tab == DebugTab::Terminal {
            self.render_terminal_panel(cx)
        } else if count == 0 {
            div()
                .p_3()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(match tab {
                    DebugTab::Console => "Program output appears here when you run (⌘R).",
                    DebugTab::BuildLog | DebugTab::Terminal => "Build output appears here when you build (⌘B).",
                })
                .into_any_element()
        } else {
            uniform_list("debug-lines", count, cx.processor(Self::render_debug_lines))
                .track_scroll(&self.debug_scroll)
                .size_full()
                .px_2()
                .into_any_element()
        };
        v_flex()
            .size_full()
            .bg(theme.background)
            .border_t_1()
            .border_color(theme.border)
            .child(header)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(debugging && tab != DebugTab::Terminal, |el| el.child(self.render_variables(cx)))
                    .child(v_flex().flex_1().h_full().min_w_0().child(div().flex_1().min_h_0().child(body)).children(prompt)),
            )
    }

    /// Xcode's Variables view (left half of the debug area while debugging).
    fn render_variables(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let variables = self.debugger.read(cx).variables.clone();
        let mono = theme.mono_font_family.clone();
        let workspace = cx.weak_entity();
        v_flex()
            .id("variables")
            .w(px(380.))
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .overflow_y_scroll()
            .when(variables.is_empty(), |el| {
                el.child(
                    div()
                        .p_3()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Variables appear when execution pauses."),
                )
            })
            .children(variables.into_iter().enumerate().map(|(ix, var)| {
                h_flex()
                    .id(("var", ix))
                    .px_2()
                    .pl(px(8. + var.depth as f32 * 14.))
                    .h(px(20.))
                    .gap_1()
                    .text_xs()
                    .font_family(mono.clone())
                    .hover(|el| el.bg(theme.secondary_hover))
                    .child(
                        div().w(px(12.)).child(if var.reference != 0 {
                            Icon::new(if var.expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                                .xsmall()
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        }),
                    )
                    .child(div().text_color(theme.primary).child(var.name.clone()))
                    .when_some(var.ty.clone(), |el, ty| {
                        el.child(div().text_color(theme.muted_foreground).child(format!("({ty})")))
                    })
                    .child(div().child("="))
                    .child(div().flex_1().truncate().child(var.value.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.debugger.update(cx, |d, cx| d.toggle_variable(ix, cx));
                    }))
                    .context_menu({
                        let workspace = workspace.clone();
                        move |menu, _, _| Workspace::variable_menu(menu, ix, &var.name, var.value.clone(), workspace.clone())
                    })
            }))
    }

    pub(super) fn debug_line_count(&self, cx: &App) -> usize {
        let build = self.build.read(cx);
        match self.debug_tab {
            DebugTab::Console => build.console.len(),
            DebugTab::BuildLog if self.raw_log => build.log.len(),
            DebugTab::BuildLog => build.log.iter().filter(|l| l.pretty.is_some()).count(),
            DebugTab::Terminal => 0,
        }
    }

    fn render_debug_lines(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let build = self.build.read(cx);
        let mono = theme.mono_font_family.clone();
        let line = |text: String, color: Hsla, bold: bool| {
            div()
                .h(px(LOG_LINE_HEIGHT))
                .w_full()
                .text_xs()
                .font_family(mono.clone())
                .text_color(color)
                .when(bold, |el| el.font_weight(FontWeight::SEMIBOLD))
                .whitespace_nowrap()
                .truncate()
                .child(text)
                .into_any_element()
        };
        match self.debug_tab {
            DebugTab::Console => build.console[range.start.min(build.console.len())..range.end.min(build.console.len())]
                .iter()
                .map(|l| line(l.clone(), theme.foreground, false))
                .collect(),
            DebugTab::Terminal => vec![],
            DebugTab::BuildLog => {
                let lines: Vec<_> = if self.raw_log {
                    build.log.iter().skip(range.start).take(range.len()).collect()
                } else {
                    build
                        .log
                        .iter()
                        .filter(|l| l.pretty.is_some())
                        .skip(range.start)
                        .take(range.len())
                        .collect()
                };
                lines
                    .into_iter()
                    .map(|l| {
                        let (color, bold) = match l.style {
                            LineStyle::Error | LineStyle::Failure => (theme.danger, true),
                            LineStyle::Warning => (theme.warning, false),
                            LineStyle::Success => (theme.success, true),
                            LineStyle::Note => (theme.muted_foreground, false),
                            LineStyle::Step => (theme.foreground, false),
                            LineStyle::Plain => (theme.muted_foreground, false),
                        };
                        let text = if self.raw_log {
                            l.raw.clone()
                        } else {
                            l.pretty.clone().unwrap_or_default()
                        };
                        line(text, color, bold)
                    })
                    .collect()
            }
        }
    }

    // ------------------------------------------------------------------
    // Status bar

    fn render_status_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let lsp = self.lsp.read(cx);
        let (lsp_icon, lsp_color, lsp_text) = match lsp.status() {
            LspStatus::Idle => (IconName::Circle, theme.muted_foreground, "sourcekit-lsp: idle".to_string()),
            LspStatus::Starting => (IconName::Loader, theme.warning, "sourcekit-lsp: starting…".to_string()),
            LspStatus::Ready => (
                IconName::CircleCheck,
                theme.success,
                lsp.progress_message()
                    .map(|p| format!("sourcekit-lsp: {p}"))
                    .unwrap_or_else(|| "sourcekit-lsp".to_string()),
            ),
            LspStatus::Failed(err) => (IconName::CircleX, theme.danger, format!("sourcekit-lsp failed: {err}")),
            LspStatus::Exited => (IconName::CircleX, theme.danger, "sourcekit-lsp exited".to_string()),
        };
        // A property list's outline has no text cursor to show.
        let language = self.active_buffer().map(|b| b.language.clone()).unwrap_or_default();
        let cursor = self.active_editor().map(|editor| {
            let state = editor.read(cx);
            let pos = state.cursor_position();
            let selected = state.selected_range().len();
            (pos.line + 1, pos.character + 1, selected, language)
        });
        let vim = self.vim_status(window, cx);
        let prediction = self.prediction_indicator(cx);
        let perf = Perf::global(cx);
        h_flex()
            .h(px(24.))
            .px_2()
            .gap_3()
            .text_xs()
            .bg(theme.title_bar)
            .text_color(theme.muted_foreground)
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(lsp_icon).xsmall().text_color(lsp_color))
                    .child(div().truncate().max_w(px(360.)).child(lsp_text)),
            )
            .when_some(self.git.status.branch.clone(), |el, branch| {
                el.child(
                    h_flex()
                        .id("status-branch")
                        .gap_1()
                        .px_1()
                        .rounded(theme.radius)
                        .hover(|el| el.bg(theme.secondary_hover))
                        .child(Icon::new(IconName::GitBranch).xsmall())
                        .child(branch)
                        .on_click(cx.listener(|this, _, window, cx| this.show_branch_picker(false, window, cx))),
                )
            })
            .when_some(self.git.busy.clone(), |el, busy| el.child(format!("{busy}…")))
            .child(div().flex_1())
            // Vim mode: the command line being typed, else the last
            // message, else the mode (`-- INSERT --`); then pending keys.
            .when_some(vim, |el, vim| {
                let (text, color) = match (&vim.command_line, &vim.message) {
                    (Some(line), _) => (line.to_string(), theme.foreground),
                    (None, Some(message)) => (message.to_string(), if vim.error { theme.danger } else { theme.foreground }),
                    (None, None) => (format!("-- {} --", vim.mode.label()), theme.foreground),
                };
                el.child(
                    h_flex()
                        .id("vim-status")
                        .gap_2()
                        .font_family(theme.mono_font_family.clone())
                        .child(div().truncate().max_w(px(480.)).text_color(color).child(text))
                        .when(!vim.pending.is_empty(), |el| el.child(vim.pending.to_string())),
                )
            })
            // Predictive completion, while a provider is set: what it's
            // doing; a click opens its menu.
            .when_some(prediction, |el, (busy, label, problem)| {
                el.child(
                    h_flex()
                        .id("prediction-status")
                        .gap_1()
                        .px_1()
                        .rounded(theme.radius)
                        .hover(|el| el.bg(theme.secondary_hover))
                        .when(problem, |el| el.text_color(theme.danger))
                        .child(if busy {
                            Icon::new(IconName::Loader).xsmall().into_any_element()
                        } else {
                            Icon::new(gpui_kit::assets::IconName::Sparkles).xsmall().into_any_element()
                        })
                        .child(div().truncate().max_w(px(320.)).child(label))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.show_predictions_menu(event.position, window, cx)
                        })),
                )
            })
            .when_some(cursor, |el, (line, col, selected, language)| {
                el.child(if selected > 0 {
                    format!("Ln {line}, Col {col} ({selected} selected)")
                } else {
                    format!("Ln {line}, Col {col}")
                })
                .child(language.to_string())
                .child(if self.settings.current.hard_tabs {
                    format!("Tab Width: {}", self.settings.current.tab_size)
                } else {
                    format!("Spaces: {}", self.settings.current.tab_size)
                })
            })
            .child(
                h_flex()
                    .id("memory")
                    .gap_1()
                    .px_1()
                    .rounded(theme.radius)
                    .hover(|el| el.bg(theme.secondary_hover))
                    .when(perf.memory_warning(), |el| el.text_color(theme.warning))
                    .child(Icon::new(IconName::MemoryStick).xsmall())
                    .child(perf.status_label())
                    .on_click(cx.listener(|this, _, window, cx| this.show_performance(&ShowPerformance, window, cx))),
            )
    }

    // ------------------------------------------------------------------
    // Actions that open pickers

    pub(super) fn open_quickly(&mut self, _: &OpenQuickly, window: &mut Window, cx: &mut Context<Self>) {
        let server = self.lsp.read(cx).server();
        let delegate = pickers::OpenQuicklyDelegate::new(
            cx.weak_entity(),
            self.project.root.clone(),
            self.files.clone(),
            server,
        );
        let picker = cx.new(|cx| Picker::new(delegate, window, cx));
        self.show_modal(picker, window, cx);
    }

    pub(super) fn quick_actions(&mut self, _: &QuickActions, window: &mut Window, cx: &mut Context<Self>) {
        let delegate = pickers::QuickActionsDelegate::new(window);
        let picker = cx.new(|cx| Picker::new(delegate, window, cx));
        self.show_modal(picker, window, cx);
    }

    pub(super) fn show_document_items(&mut self, _: &ShowDocumentItems, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.active_item() else { return };
        let cursor_line = buffer.editor.read(cx).cursor_position().line;
        let path = buffer.path.clone();
        let workspace = cx.weak_entity();
        // Use the cached outline when it belongs to this file, refresh otherwise.
        let entries = if self.outline_path.as_ref() == Some(&path) {
            self.outline.clone()
        } else {
            vec![]
        };
        let generation = self.outline_generation;
        let picker = cx.new(|cx| {
            Picker::new(
                pickers::DocumentItemsDelegate::new(workspace, entries, cursor_line),
                window,
                cx,
            )
        });
        self.show_modal(picker.clone(), window, cx);
        self.refresh_outline(cx);
        // Fill the picker once fresh symbols arrive.
        let picker = picker.downgrade();
        let task = cx.spawn_in(window, async move |this, cx| {
            for _ in 0..40 {
                cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
                let ready = this
                    .update(cx, |this, _| {
                        (this.outline_generation != generation && this.outline_path.as_ref() == Some(&path))
                            .then(|| this.outline.clone())
                    })
                    .ok()
                    .flatten();
                if let Some(entries) = ready {
                    picker
                        .update_in(cx, |picker, window, cx| {
                            picker.delegate.set_entries(entries, cursor_line);
                            picker.refresh_query(window, cx);
                        })
                        .ok();
                    break;
                }
            }
        });
        self._tasks.push(task);
    }

    pub(super) fn jump_to_line(&mut self, _: &JumpToLine, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.active_item() else { return };
        let lines = zcode_editor::RopeExt::lines_len(buffer.editor.read(cx).text());
        let delegate = pickers::JumpToLineDelegate::new(cx.weak_entity(), lines);
        let picker = cx.new(|cx| Picker::new(delegate, window, cx));
        self.show_modal(picker, window, cx);
    }

    pub(super) fn choose_scheme(&mut self, _: &ChooseScheme, window: &mut Window, cx: &mut Context<Self>) {
        let delegate = pickers::SchemeDelegate::new(self.build.clone(), cx);
        let picker = cx.new(|cx| Picker::new(delegate, window, cx));
        self.show_modal(picker, window, cx);
    }

    pub(super) fn choose_destination(&mut self, _: &ChooseDestination, window: &mut Window, cx: &mut Context<Self>) {
        let delegate = pickers::DestinationDelegate::new(self.build.clone(), cx);
        let picker = cx.new(|cx| Picker::new(delegate, window, cx));
        self.show_modal(picker, window, cx);
    }

    pub(super) fn show_performance(&mut self, _: &ShowPerformance, window: &mut Window, cx: &mut Context<Self>) {
        let panel = cx.new(|cx| crate::perf_panel::PerfPanel::new(window, cx));
        self.show_modal(panel, window, cx);
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // Panels stay in the group when hidden (`visible(false)`) so their
        // sizes survive toggling.
        let navigator = if self.show_navigator {
            self.render_navigator(window, cx).into_any_element()
        } else {
            div().into_any_element()
        };
        let editor_area = self.render_editor_area(cx);
        let debug_area = if self.show_debug_area {
            self.render_debug_area(cx).into_any_element()
        } else {
            div().into_any_element()
        };

        let center = v_resizable("editor-debug")
            .child(resizable_panel().child(editor_area))
            .child(
                resizable_panel()
                    .size(px(240.))
                    .size_range(px(80.)..px(900.))
                    .visible(self.show_debug_area)
                    .child(debug_area),
            );
        let inspector = if self.show_inspector {
            self.render_inspector(cx).into_any_element()
        } else {
            div().into_any_element()
        };
        let agent_panel = match (&self.agent.panel, self.agent.shown) {
            (Some(panel), true) => panel.clone().into_any_element(),
            _ => div().into_any_element(),
        };
        let main = h_resizable("navigator-editor")
            .child(
                resizable_panel()
                    .size(px(270.))
                    .size_range(px(180.)..px(560.))
                    .visible(self.show_navigator)
                    .child(navigator),
            )
            .child(resizable_panel().child(center))
            .child(
                resizable_panel()
                    .size(px(260.))
                    .size_range(px(200.)..px(480.))
                    .visible(self.show_inspector)
                    .child(inspector),
            )
            .child(
                resizable_panel()
                    .size(px(380.))
                    .size_range(px(280.)..px(900.))
                    .visible(self.agent.shown && self.agent.panel.is_some())
                    .child(agent_panel),
            );

        v_flex()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &Save, window, cx| {
                // Project settings are written as they are edited; a
                // multibuffer saves the files it changed.
                if let Some(view) = this.active_multibuffer().cloned() {
                    this.save_multibuffer(&view, window, cx);
                } else if let Some(path) = this.active_file_path() {
                    this.save_path_from_user(&path, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SaveAll, window, cx| this.save_all_from_user(window, cx)))
            // Xcode's Close Tab, Editor Pane, Window Tab, or Window.
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                let pane = this.active_pane;
                if this.modal.is_some() {
                    this.dismiss_modal(window, cx);
                } else if let Some(ix) = this.focused_panel_terminal(window, cx) {
                    // ⌘W in the debug area's terminal closes that terminal.
                    let view = this.terminals.panel[ix].view.entity_id();
                    this.close_terminal(view, window, cx);
                } else if let Some(ix) = this.pane(pane).filter(|p| !p.items.is_empty()).map(|p| p.active) {
                    this.close_item(pane, ix, window, cx);
                } else if this.panes.len() > 1 {
                    this.close_pane(pane, window, cx);
                } else {
                    window.remove_window();
                }
            }))
            .on_action(cx.listener(|this, _: &CloseOtherTabs, window, cx| this.close_other_tabs(window, cx)))
            .on_action(cx.listener(|this, _: &NewEditorPaneOnRight, window, cx| {
                this.split_pane(SplitAxis::Horizontal, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewEditorPaneBelow, window, cx| this.split_pane(SplitAxis::Vertical, window, cx)))
            .on_action(cx.listener(|this, _: &CloseEditorPane, window, cx| this.close_pane(this.active_pane, window, cx)))
            .on_action(cx.listener(|this, _: &CloseOtherEditorPanes, window, cx| this.close_other_panes(window, cx)))
            .on_action(cx.listener(|this, _: &FocusNextEditorPane, window, cx| this.focus_next_pane(true, window, cx)))
            .on_action(cx.listener(|this, _: &FocusPreviousEditorPane, window, cx| this.focus_next_pane(false, window, cx)))
            .on_action(cx.listener(|this, _: &FocusEditorPane, _, cx| this.toggle_zoom(cx)))
            .on_action(cx.listener(|this, _: &ShowAssistant, window, cx| this.toggle_assistant(window, cx)))
            .on_action(cx.listener(|this, _: &ShowEditorOnly, window, cx| this.show_editor_only(window, cx)))
            .on_action(cx.listener(|this, _: &JumpToDefinitionInNextPane, window, cx| {
                this.jump_to_definition_in_next_pane(window, cx)
            }))
            .on_action(cx.listener(|this, _: &JumpToCounterpartInNextPane, window, cx| {
                this.jump_to_counterpart_in_next_pane(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseProject, window, _| {
                let _ = this;
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &RevealInFinder, _, cx| {
                let path = this
                    .active_buffer()
                    .map(|b| b.path.clone())
                    .unwrap_or_else(|| this.project.root.clone());
                cx.reveal_path(&path);
            }))
            .on_action(cx.listener(Self::new_file))
            .on_action(cx.listener(|this, _: &OpenAsSourceCode, window, cx| {
                if let Some(ix) = this.active_item().and(this.pane(this.active_pane)).map(|p| p.active) {
                    this.open_as(this.active_pane, ix, true, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &OpenAsPropertyList, window, cx| {
                if let Some(ix) = this.active_item().and(this.pane(this.active_pane)).map(|p| p.active) {
                    this.open_as(this.active_pane, ix, false, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowProjectSettings, window, cx| this.open_project_settings(window, cx)))
            .on_action(cx.listener(Self::open_quickly))
            .on_action(cx.listener(Self::quick_actions))
            .on_action(cx.listener(Self::show_document_items))
            .on_action(cx.listener(Self::jump_to_line))
            .on_action(cx.listener(Self::choose_scheme))
            .on_action(cx.listener(|this, _: &NextScheme, _, cx| this.build.update(cx, |b, cx| b.step_scheme(1, cx))))
            .on_action(cx.listener(|this, _: &PreviousScheme, _, cx| this.build.update(cx, |b, cx| b.step_scheme(-1, cx))))
            .on_action(cx.listener(|this, _: &EditScheme, window, cx| {
                this.edit_scheme(SchemeAction::Run, None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &RunWithOptions, window, cx| {
                this.edit_scheme(SchemeAction::Run, Some(SchemeAction::Run), window, cx)
            }))
            .on_action(cx.listener(|this, _: &TestWithOptions, window, cx| {
                this.edit_scheme(SchemeAction::Test, Some(SchemeAction::Test), window, cx)
            }))
            .on_action(cx.listener(Self::choose_destination))
            .on_action(cx.listener(Self::show_performance))
            .on_action(cx.listener(Self::cut_or_cut_line))
            .on_action(cx.listener(Self::copy_or_copy_line))
            .on_action(cx.listener(Self::paste_or_paste_line))
            .on_action(cx.listener(Self::toggle_comment))
            .on_action(cx.listener(Self::duplicate_line))
            .on_action(cx.listener(Self::move_line_up))
            .on_action(cx.listener(Self::move_line_down))
            .on_action(cx.listener(Self::select_next_occurrence))
            .on_action(cx.listener(Self::next_placeholder))
            .on_action(cx.listener(Self::previous_placeholder))
            .on_action(cx.listener(Self::tab_or_placeholder))
            .on_action(cx.listener(Self::show_predictive_completion))
            .on_action(cx.listener(Self::accept_edit_prediction))
            .on_action(cx.listener(Self::dismiss_edit_prediction))
            .on_action(cx.listener(Self::toggle_soft_wrap))
            .on_action(cx.listener(Self::jump_to_definition))
            .on_action(cx.listener(Self::fix_issue))
            .on_action(cx.listener(Self::jump_to_counterpart))
            .on_action(cx.listener(Self::find_selected_symbol))
            .on_action(cx.listener(Self::edit_all_in_scope))
            .on_action(cx.listener(Self::format_file))
            .on_action(cx.listener(Self::re_indent))
            .on_action(cx.listener(Self::rename))
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::go_forward))
            .on_action(cx.listener(|this, _: &FindInFile, window, cx| {
                window.dispatch_action(Box::new(zcode_editor::Search), cx);
                let _ = this;
            }))
            .on_action(cx.listener(|this, _: &FindAndReplaceInFile, window, cx| {
                window.dispatch_action(Box::new(zcode_editor::Replace), cx);
                let _ = this;
            }))
            .on_action(cx.listener(|this, _: &FindInWorkspace, window, cx| {
                this.find.replacing = false;
                this.show_navigator_tab(NavigatorTab::Find, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FindAndReplaceInWorkspace, window, cx| this.show_find_and_replace(window, cx)))
            .on_action(cx.listener(|this, _: &ShowFindResultsInEditor, window, cx| {
                this.show_find_results_in_editor(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowIssuesInEditor, window, cx| {
                this.show_issues_in_editor(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenExcerpts, _, cx| this.open_excerpts(cx)))
            .on_action(cx.listener(|this, _: &ExpandExcerpts, window, cx| this.expand_excerpts(window, cx)))
            .on_action(cx.listener(|this, _: &NextIssue, window, cx| this.step_issue(1, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousIssue, window, cx| this.step_issue(-1, window, cx)))
            .on_action(cx.listener(|this, _: &RevealInNavigator, window, cx| {
                this.show_navigator_tab(NavigatorTab::Project, window, cx);
                if let Some(path) = this.active_item().map(|item| item.path.to_string_lossy().to_string()) {
                    let id: SharedString = path.into();
                    this.tree.update(cx, |tree, cx| {
                        tree.reveal_item(&id, ScrollStrategy::Center, cx);
                        if let Some(ix) = tree.index_of(&id) {
                            tree.set_selected_index(Some(ix), cx);
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.step_tab(true, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| this.step_tab(false, window, cx)))
            .on_action(cx.listener(|this, _: &ShowProjectNavigator, window, cx| {
                this.show_navigator_tab(NavigatorTab::Project, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowSourceControlNavigator, window, cx| {
                this.show_navigator_tab(NavigatorTab::SourceControl, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowBookmarksNavigator, window, cx| {
                this.show_navigator_tab(NavigatorTab::Bookmarks, window, cx)
            }))
            .on_action(cx.listener(|this, _: &BookmarkLine, window, cx| this.bookmark_line(false, window, cx)))
            .on_action(cx.listener(|this, _: &BookmarkLineWithDialog, window, cx| this.bookmark_line(true, window, cx)))
            .on_action(cx.listener(|this, _: &BookmarkFile, window, cx| this.bookmark_file(false, window, cx)))
            .on_action(cx.listener(|this, _: &BookmarkFileWithDialog, window, cx| this.bookmark_file(true, window, cx)))
            .on_action(cx.listener(|this, _: &NextBookmark, window, cx| this.step_bookmark(true, false, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousBookmark, window, cx| this.step_bookmark(false, false, window, cx)))
            .on_action(cx.listener(|this, _: &NextBookmarkInWorkspace, window, cx| this.step_bookmark(true, true, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousBookmarkInWorkspace, window, cx| this.step_bookmark(false, true, window, cx)))
            .on_action(cx.listener(|this, _: &ShowFindNavigator, window, cx| {
                this.show_navigator_tab(NavigatorTab::Find, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowIssueNavigator, window, cx| {
                this.show_navigator_tab(NavigatorTab::Issues, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowTestNavigator, window, cx| {
                this.show_navigator_tab(NavigatorTab::Tests, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowReportNavigator, window, cx| {
                this.show_navigator_tab(NavigatorTab::Reports, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowCodingAssistant, window, cx| this.toggle_agent_panel(window, cx)))
            .on_action(cx.listener(|this, _: &NewConversation, window, cx| this.new_conversation(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleNavigator, _, cx| {
                this.show_navigator = !this.show_navigator;
                cx.notify();
            }))
            .on_action(cx.listener(Self::add_files))
            .on_action(cx.listener(Self::add_package_dependencies))
            .on_action(cx.listener(Self::resolve_package_versions))
            .on_action(cx.listener(Self::update_to_latest_package_versions))
            .on_action(cx.listener(Self::reset_package_caches))
            .on_action(cx.listener(|this, _: &ShowFileInspector, _, cx| this.show_file_inspector(None, cx)))
            .on_action(cx.listener(|this, _: &ToggleInspector, _, cx| this.toggle_inspector(cx)))
            .on_action(cx.listener(|this, _: &ToggleDebugArea, _, cx| {
                this.show_debug_area = !this.show_debug_area;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ActivateConsole, window, cx| this.activate_console(window, cx)))
            .on_action(cx.listener(|this, _: &ActivateTerminal, window, cx| this.activate_terminal(window, cx)))
            .on_action(cx.listener(|this, _: &NewTerminal, window, cx| {
                let dir = this.project.root.clone();
                this.new_panel_terminal(crate::terminal::Spawn::shell(dir), window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewTerminalTab, window, cx| {
                let dir = this.project.root.clone();
                this.new_terminal_tab(crate::terminal::Spawn::shell(dir), window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenTerminalAtLocation, window, cx| this.open_terminal_at(None, window, cx)))
            .on_action(cx.listener(|this, _: &IncreaseFontSize, window, cx| this.change_font_size(1., window, cx)))
            .on_action(cx.listener(|this, _: &DecreaseFontSize, window, cx| this.change_font_size(-1., window, cx)))
            .on_action(cx.listener(|this, _: &ResetFontSize, window, cx| this.change_font_size(0., window, cx)))
            .on_action(cx.listener(|this, _: &Build, window, cx| this.start_build(BuildAction::Build, window, cx)))
            .on_action(cx.listener(|this, _: &Run, window, cx| this.start_build(BuildAction::Run, window, cx)))
            .on_action(cx.listener(|this, _: &Test, window, cx| this.test_all(window, cx)))
            .on_action(cx.listener(Self::test_selected_items))
            .on_action(cx.listener(Self::test_again))
            .on_action(cx.listener(Self::rerun_failed_tests))
            .on_action(cx.listener(Self::choose_test_plan))
            .on_action(cx.listener(|this, _: &ClearTestResults, _, cx| this.build.update(cx, |b, cx| b.clear_test_results(cx))))
            .on_action(cx.listener(|this, _: &Clean, window, cx| this.start_build(BuildAction::Clean, window, cx)))
            .on_action(cx.listener(|this, _: &Stop, _, cx| this.stop(cx)))
            .on_action(cx.listener(Self::toggle_breakpoint))
            .on_action(cx.listener(|this, _: &ToggleBreakpointsActive, _, cx| {
                this.debugger.update(cx, |d, cx| d.toggle_breakpoints_active(cx));
            }))
            .on_action(cx.listener(|this, _: &EditBreakpoint, window, cx| this.edit_breakpoint_at_cursor(window, cx)))
            .on_action(cx.listener(|this, _: &CreateSymbolicBreakpoint, window, cx| this.create_symbolic_breakpoint(window, cx)))
            .on_action(cx.listener(|this, _: &CreateSwiftErrorBreakpoint, window, cx| {
                this.create_swift_error_breakpoint(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CreateExceptionBreakpoint, window, cx| this.create_exception_breakpoint(window, cx)))
            .on_action(cx.listener(|this, _: &DeleteAllBreakpoints, _, cx| {
                this.debugger.update(cx, |d, cx| d.remove_all_breakpoints(cx));
            }))
            .on_action(cx.listener(|this, _: &DebugContinue, _, cx| {
                this.debugger.update(cx, |d, cx| d.continue_or_pause(cx));
            }))
            .on_action(cx.listener(|this, _: &StepOver, _, cx| this.debugger.update(cx, |d, cx| d.step_over(cx))))
            .on_action(cx.listener(|this, _: &StepInto, _, cx| this.debugger.update(cx, |d, cx| d.step_into(cx))))
            .on_action(cx.listener(|this, _: &StepOut, _, cx| this.debugger.update(cx, |d, cx| d.step_out(cx))))
            .on_action(cx.listener(|this, _: &ShowDebugNavigator, window, cx| {
                this.show_navigator_tab(NavigatorTab::Debug, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowBreakpointNavigator, window, cx| {
                this.show_navigator_tab(NavigatorTab::Breakpoints, window, cx)
            }))
            .on_action(cx.listener(|this, _: &RefreshDestinations, _, cx| this.build.update(cx, |b, cx| b.reload(cx))))
            .on_action(cx.listener(|this, _: &NextChange, window, cx| this.jump_to_change_anywhere(true, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousChange, window, cx| this.jump_to_change_anywhere(false, window, cx)))
            .on_action(cx.listener(|this, _: &CodeReview, window, cx| this.toggle_code_review(window, cx)))
            // The Editor menu's toggles write the settings, which apply them.
            .on_action(cx.listener(|this, _: &SideBySideComparison, window, cx| {
                this.set_setting(&["git", "side_by_side_comparison"], serde_json::json!(true), window, cx)
            }))
            .on_action(cx.listener(|this, _: &InlineComparison, window, cx| {
                this.set_setting(&["git", "side_by_side_comparison"], serde_json::json!(false), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowAuthors, window, cx| {
                this.toggle_setting(&["git", "show_authors"], |s| s.show_authors, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleInlineBlame, window, cx| {
                this.toggle_setting(&["git", "inline_blame", "enabled"], |s| s.inline_blame, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleInlayHints, window, cx| {
                this.toggle_setting(&["inlay_hints", "enabled"], |s| s.inlay_hints, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleVimMode, window, cx| this.toggle_vim_mode(window, cx)))
            .on_action(cx.listener(Self::toggle_invisibles))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::open_project_settings_file))
            .on_action(cx.listener(Self::open_default_settings))
            .on_action(cx.listener(Self::open_default_keymap))
            .on_action(cx.listener(Self::select_theme))
            .on_action(cx.listener(Self::import_xcode_theme))
            .on_action(cx.listener(Self::import_xcode_key_bindings))
            .on_action(cx.listener(|this, _: &ShowCallers, window, cx| this.show_callers(window, cx)))
            .on_action(cx.listener(|this, _: &FindCallHierarchy, window, cx| this.find_hierarchy(HierarchyKind::Callers, window, cx)))
            .on_action(cx.listener(|this, _: &FindAncestorTypes, window, cx| this.find_hierarchy(HierarchyKind::Supertypes, window, cx)))
            .on_action(cx.listener(|this, _: &FindDescendentTypes, window, cx| this.find_hierarchy(HierarchyKind::Subtypes, window, cx)))
            .on_action(cx.listener(|this, _: &FindConformingTypes, window, cx| this.find_hierarchy(HierarchyKind::Subtypes, window, cx)))
            .on_action(cx.listener(|this, _: &Commit, window, cx| this.show_commit_sheet(window, cx)))
            .on_action(cx.listener(|this, _: &Push, window, cx| this.show_remote_sheet(RemoteAction::Push, window, cx)))
            .on_action(cx.listener(|this, _: &Pull, window, cx| this.show_remote_sheet(RemoteAction::Pull, window, cx)))
            .on_action(cx.listener(|this, _: &FetchChanges, window, cx| this.fetch_changes(window, cx)))
            .on_action(cx.listener(|this, _: &RefreshFileStatus, _, cx| this.refresh_scm(cx)))
            .on_action(cx.listener(|this, _: &StageAllChanges, window, cx| this.stage_all(true, window, cx)))
            .on_action(cx.listener(|this, _: &UnstageAllChanges, window, cx| this.stage_all(false, window, cx)))
            .on_action(cx.listener(|this, _: &DiscardAllChanges, window, cx| this.discard_all(window, cx)))
            .on_action(cx.listener(|this, _: &SwitchBranch, window, cx| this.show_branch_picker(false, window, cx)))
            .on_action(cx.listener(|this, _: &NewBranch, window, cx| this.show_branch_picker(true, window, cx)))
            .on_action(cx.listener(|this, _: &ClearConsole, _, cx| this.build.update(cx, |b, cx| b.clear_console(cx))))
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h_0().child(main))
            .child(self.render_status_bar(window, cx))
            .when_some(self.modal.as_ref().map(|m| (m.view.clone(), m.position)), |el, (view, position)| {
                let content = div().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(view);
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .pt(px(72.))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.dismiss_modal(window, cx)),
                        )
                        .map(|el| match position {
                            // A popover beside what it edits, kept in the window.
                            Some(position) => el.child(
                                anchored().position(position + point(px(0.), px(2.))).snap_to_window_with_margin(px(8.)).child(content),
                            ),
                            None => el.child(content),
                        }),
                )
            })
            .when_some(self.popup_menu.as_ref().map(|p| (p.menu.clone(), p.position)), |el, (menu, position)| {
                el.child(
                    deferred(anchored().position(position).snap_to_window_with_margin(px(8.)).child(menu)).with_priority(1),
                )
            })
    }
}

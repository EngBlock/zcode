//! The navigator area (left): Project, Source Control, Symbols, Find,
//! Issues, Tests and Reports — Xcode's ⌘1…⌘9.

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Selectable as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::tree::tree;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::assets::IconName;

use super::pickers::{file_icon, symbol_icon};
use super::{Target, Workspace};
use crate::build::log::{IssueSource, Severity, TestStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigatorTab {
    Project,
    SourceControl,
    Symbols,
    Find,
    Issues,
    Tests,
    Debug,
    Breakpoints,
    Reports,
}

impl NavigatorTab {
    const ALL: [NavigatorTab; 9] = [
        Self::Project,
        Self::SourceControl,
        Self::Symbols,
        Self::Find,
        Self::Issues,
        Self::Tests,
        Self::Debug,
        Self::Breakpoints,
        Self::Reports,
    ];

    fn icon(&self) -> IconName {
        match self {
            Self::Project => IconName::Folder,
            Self::SourceControl => IconName::GitBranch,
            Self::Symbols => IconName::Shapes,
            Self::Find => IconName::Search,
            Self::Issues => IconName::TriangleAlert,
            Self::Tests => IconName::Diamond,
            Self::Debug => IconName::Bug,
            Self::Breakpoints => IconName::Tag,
            Self::Reports => IconName::MessageSquareText,
        }
    }

    fn title(&self) -> &'static str {
        match self {
            Self::Project => "Project Navigator (⌘1)",
            Self::SourceControl => "Source Control Navigator (⌘2)",
            Self::Symbols => "Symbol Navigator (⌘3)",
            Self::Find => "Find Navigator (⌘4)",
            Self::Issues => "Issue Navigator (⌘5)",
            Self::Tests => "Test Navigator (⌘6)",
            Self::Debug => "Debug Navigator (⌘7)",
            Self::Breakpoints => "Breakpoint Navigator (⌘8)",
            Self::Reports => "Report Navigator (⌘9)",
        }
    }
}

fn row(id: impl Into<ElementId>) -> Stateful<Div> {
    h_flex().id(id).w_full().px_2().py(px(3.)).gap_1p5().text_sm()
}

fn section_header(label: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px_2()
        .pt_2()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(label.into())
}

fn empty(message: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .p_4()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(message.into())
}

impl Workspace {
    pub(super) fn show_navigator_tab(&mut self, tab: NavigatorTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_navigator && self.navigator == tab && tab != NavigatorTab::Find {
            // Pressing the shortcut again keeps it open, like Xcode.
        }
        self.navigator = tab;
        self.show_navigator = true;
        match tab {
            NavigatorTab::Symbols => self.refresh_outline(cx),
            NavigatorTab::SourceControl => self.refresh_scm(cx),
            NavigatorTab::Find => {
                let selected = self
                    .active_editor()
                    .map(|e| e.read(cx).selected_text().to_string())
                    .filter(|s| !s.is_empty() && !s.contains('\n'));
                self.find.input.update(cx, |input, cx| {
                    if let Some(selected) = selected {
                        input.set_value(selected, window, cx);
                    }
                    input.focus(window, cx);
                    // Typing replaces the pre-filled query, as in Xcode.
                    input.select_all(window, cx);
                });
            }
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn render_navigator(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tabs = h_flex()
            .h(px(30.))
            .px_1()
            .gap_0p5()
            .justify_center()
            .border_b_1()
            .border_color(theme.border)
            .children(NavigatorTab::ALL.iter().map(|&tab| {
                let selected = self.navigator == tab;
                let issues = if tab == NavigatorTab::Issues {
                    self.all_issues(cx)
                        .iter()
                        .filter(|i| i.severity == Severity::Error)
                        .count()
                } else {
                    0
                };
                Button::new(SharedString::from(format!("nav-{tab:?}")))
                    .icon(Icon::new(tab.icon()).when(issues > 0, |i| i.text_color(theme.danger)))
                    .ghost()
                    .xsmall()
                    .selected(selected)
                    .tooltip(tab.title())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.show_navigator_tab(tab, window, cx);
                    }))
            }));

        let body = match self.navigator {
            NavigatorTab::Project => self.render_project_navigator(cx).into_any_element(),
            NavigatorTab::SourceControl => self.render_scm_navigator(cx).into_any_element(),
            NavigatorTab::Symbols => self.render_symbol_navigator(cx).into_any_element(),
            NavigatorTab::Find => self.render_find_navigator(window, cx).into_any_element(),
            NavigatorTab::Issues => self.render_issue_navigator(cx).into_any_element(),
            NavigatorTab::Tests => self.render_test_navigator(cx).into_any_element(),
            NavigatorTab::Debug => self.render_debug_navigator(cx).into_any_element(),
            NavigatorTab::Breakpoints => self.render_breakpoint_navigator(cx).into_any_element(),
            NavigatorTab::Reports => self.render_report_navigator(cx).into_any_element(),
        };

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(tabs)
            .child(div().flex_1().min_h_0().child(body))
    }

    fn render_project_navigator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let workspace = cx.weak_entity();
        let dirty: Vec<PathBuf> = self
            .buffers
            .iter()
            .filter(|b| b.dirty)
            .map(|b| b.path.clone())
            .collect();
        let error_files: Vec<PathBuf> = self
            .all_issues(cx)
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .filter_map(|i| i.path)
            .collect();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Icon::new(IconName::FolderCode).small().text_color(theme.primary))
                    .child(self.project.name.clone())
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(theme.muted_foreground)
                            .child(self.project.kind.label()),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    tree(&self.tree, move |ix, entry, selected, _window, cx| {
                        let item = entry.item();
                        let path = PathBuf::from(item.id.as_str());
                        let is_folder = entry.is_folder();
                        let icon = if is_folder {
                            if entry.is_expanded() {
                                IconName::FolderOpen
                            } else {
                                IconName::Folder
                            }
                        } else {
                            file_icon(&path)
                        };
                        let theme = cx.theme();
                        let has_error = !is_folder && error_files.contains(&path);
                        let is_dirty = !is_folder && dirty.contains(&path);
                        let workspace = workspace.clone();
                        ListItem::new(ix)
                            .w_full()
                            .rounded(theme.radius)
                            .py(px(2.))
                            .px_2()
                            .pl(px(12.) * entry.depth() + px(8.))
                            .selected(selected)
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .text_sm()
                                    .child(Icon::new(icon).small().text_color(if is_folder {
                                        theme.primary
                                    } else {
                                        theme.muted_foreground
                                    }))
                                    .child(div().flex_1().truncate().child(item.label.clone()))
                                    .when(is_dirty, |el| {
                                        el.child(div().size(px(6.)).rounded_full().bg(theme.muted_foreground))
                                    })
                                    .when(has_error, |el| {
                                        el.child(Icon::new(IconName::CircleX).xsmall().text_color(theme.danger))
                                    }),
                            )
                            .on_click(move |_, window, cx| {
                                if is_folder {
                                    return;
                                }
                                let path = path.clone();
                                workspace
                                    .update(cx, |this, cx| this.open_file(path, None, true, window, cx))
                                    .ok();
                            })
                    })
                    .context_menu({
                        let workspace = cx.weak_entity();
                        move |_, entry, menu, _, _| {
                            let path = PathBuf::from(entry.item().id.as_str());
                            let folder = if entry.is_folder() {
                                path.clone()
                            } else {
                                path.parent().map(PathBuf::from).unwrap_or_else(|| path.clone())
                            };
                            let (ws1, ws2, ws3) = (workspace.clone(), workspace.clone(), workspace.clone());
                            let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
                            menu.item(PopupMenuItem::new("New File…").on_click(move |_, window, cx| {
                                let folder = folder.clone();
                                ws1.update(cx, |ws, cx| ws.new_file_in(folder, window, cx)).ok();
                            }))
                            .separator()
                            .item(PopupMenuItem::new("Rename…").on_click(move |_, window, cx| {
                                let path = p1.clone();
                                ws2.update(cx, |ws, cx| ws.prompt_rename(path, window, cx)).ok();
                            }))
                            .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| {
                                cx.reveal_path(&p2);
                            }))
                            .separator()
                            .item(PopupMenuItem::new("Move to Trash").on_click(move |_, window, cx| {
                                let path = p3.clone();
                                ws3.update(cx, |ws, cx| ws.move_to_trash(path, window, cx)).ok();
                            }))
                        }
                    })
                    .text_sm()
                    .p_1(),
                ),
            )
    }

    fn render_scm_navigator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut list = v_flex()
            .id("scm")
            .size_full()
            .overflow_y_scroll()
            .child(
                h_flex()
                    .justify_between()
                    .child(section_header(
                        match &self.scm_branch {
                            Some(branch) => format!("Changes — {branch}"),
                            None => "Changes".to_string(),
                        },
                        cx,
                    ))
                    .child(
                        Button::new("scm-refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .xsmall()
                            .tooltip("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_scm(cx))),
                    ),
            );
        if self.scm_branch.is_none() && self.scm_status.is_empty() {
            return list.child(empty("Not a Git repository.", cx));
        }
        if self.scm_status.is_empty() {
            return list.child(empty("No local changes.", cx));
        }
        for (ix, (status, path)) in self.scm_status.iter().enumerate() {
            let color = match status.as_str() {
                "M" | "R" => theme.warning,
                "A" | "??" => theme.success,
                "D" => theme.danger,
                _ => theme.muted_foreground,
            };
            let label = if status == "??" { "A?" } else { status.as_str() };
            let open = path.clone();
            list = list.child(
                row(("scm-row", ix))
                    .hover(|el| el.bg(theme.sidebar_accent))
                    .child(Icon::new(file_icon(path)).small().text_color(theme.muted_foreground))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .child(path.file_name().unwrap_or_default().to_string_lossy().to_string()),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).truncate().max_w(px(110.)).child(
                        self.relative(path.parent().unwrap_or(path)),
                    ))
                    .child(div().w(px(18.)).text_xs().font_weight(FontWeight::BOLD).text_color(color).child(label.to_string()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if open.is_file() {
                            this.open_file(open.clone(), None, true, window, cx);
                        }
                    })),
            );
        }
        list
    }

    fn render_symbol_navigator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let title = self
            .active_buffer()
            .map(|b| b.title())
            .unwrap_or_else(|| "No file".into());
        let mut list = v_flex()
            .id("symbols")
            .size_full()
            .overflow_y_scroll()
            .child(section_header(title, cx));
        if self.outline.is_empty() {
            let message = match self.lsp.read(cx).status() {
                crate::lsp::LspStatus::Ready => "No symbols in this file.",
                _ => "Symbols appear once the language server is ready.",
            };
            return list.child(empty(message, cx));
        }
        for (ix, entry) in self.outline.clone().into_iter().enumerate() {
            list = list.child(
                row(("symbol", ix))
                    .pl(px(8. + entry.depth as f32 * 14.))
                    .hover(|el| el.bg(theme.sidebar_accent))
                    .child(Icon::new(symbol_icon(entry.kind)).small().text_color(theme.primary))
                    .child(div().flex_1().truncate().child(entry.name.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.reveal_outline_entry(&entry, window, cx);
                    })),
            );
        }
        list
    }

    fn render_find_navigator(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let options = self.find.options.clone();
        let toggle = |id: &'static str, label: &'static str, on: bool, tooltip: &'static str| {
            Button::new(id)
                .label(label)
                .xsmall()
                .ghost()
                .selected(on)
                .tooltip(tooltip)
        };
        let results = self.find.results.clone();
        let total: usize = results.iter().map(|f| f.matches.len()).sum();
        let summary = if self.find.searching {
            "Searching…".to_string()
        } else if let Some(err) = &self.find.error {
            format!("Invalid pattern: {err}")
        } else if results.is_empty() && self.find.input.read(cx).value().is_empty() && self.find.title.is_none() {
            String::new()
        } else {
            let capped = if total >= crate::search::MAX_MATCHES { "+" } else { "" };
            format!(
                "{}{total}{capped} results in {} files",
                self.find.title.as_ref().map(|t| format!("{t}: ")).unwrap_or_default(),
                results.len()
            )
        };

        let mut list = v_flex().id("find-results").flex_1().min_h_0().overflow_y_scroll();
        for (file_ix, file) in results.iter().enumerate() {
            list = list.child(
                row(("find-file", file_ix))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Icon::new(file_icon(&file.path)).small().text_color(theme.muted_foreground))
                    .child(div().truncate().child(file.path.file_name().unwrap_or_default().to_string_lossy().to_string()))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(self.relative(file.path.parent().unwrap_or(&file.path))),
                    ),
            );
            for (match_ix, m) in file.matches.iter().enumerate().take(100) {
                let path = file.path.clone();
                let line = m.line;
                let column = m.column;
                let highlight = HighlightStyle {
                    background_color: Some(theme.warning.opacity(0.35)),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..Default::default()
                };
                let text = StyledText::new(m.preview.clone()).with_highlights(vec![(m.range.clone(), highlight)]);
                list = list.child(
                    row(SharedString::from(format!("find-{file_ix}-{match_ix}")))
                        .pl_6()
                        .text_xs()
                        .hover(|el| el.bg(theme.sidebar_accent))
                        .child(div().text_color(theme.muted_foreground).w(px(28.)).child(format!("{}", line + 1)))
                        .child(div().flex_1().truncate().font_family(theme.mono_font_family.clone()).child(text))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_file(path.clone(), Some(Target::Line { line, column }), true, window, cx);
                        })),
                );
            }
        }

        v_flex()
            .size_full()
            .child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .child(Input::new(&self.find.input).small().cleanable(true))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(toggle("find-case", "Aa", options.case_sensitive, "Match Case").on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.find.options.case_sensitive = !this.find.options.case_sensitive;
                                    this.run_find(window, cx);
                                },
                            )))
                            .child(toggle("find-word", "W", options.whole_word, "Whole Words").on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.find.options.whole_word = !this.find.options.whole_word;
                                    this.run_find(window, cx);
                                },
                            )))
                            .child(toggle("find-regex", ".*", options.regex, "Regular Expression").on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.find.options.regex = !this.find.options.regex;
                                    this.run_find(window, cx);
                                },
                            )))
                            .child(div().flex_1())
                            .child(
                                Button::new("find-go")
                                    .label("Find")
                                    .xsmall()
                                    .primary()
                                    .on_click(cx.listener(|this, _, window, cx| this.run_find(window, cx))),
                            ),
                    )
                    .when(!summary.is_empty(), |el| {
                        el.child(div().text_xs().text_color(theme.muted_foreground).child(summary))
                    }),
            )
            .child(div().h(px(1.)).bg(theme.border))
            .child(list)
    }

    fn render_issue_navigator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let issues = self.all_issues(cx);
        let mut list = v_flex().id("issues").size_full().overflow_y_scroll();
        if issues.is_empty() {
            return list.child(empty("No issues", cx));
        }
        let mut current_file: Option<Option<PathBuf>> = None;
        for (ix, issue) in issues.into_iter().enumerate() {
            if current_file.as_ref() != Some(&issue.path) {
                current_file = Some(issue.path.clone());
                let label = issue
                    .path
                    .as_ref()
                    .map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_string())
                    .unwrap_or_else(|| "Build".into());
                list = list.child(section_header(label, cx));
            }
            let (icon, color) = match issue.severity {
                Severity::Error => (IconName::CircleX, theme.danger),
                Severity::Warning => (IconName::TriangleAlert, theme.warning),
                Severity::Note => (IconName::Info, theme.muted_foreground),
            };
            let location = issue.line.map(|l| format!(":{l}")).unwrap_or_default();
            let live = issue.source == IssueSource::Live;
            let open = issue.clone();
            list = list.child(
                h_flex()
                    .id(("issue", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_1p5()
                    .items_start()
                    .text_xs()
                    .hover(|el| el.bg(theme.sidebar_accent))
                    .child(Icon::new(icon).small().text_color(color))
                    .child(
                        v_flex()
                            .flex_1()
                            .child(div().child(issue.message.clone()))
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{}{location}", if live { "Live" } else { "Build" })),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.open_issue(&open, window, cx))),
            );
        }
        list
    }

    fn render_test_navigator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let build = self.build.read(cx);
        let tests = build.tests.clone();
        let failed = build.failed_tests();
        let busy = build.phase.is_busy();
        let mut list = v_flex()
            .id("tests")
            .size_full()
            .overflow_y_scroll()
            .child(
                h_flex()
                    .justify_between()
                    .child(section_header(
                        if tests.is_empty() {
                            "Tests".to_string()
                        } else {
                            format!("{} tests, {failed} failed", tests.len())
                        },
                        cx,
                    ))
                    .child(
                        Button::new("run-tests")
                            .icon(IconName::Play)
                            .ghost()
                            .xsmall()
                            .disabled(busy)
                            .tooltip("Run Tests (⌘U)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.build.update(cx, |b, cx| b.start(crate::build::BuildAction::Test, cx));
                            })),
                    ),
            );
        if tests.is_empty() {
            return list.child(empty("Run tests with ⌘U to see results here.", cx));
        }
        for (ix, test) in tests.into_iter().enumerate() {
            let (icon, color) = match test.status {
                TestStatus::Passed => (IconName::CircleCheck, theme.success),
                TestStatus::Failed => (IconName::CircleX, theme.danger),
                TestStatus::Skipped => (IconName::CircleSlash, theme.muted_foreground),
            };
            list = list.child(
                row(("test", ix))
                    .child(Icon::new(icon).small().text_color(color))
                    .child(div().flex_1().truncate().child(test.name.clone()))
                    .when_some(test.duration, |el, d| {
                        el.child(div().text_xs().text_color(theme.muted_foreground).child(format!("{d:.3}s")))
                    }),
            );
        }
        list
    }

    fn render_debug_navigator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let debugger = self.debugger.read(cx);
        let state = debugger.state.clone();
        let frames = debugger.frames.clone();
        let selected = debugger.selected_frame;
        let threads = debugger.threads.clone();
        let mut list = v_flex().id("debug-nav").size_full().overflow_y_scroll();
        let status = match &state {
            crate::debugger::DebugState::Inactive | crate::debugger::DebugState::Exited => {
                return list.child(empty(
                    "Not debugging. Set a breakpoint (⌘\\) and run (⌘R) to debug.",
                    cx,
                ));
            }
            crate::debugger::DebugState::Starting => "Attaching…".to_string(),
            crate::debugger::DebugState::Running => "Running".to_string(),
            crate::debugger::DebugState::Stopped { reason, description, .. } => {
                description.clone().unwrap_or_else(|| format!("Paused ({reason})"))
            }
        };
        list = list.child(section_header(status, cx));
        if let crate::debugger::DebugState::Stopped { thread_id, .. } = &state {
            let thread_name = threads
                .iter()
                .find(|(id, _)| id == thread_id)
                .map(|(_, name)| name.clone())
                .unwrap_or_default();
            list = list.child(
                row("debug-thread")
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Icon::new(IconName::Layers).small().text_color(theme.muted_foreground))
                    .child(format!("Thread {thread_id} {thread_name}")),
            );
        }
        for (ix, frame) in frames.into_iter().enumerate() {
            let has_source = frame.path.as_ref().is_some_and(|p| p.exists());
            list = list.child(
                row(("frame", ix))
                    .pl_5()
                    .text_xs()
                    .when(ix == selected, |el| el.bg(theme.sidebar_accent))
                    .hover(|el| el.bg(theme.sidebar_accent))
                    .when(!has_source, |el| el.text_color(theme.muted_foreground))
                    .child(div().w(px(18.)).text_color(theme.muted_foreground).child(ix.to_string()))
                    .child(div().flex_1().truncate().child(frame.name.clone()))
                    .when_some(frame.path.clone().filter(|_| has_source), |el, path| {
                        el.child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(format!("{}:{}", path.file_name().unwrap_or_default().to_string_lossy(), frame.line)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.debugger.update(cx, |d, cx| d.select_frame(ix, cx));
                    })),
            );
        }
        list
    }

    fn render_breakpoint_navigator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let debugger = self.debugger.read(cx);
        let breakpoints = debugger.breakpoints.clone();
        let active = debugger.breakpoints_active;
        let mut list = v_flex()
            .id("breakpoints")
            .size_full()
            .overflow_y_scroll()
            .child(
                h_flex()
                    .justify_between()
                    .child(section_header(if active { "Breakpoints" } else { "Breakpoints (deactivated)" }, cx))
                    .child(
                        Button::new("clear-breakpoints")
                            .icon(IconName::Trash)
                            .ghost()
                            .xsmall()
                            .tooltip("Delete All Breakpoints")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.debugger.update(cx, |d, cx| d.remove_all_breakpoints(cx));
                            })),
                    ),
            );
        if breakpoints.is_empty() {
            return list.child(empty("No breakpoints. Use ⌘\\ on a line to add one.", cx));
        }
        for (file_ix, (path, lines)) in breakpoints.into_iter().enumerate() {
            list = list.child(
                row(("bp-file", file_ix))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Icon::new(file_icon(&path)).small().text_color(theme.muted_foreground))
                    .child(path.file_name().unwrap_or_default().to_string_lossy().to_string()),
            );
            for line in lines {
                let open = path.clone();
                let remove = path.clone();
                list = list.child(
                    row(SharedString::from(format!("bp-{file_ix}-{line}")))
                        .pl_6()
                        .text_xs()
                        .hover(|el| el.bg(theme.sidebar_accent))
                        .child(Icon::new(IconName::Tag).xsmall().text_color(if active {
                            theme.primary
                        } else {
                            theme.muted_foreground
                        }))
                        .child(div().flex_1().child(format!("line {line}")))
                        .child(
                            Button::new(SharedString::from(format!("bp-remove-{file_ix}-{line}")))
                                .icon(IconName::X)
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let path = remove.clone();
                                    this.debugger.update(cx, |d, cx| d.toggle_breakpoint(path, line, cx));
                                })),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let target = Target::Line { line: line.saturating_sub(1), column: 0 };
                            this.open_file(open.clone(), Some(target), true, window, cx);
                        })),
                );
            }
        }
        list
    }

    fn render_report_navigator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let reports = self.build.read(cx).reports.clone();
        let mut list = v_flex()
            .id("reports")
            .size_full()
            .overflow_y_scroll()
            .child(section_header("Reports", cx));
        if reports.is_empty() {
            return list.child(empty("Build, run or test to create a report.", cx));
        }
        for (ix, report) in reports.into_iter().enumerate() {
            let (icon, color) = if report.success {
                (IconName::CircleCheck, theme.success)
            } else {
                (IconName::CircleX, theme.danger)
            };
            let ago = report
                .finished_at
                .elapsed()
                .map(|d| match d.as_secs() {
                    s if s < 60 => "just now".to_string(),
                    s if s < 3600 => format!("{} min ago", s / 60),
                    s => format!("{} h ago", s / 3600),
                })
                .unwrap_or_default();
            let log = report.log_path.clone();
            list = list.child(
                h_flex()
                    .id(("report", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_1p5()
                    .hover(|el| el.bg(theme.sidebar_accent))
                    .child(Icon::new(icon).small().text_color(color))
                    .child(
                        v_flex()
                            .flex_1()
                            .text_xs()
                            .child(
                                div()
                                    .text_sm()
                                    .child(format!("{} {}", report.action.verb(), report.scheme)),
                            )
                            .child(div().text_color(theme.muted_foreground).child(format!(
                                "{} · {:.1}s · {} errors, {} warnings · {ago}",
                                report.destination,
                                report.duration.as_secs_f32(),
                                report.errors,
                                report.warnings
                            ))),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_file(log.clone(), None, true, window, cx);
                    })),
            );
        }
        list
    }
}

//! The navigator area (left): Project, Source Control, Bookmarks, Find,
//! Issues, Tests, Debug, Breakpoints and Reports — Xcode 27's ⌘1…⌘9. Xcode
//! 27 has no Symbol navigator; the active file's symbols are in Document
//! Items (⌃6) and the workspace's in Open Quickly (⇧⌘O).

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::Selectable as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::tree::tree;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::assets::IconName;

use super::packages::{PACKAGES_ID, package_of_item};
use super::pickers::file_icon;
use super::{Target, Workspace};
use crate::build::log::{IssueSource, Severity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigatorTab {
    Project,
    SourceControl,
    Bookmarks,
    Find,
    Issues,
    Tests,
    Debug,
    Breakpoints,
    Reports,
}

impl NavigatorTab {
    /// Its name in key contexts.
    fn key_name(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::SourceControl => "source_control",
            Self::Bookmarks => "bookmarks",
            Self::Find => "find",
            Self::Issues => "issues",
            Self::Tests => "tests",
            Self::Debug => "debug",
            Self::Breakpoints => "breakpoints",
            Self::Reports => "reports",
        }
    }

    const ALL: [NavigatorTab; 9] = [
        Self::Project,
        Self::SourceControl,
        Self::Bookmarks,
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
            Self::Bookmarks => IconName::Bookmark,
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
            Self::Bookmarks => "Bookmark Navigator (⌘3)",
            Self::Find => "Find Navigator (⌘4)",
            Self::Issues => "Issue Navigator (⌘5)",
            Self::Tests => "Test Navigator (⌘6)",
            Self::Debug => "Debug Navigator (⌘7)",
            Self::Breakpoints => "Breakpoint Navigator (⌘8)",
            Self::Reports => "Report Navigator (⌘9)",
        }
    }
}

pub(super) fn row(id: impl Into<ElementId>) -> Stateful<Div> {
    h_flex().id(id).w_full().px_2().py(px(3.)).gap_1p5().text_sm()
}

pub(super) fn section_header(label: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px_2()
        .pt_2()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(label.into())
}

pub(super) fn empty(message: impl Into<SharedString>, cx: &App) -> Div {
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
            NavigatorTab::Bookmarks => self.render_bookmark_navigator(window, cx).into_any_element(),
            NavigatorTab::Find => self.render_find_navigator(window, cx),
            NavigatorTab::Issues => self.render_issue_navigator(cx).into_any_element(),
            NavigatorTab::Tests => self.render_test_navigator(cx).into_any_element(),
            NavigatorTab::Debug => self.render_debug_navigator(cx).into_any_element(),
            NavigatorTab::Breakpoints => self.render_breakpoint_navigator(cx).into_any_element(),
            NavigatorTab::Reports => self.render_report_navigator(cx).into_any_element(),
        };

        // For keymap.json: `Navigator`, `navigator == find`.
        let context = format!("Navigator navigator={}", self.navigator.key_name());
        v_flex()
            .key_context(context.as_str())
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
        let is_xcode = self.project.kind.is_xcode();
        let package_rows = self.packages.rows.clone();
        let header = h_flex()
            .id("project-header")
            .mx_1()
            .mt_1()
            .px_1()
            .py_1()
            .gap_1()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .rounded(theme.radius)
            .when(self.showing_project_settings(), |el| el.bg(theme.sidebar_accent))
            .child(Icon::new(IconName::FolderCode).small().text_color(theme.primary))
            .child(self.project.name.clone())
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(theme.muted_foreground)
                    .child(self.project.kind.label()),
            );
        // Clicking the project opens the project editor, as in Xcode.
        let header = if is_xcode {
            let pbxproj = self
                .xcode_projects()
                .first()
                .map(|p| crate::xcodeproj::XcodeProject::pbxproj_path(p));
            let opened = self.project.kind.clone();
            let root = self.project.root.clone();
            let workspace = cx.weak_entity();
            header
                .cursor_pointer()
                .hover(|el| el.bg(theme.sidebar_accent))
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Show Project Settings").build(window, cx))
                .on_click(cx.listener(|this, _, window, cx| this.open_project_settings(window, cx)))
                .context_menu(move |menu, _, _| {
                    let (ws1, ws2) = (workspace.clone(), workspace.clone());
                    let pbxproj = pbxproj.clone();
                    let bundle = match &opened {
                        crate::project::ProjectKind::XcodeProject(p) | crate::project::ProjectKind::XcodeWorkspace(p) => p.clone(),
                        _ => root.clone(),
                    };
                    let reveal = bundle.clone();
                    menu.item(PopupMenuItem::new("Project Settings").on_click(move |_, window, cx| {
                        ws1.update(cx, |ws, cx| ws.open_project_settings(window, cx)).ok();
                    }))
                    .when_some(pbxproj, |menu, pbxproj| {
                        menu.item(PopupMenuItem::new("Open project.pbxproj").on_click(move |_, window, cx| {
                            let path = pbxproj.clone();
                            ws2.update(cx, |ws, cx| ws.open_file(path, None, true, window, cx)).ok();
                        }))
                    })
                    .separator()
                    .item(PopupMenuItem::new("Open in Xcode").on_click(move |_, _, cx| {
                        let bundle = bundle.clone();
                        cx.background_spawn(async move { super::project_settings::open_in_xcode(&bundle) }).detach();
                    }))
                    .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&reveal)))
                })
                .into_any_element()
        } else {
            header.into_any_element()
        };
        v_flex()
            .size_full()
            .child(header)
            .child(
                div().flex_1().min_h_0().child(
                    tree(&self.tree, move |ix, entry, selected, _window, cx| {
                        let item = entry.item();
                        // Package Dependencies and its packages.
                        if item.id.as_ref() == PACKAGES_ID || package_rows.contains_key(&item.id) {
                            return package_list_item(ix, entry, package_rows.get(&item.id), selected, cx);
                        }
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
                            .on_click(move |event, window, cx| {
                                let path = path.clone();
                                let next_pane = event.modifiers().alt;
                                workspace
                                    .update(cx, |this, cx| {
                                        if is_folder {
                                            // The File inspector follows the selection.
                                            this.inspected = Some(path);
                                            cx.notify();
                                        } else if next_pane {
                                            // ⌥-click: in the assistant (the next pane), as in Xcode.
                                            this.open_in_next_pane(path, None, window, cx)
                                        } else {
                                            this.open_file(path, None, true, window, cx)
                                        }
                                    })
                                    .ok();
                            })
                    })
                    .context_menu({
                        let workspace = cx.weak_entity();
                        let add_files_label: SharedString = format!("Add Files to “{}”…", self.project.name).into();
                        move |_, entry, menu, _, cx| {
                            let id = entry.item().id.clone();
                            if id.as_ref() == PACKAGES_ID {
                                return packages_menu(menu, workspace.clone());
                            }
                            if let Some(identity) = package_of_item(&id) {
                                return package_menu(menu, identity.to_string(), workspace.clone(), cx);
                            }
                            let path = PathBuf::from(entry.item().id.as_str());
                            // A package's sources are read-only.
                            if crate::packages::location::is_package_checkout(&path) {
                                return menu.item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&path)));
                            }
                            let folder = if entry.is_folder() {
                                path.clone()
                            } else {
                                path.parent().map(PathBuf::from).unwrap_or_else(|| path.clone())
                            };
                            let (ws1, ws2, ws3) = (workspace.clone(), workspace.clone(), workspace.clone());
                            let (ws4, ws5, ws6) = (workspace.clone(), workspace.clone(), workspace.clone());
                            let (p1, p2, p3, p4, p5) = (path.clone(), path.clone(), path.clone(), path.clone(), path.clone());
                            let add_to = folder.clone();
                            menu.item(PopupMenuItem::new("Show File Inspector").on_click(move |_, _, cx| {
                                let path = p4.clone();
                                ws4.update(cx, |ws, cx| ws.show_file_inspector(Some(path), cx)).ok();
                            }))
                            .separator()
                            .item(PopupMenuItem::new("New File…").on_click(move |_, window, cx| {
                                let folder = folder.clone();
                                ws1.update(cx, |ws, cx| ws.new_file_in(folder, window, cx)).ok();
                            }))
                            .item(PopupMenuItem::new(add_files_label.clone()).on_click(move |_, window, cx| {
                                let folder = add_to.clone();
                                ws5.update(cx, |ws, cx| ws.prompt_add_files(folder, window, cx)).ok();
                            }))
                            .separator()
                            .item(PopupMenuItem::new("Rename…").on_click(move |_, window, cx| {
                                let path = p1.clone();
                                ws2.update(cx, |ws, cx| ws.prompt_rename(path, window, cx)).ok();
                            }))
                            .item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| {
                                cx.reveal_path(&p2);
                            }))
                            .item(PopupMenuItem::new("Open Terminal at Location").on_click(move |_, window, cx| {
                                let path = p5.clone();
                                ws6.update(cx, |ws, cx| ws.open_terminal_at(Some(path), window, cx)).ok();
                            }))
                            .when(!entry.is_folder(), |menu| {
                                let (ws, path) = (workspace.clone(), path.clone());
                                menu.item(PopupMenuItem::new("Bookmark File").on_click(move |_, window, cx| {
                                    ws.update(cx, |ws, cx| ws.bookmark_path(path.clone(), window, cx)).ok();
                                }))
                            })
                            .when(!entry.is_folder() && crate::plist::is_property_list(&path), |menu| {
                                // Xcode's Open As ▸ Source Code / Property List.
                                let (ws1, ws2, p1, p2) = (workspace.clone(), workspace.clone(), path.clone(), path.clone());
                                menu.separator()
                                    .item(PopupMenuItem::new("Open As Source Code").on_click(move |_, window, cx| {
                                        ws1.update(cx, |ws, cx| ws.open_file_as(p1.clone(), true, window, cx)).ok();
                                    }))
                                    .item(PopupMenuItem::new("Open As Property List").on_click(move |_, window, cx| {
                                        ws2.update(cx, |ws, cx| ws.open_file_as(p2.clone(), false, window, cx)).ok();
                                    }))
                            })
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

    fn render_find_navigator(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.hierarchy.shown {
            return self.render_hierarchy(cx);
        }
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
            let bookmark_file = file.path.clone();
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
                    )
                    .context_menu({
                        let workspace = cx.weak_entity();
                        move |menu, _, _| {
                            let (workspace, path) = (workspace.clone(), bookmark_file.clone());
                            menu.item(PopupMenuItem::new("Bookmark File").on_click(move |_, window, cx| {
                                workspace.update(cx, |ws, cx| ws.bookmark_path(path.clone(), window, cx)).ok();
                            }))
                        }
                    }),
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
                let (bookmark_path, bookmark_text) = (file.path.clone(), m.preview.clone());
                list = list.child(
                    row(SharedString::from(format!("find-{file_ix}-{match_ix}")))
                        .pl_6()
                        .text_xs()
                        .hover(|el| el.bg(theme.sidebar_accent))
                        .child(div().text_color(theme.muted_foreground).w(px(28.)).child(format!("{}", line + 1)))
                        .child(div().flex_1().truncate().font_family(theme.mono_font_family.clone()).child(text))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_file(path.clone(), Some(Target::Line { line, column }), true, window, cx);
                        }))
                        .context_menu({
                            let workspace = cx.weak_entity();
                            move |menu, _, _| {
                                let (workspace, path, text) = (workspace.clone(), bookmark_path.clone(), bookmark_text.clone());
                                menu.item(PopupMenuItem::new("Bookmark Line").on_click(move |_, window, cx| {
                                    let (path, text) = (path.clone(), text.clone());
                                    workspace.update(cx, |ws, cx| ws.bookmark_find_result(path, line, text, window, cx)).ok();
                                }))
                            }
                        }),
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
                    // Find and Replace in Workspace: Replace All edits the
                    // results in a tab, unsaved and undone in one step.
                    .when_some(self.find.replace_input.clone().filter(|_| self.find.replacing), |el, replace_input| {
                        let can_replace = !self.find.searching && total > 0 && self.find.title.is_none();
                        el.child(
                            h_flex()
                                .gap_1()
                                .child(div().flex_1().child(Input::new(&replace_input).small().cleanable(true)))
                                .child(
                                    Button::new("find-replace-all")
                                        .label("Replace All")
                                        .xsmall()
                                        .disabled(!can_replace)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let replacement = this.find.replace_input.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default();
                                            this.replace_all_in_workspace(replacement, window, cx);
                                        })),
                                ),
                        )
                    })
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
                        let can_bookmark = !self.find.searching && total > 0;
                        el.child(
                            h_flex()
                                .gap_1()
                                .child(div().flex_1().text_xs().text_color(theme.muted_foreground).child(summary))
                                .when(can_bookmark, |el| {
                                    // The results in a tab, to edit in place (Zed's multibuffer).
                                    el.child(
                                        Button::new("find-in-editor")
                                            .icon(IconName::ExternalLink)
                                            .ghost()
                                            .xsmall()
                                            .tooltip("Show Find Results in Editor")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.show_find_results_in_editor(window, cx);
                                            })),
                                    )
                                })
                                .when(can_bookmark, |el| {
                                    // Xcode bookmarks the query, with its results, as a group.
                                    el.child(
                                        Button::new("find-bookmark")
                                            .icon(IconName::BookmarkPlus)
                                            .ghost()
                                            .xsmall()
                                            .tooltip("Bookmark Search Results (⌃⌘')")
                                            .on_click(cx.listener(|this, _, window, cx| this.bookmark_find_results(window, cx))),
                                    )
                                }),
                        )
                    }),
            )
            .child(div().h(px(1.)).bg(theme.border))
            .child(list)
            .into_any_element()
    }

    fn render_issue_navigator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let issues = self.all_issues(cx);
        let mut list = v_flex().id("issues").flex_1().min_h_0().overflow_y_scroll();
        if issues.is_empty() {
            return v_flex().size_full().child(list.child(empty("No issues", cx)));
        }
        // Every issue with its lines around it, in a tab (a multibuffer).
        let bar = h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(div().flex_1().text_xs().text_color(theme.muted_foreground).child(match issues.len() {
                1 => "1 issue".to_string(),
                n => format!("{n} issues"),
            }))
            .child(
                Button::new("issues-in-editor")
                    .icon(IconName::ExternalLink)
                    .ghost()
                    .xsmall()
                    .tooltip("Show Issues in Editor")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_issues_in_editor(window, cx);
                    })),
            );
        let mut current_file: Option<(Option<PathBuf>, bool)> = None;
        for (ix, issue) in issues.into_iter().enumerate() {
            let package = issue.source == IssueSource::Package;
            if current_file.as_ref() != Some(&(issue.path.clone(), package)) {
                current_file = Some((issue.path.clone(), package));
                let label = match &issue.path {
                    Some(path) => path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                    // Xcode's heading for package resolution errors.
                    None if package => "Package Loading".into(),
                    None => "Build".into(),
                };
                list = list.child(section_header(label, cx));
            }
            let (icon, color) = match issue.severity {
                Severity::Error => (IconName::CircleX, theme.danger),
                Severity::Warning => (IconName::TriangleAlert, theme.warning),
                Severity::Note => (IconName::Info, theme.muted_foreground),
            };
            let location = issue.line.map(|l| format!(":{l}")).unwrap_or_default();
            let source = match issue.source {
                IssueSource::Live => "Live",
                IssueSource::Build => "Build",
                IssueSource::Package => "Package",
            };
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
                            .min_w_0()
                            .child(div().child(issue.message.clone()))
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{source}{location}")),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.open_issue(&open, window, cx))),
            );
        }
        v_flex().size_full().child(bar).child(list)
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

/// A row of the Package Dependencies section: its header, or a package
/// with its version (or branch, or commit) beside its name.
fn package_list_item(ix: usize, entry: &gpui_kit::component::tree::TreeEntry, row: Option<&super::packages::PackageRow>, selected: bool, cx: &App) -> ListItem {
    let theme = cx.theme();
    let content = match row {
        None => h_flex()
            .gap_1p5()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .child(Icon::new(IconName::Library).small().text_color(theme.muted_foreground))
            .child(entry.item().label.clone()),
        Some(row) => h_flex()
            .gap_1p5()
            .text_sm()
            .child(Icon::new(if row.local { IconName::FolderCode } else { IconName::Package }).small().text_color(theme.primary))
            .child(div().truncate().child(row.name.clone()))
            .child(div().flex_1().truncate().text_xs().text_color(theme.muted_foreground).child(row.state.clone())),
    };
    ListItem::new(ix)
        .w_full()
        .rounded(theme.radius)
        .py(px(2.))
        .px_2()
        .pl(px(12.) * entry.depth() + px(8.))
        .selected(selected)
        .child(content)
}

/// The Package Dependencies header's menu.
fn packages_menu(menu: gpui_kit::component::menu::PopupMenu, workspace: WeakEntity<Workspace>) -> gpui_kit::component::menu::PopupMenu {
    use crate::packages::Operation;
    let (ws1, ws2, ws3, ws4, ws5) = (workspace.clone(), workspace.clone(), workspace.clone(), workspace.clone(), workspace);
    menu.item(PopupMenuItem::new("Add Package Dependencies…").on_click(move |_, window, cx| {
        ws1.update(cx, |ws, cx| ws.show_add_package(None, window, cx)).ok();
    }))
    .item(PopupMenuItem::new("Add Local Package…").on_click(move |_, window, cx| {
        ws2.update(cx, |ws, cx| ws.prompt_add_local_package(window, cx)).ok();
    }))
    .separator()
    .item(PopupMenuItem::new("Reset Package Caches").on_click(move |_, window, cx| {
        ws3.update(cx, |ws, cx| ws.package_operation(Operation::Reset, window, cx)).ok();
    }))
    .item(PopupMenuItem::new("Resolve Package Versions").on_click(move |_, window, cx| {
        ws4.update(cx, |ws, cx| ws.package_operation(Operation::Resolve, window, cx)).ok();
    }))
    .item(PopupMenuItem::new("Update to Latest Package Versions").on_click(move |_, window, cx| {
        ws5.update(cx, |ws, cx| ws.package_operation(Operation::UpdateAll, window, cx)).ok();
    }))
}

/// A package's menu: Update Package and Remove Package (for packages the
/// project declares), Show in Finder.
fn package_menu(
    menu: gpui_kit::component::menu::PopupMenu,
    identity: String,
    workspace: WeakEntity<Workspace>,
    cx: &App,
) -> gpui_kit::component::menu::PopupMenu {
    let Some(ws) = workspace.upgrade() else { return menu };
    let ws = ws.read(cx);
    let Some(package) = ws.packages.dependencies.package(&identity).cloned() else { return menu };
    // Packages a workspace lists directly aren't in any project to remove
    // them from.
    let removable = package.direct && (package.reference.is_some() || ws.project.kind == crate::project::ProjectKind::SwiftPackage);
    let (ws1, ws2) = (workspace.clone(), workspace);
    let (id1, id2) = (identity.clone(), identity);
    menu.item(PopupMenuItem::new("Update Package").disabled(package.local).on_click(move |_, window, cx| {
        ws1.update(cx, |ws, cx| ws.update_package(&id1, window, cx)).ok();
    }))
    .item(PopupMenuItem::new("Remove Package").disabled(!removable).on_click(move |_, window, cx| {
        ws2.update(cx, |ws, cx| {
            if let Err(err) = ws.remove_package(&id2, window, cx) {
                ws.notify_error(err, window, cx);
            }
        })
        .ok();
    }))
    .when_some(package.checkout, |menu, checkout| {
        menu.separator().item(PopupMenuItem::new("Show in Finder").on_click(move |_, _, cx| cx.reveal_path(&checkout)))
    })
}

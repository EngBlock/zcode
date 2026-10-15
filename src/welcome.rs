//! The welcome window shown when zcode starts without a project.

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::assets::IconName;

use crate::actions::OpenProject;
use crate::paths::display_path;
use crate::project;
use crate::session::Session;

pub struct Welcome {
    recent: Vec<PathBuf>,
    focus_handle: FocusHandle,
}

impl Welcome {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let recent = Session::load()
            .recent
            .into_iter()
            .filter(|p| p.exists())
            .collect();
        Self {
            recent,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Focusable for Welcome {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Welcome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let recent = self.recent.clone();
        h_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .child(
                        div()
                            .size(px(88.))
                            .rounded(px(20.))
                            .bg(theme.primary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Hammer).size(px(48.)).text_color(theme.primary_foreground)),
                    )
                    .child(div().text_2xl().font_weight(FontWeight::BOLD).child("zcode"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(format!("Version {} · Fast editing for Apple platforms", env!("CARGO_PKG_VERSION"))),
                    )
                    .child(div().h(px(16.)))
                    .child(
                        Button::new("open")
                            .label("Open Existing Project…")
                            .icon(IconName::FolderOpen)
                            .primary()
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenProject), cx)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(".xcodeproj, .xcworkspace, Package.swift or any folder"),
                    ),
            )
            .child(
                v_flex()
                    .w(px(320.))
                    .h_full()
                    .p_2()
                    .gap_1()
                    .bg(theme.sidebar)
                    .border_l_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .p_2()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground)
                            .child("RECENT"),
                    )
                    .when(recent.is_empty(), |el| {
                        el.child(div().p_2().text_sm().text_color(theme.muted_foreground).child("No recent projects"))
                    })
                    .children(recent.into_iter().enumerate().map(|(ix, path)| {
                        let info = project::detect(&path);
                        let icon = match info.kind {
                            project::ProjectKind::XcodeProject(_) | project::ProjectKind::XcodeWorkspace(_) => IconName::Hammer,
                            project::ProjectKind::SwiftPackage => IconName::Package,
                            project::ProjectKind::Folder => IconName::Folder,
                        };
                        h_flex()
                            .id(("recent", ix))
                            .p_2()
                            .gap_2()
                            .rounded(theme.radius)
                            .hover(|el| el.bg(theme.sidebar_accent))
                            .child(Icon::new(icon).text_color(theme.primary))
                            .child(
                                v_flex()
                                    .overflow_hidden()
                                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(info.name.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .truncate()
                                            .text_color(theme.muted_foreground)
                                            .child(display_path(&path)),
                                    ),
                            )
                            .on_click(move |_, window, cx| {
                                crate::app::open_project(path.clone(), cx);
                                window.remove_window();
                            })
                    })),
            )
            .on_action(cx.listener(|_, _: &crate::actions::CloseTab, window, _| window.remove_window()))
            .map(|el| {
                let _ = window;
                el
            })
    }
}

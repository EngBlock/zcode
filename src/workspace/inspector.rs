//! The File inspector (⌥⌘1), Xcode's right-hand panel for the selected
//! file: its name, type and location, and Target Membership, the targets
//! that build it, which can be edited in group-based projects.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::Workspace;
use crate::xcodeproj::files::{FileInfo, Tracking};

/// The inspected file, the index generation it was computed from, and what
/// the project that knows it says.
pub(super) type InspectorInfo = ((PathBuf, u64), Option<(PathBuf, FileInfo)>);

/// Xcode's name for a file type, as the File inspector's Type menu shows it.
fn type_label(file_type: &str) -> String {
    let label = match file_type {
        "sourcecode.swift" => "Swift Source",
        "sourcecode.c.objc" => "Objective-C Source",
        "sourcecode.cpp.objcpp" => "Objective-C++ Source",
        "sourcecode.c.c" => "C Source",
        "sourcecode.cpp.cpp" => "C++ Source",
        "sourcecode.c.h" => "C Header",
        "sourcecode.cpp.h" => "C++ Header",
        "sourcecode.metal" => "Metal Shader Source",
        "sourcecode.module-map" => "Module Map",
        "text.plist.xml" => "Property List",
        "text.plist.strings" => "Strings File",
        "text.json.xcstrings" => "String Catalog",
        "text.plist.entitlements" => "Entitlements File",
        "text.xcconfig" => "Configuration Settings File",
        "text.json" => "JSON",
        "text" => "Plain Text",
        "net.daringfireball.markdown" => "Markdown Text",
        "folder.assetcatalog" => "Asset Catalog",
        "file.storyboard" => "Interface Builder Storyboard",
        "file.xib" => "Interface Builder Document",
        "image.png" => "PNG Image",
        "image.jpeg" => "JPEG Image",
        "folder" => "Folder Reference",
        other => return other.to_string(),
    };
    format!("Default - {label}")
}

impl Workspace {
    /// The file the inspector shows: the navigator's pick, else the active
    /// file.
    pub(super) fn inspected_path(&self) -> Option<PathBuf> {
        self.inspected.clone().or_else(|| self.active_buffer().map(|b| b.path.clone()))
    }

    /// View ▸ Inspectors ▸ File (⌥⌘1), optionally for a specific file.
    pub fn show_file_inspector(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if path.is_some() {
            self.inspected = path;
        }
        self.show_inspector = true;
        self.refresh_project_index(cx);
        cx.notify();
    }

    pub(super) fn toggle_inspector(&mut self, cx: &mut Context<Self>) {
        self.show_inspector = !self.show_inspector;
        if self.show_inspector {
            self.refresh_project_index(cx);
        }
        cx.notify();
    }

    /// The project that knows the inspected file, and what it says about
    /// it. Cached until the selection or the projects change.
    pub fn inspected_info(&mut self) -> Option<(PathBuf, PathBuf, FileInfo)> {
        let path = self.inspected_path()?;
        let key = (path.clone(), self.project_index.generation);
        if self.inspector_info.as_ref().map(|(k, _)| k) != Some(&key) {
            self.inspector_info = Some((key, self.look_up_file(&path)));
        }
        let (_, info) = self.inspector_info.as_ref()?;
        info.clone().map(|(project, info)| (path, project, info))
    }

    fn look_up_file(&self, path: &std::path::Path) -> Option<(PathBuf, FileInfo)> {
        self.project_index
            .projects
            .iter()
            .map(|(project, _, files)| (project.clone(), files.file_info(path)))
            .find(|(_, info)| !matches!(info.tracking, Tracking::NotInProject { .. }))
    }

    pub fn inspector_summary(&self) -> serde_json::Value {
        let path = self.inspected_path();
        let info = path.as_ref().and_then(|p| self.look_up_file(p)).map(|(project, info)| (PathBuf::new(), project, info));
        let tracking = info.as_ref().map(|(_, _, info)| match &info.tracking {
            Tracking::Synchronized { folder, targets } => serde_json::json!({ "synchronized": folder, "targets": targets }),
            Tracking::Referenced { referenced, targets } => serde_json::json!({
                "referenced": referenced,
                "targets": targets.iter().map(|t| format!("{}: {}", t.name, if t.member { "on" } else { "off" })).collect::<Vec<_>>(),
            }),
            Tracking::Group => serde_json::json!("group"),
            Tracking::NotInProject { .. } => serde_json::Value::Null,
        });
        serde_json::json!({
            "shown": self.show_inspector,
            "path": path,
            "type": info.as_ref().map(|(_, _, i)| i.file_type.clone()),
            "tracking": tracking,
        })
    }

    pub(super) fn render_inspector(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let header = h_flex()
            .px_3()
            .h(px(32.))
            .flex_none()
            .gap_1p5()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(IconName::File).small().text_color(theme.muted_foreground))
            .child("File Inspector");
        let panel = v_flex().id("inspector").size_full().bg(theme.sidebar).border_l_1().border_color(theme.border).child(header);
        let Some(path) = self.inspected_path() else {
            return panel.child(div().p_3().text_sm().text_color(theme.muted_foreground).child("No Selection"));
        };
        let info = self.inspected_info();
        let loading = info.is_none() && self.project_index.generation == 0 && !self.xcode_projects.is_empty();

        let section = |label: &'static str| {
            div()
                .pt_3()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(label)
        };
        let row = |label: &'static str, value: String| {
            h_flex()
                .items_start()
                .gap_2()
                .text_xs()
                .child(div().w(px(64.)).flex_none().text_right().text_color(theme.muted_foreground).child(label))
                .child(div().flex_1().min_w_0().child(value))
        };
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let file_type = info.as_ref().map(|(_, _, i)| type_label(&i.file_type));
        let location = info.as_ref().map(|(_, _, i)| i.location.clone()).filter(|l| !l.is_empty());

        let reveal = path.clone();
        let mut body = v_flex()
            .px_3()
            .pb_3()
            .gap_1()
            .child(section("IDENTITY AND TYPE"))
            .child(row("Name", name))
            .when_some(file_type, |el, t| el.child(row("Type", t)))
            .when_some(location, |el, l| el.child(row("Location", l)))
            .child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .text_xs()
                    .child(div().w(px(64.)).flex_none().text_right().text_color(theme.muted_foreground).child("Full Path"))
                    .child(div().flex_1().min_w_0().child(path.display().to_string()))
                    .child(
                        Button::new("inspector-reveal")
                            .icon(IconName::FolderOpen)
                            .xsmall()
                            .ghost()
                            .tooltip("Show in Finder")
                            .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
                    ),
            );

        let note = |text: String| div().pt_1().text_xs().text_color(theme.muted_foreground).child(text);
        match info {
            None if loading => body = body.child(section("TARGET MEMBERSHIP")).child(note("Loading…".into())),
            None => {
                let text = match self.project.kind {
                    crate::project::ProjectKind::SwiftPackage => "Swift packages define their targets in Package.swift.",
                    _ => "This file isn’t part of an Xcode project.",
                };
                body = body.child(section("TARGET MEMBERSHIP")).child(note(text.into()));
            }
            Some((_, _, FileInfo { tracking: Tracking::Group, .. })) => {}
            Some((_, _, FileInfo { tracking: Tracking::NotInProject { .. }, .. })) => {}
            Some((_, _, FileInfo { tracking: Tracking::Synchronized { folder, targets }, .. })) => {
                body = body.child(section("TARGET MEMBERSHIP"));
                for (ix, target) in targets.iter().enumerate() {
                    body = body.child(Checkbox::new(("inspector-sync-target", ix)).label(target.clone()).checked(true).disabled(true));
                }
                body = body.child(note(format!("In the synchronized folder “{folder}”, which its targets build.")));
            }
            Some((path, project, FileInfo { tracking: Tracking::Referenced { referenced, targets }, .. })) => {
                body = body.child(section("TARGET MEMBERSHIP"));
                if targets.iter().all(|t| !t.available) {
                    body = body.child(note("Targets don’t build files of this type.".into()));
                }
                for (ix, target) in targets.into_iter().enumerate() {
                    let workspace = cx.weak_entity();
                    let (project, path, id) = (project.clone(), path.clone(), target.id.clone());
                    body = body.child(
                        Checkbox::new(("inspector-target", ix))
                            .label(target.name.clone())
                            .checked(target.member)
                            .disabled(!target.available)
                            .on_click(move |checked: &bool, window, cx| {
                                let (project, path, id, checked) = (project.clone(), path.clone(), id.clone(), *checked);
                                workspace
                                    .update(cx, |ws, cx| ws.set_target_membership(project, path, id, checked, window, cx))
                                    .ok();
                            }),
                    );
                }
                if !referenced {
                    body = body.child(note("Not in the project yet: checking a target adds it.".into()));
                }
            }
        }
        panel.child(div().id("inspector-body").flex_1().min_h_0().overflow_y_scroll().child(body))
    }
}

//! The sheet Xcode shows after you pick files in Add Files… (⌥⌘A): whether
//! to copy them into the destination folder, how to add folders, and which
//! targets build them.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::Workspace;
use crate::actions::{PickerConfirm, PickerDismiss};
use crate::xcodeproj::files::ProjectFiles;

/// What the sheet confirmed.
pub struct AddFilesRequest {
    pub paths: Vec<PathBuf>,
    pub destination: PathBuf,
    pub copy: bool,
    /// The `.xcodeproj` to add references to; `None` when the files only
    /// need copying (a synchronized folder, or not an Xcode project).
    pub project: Option<PathBuf>,
    pub targets: Vec<String>,
    pub folder_references: bool,
}

struct TargetRow {
    id: String,
    name: String,
    checked: bool,
}

pub struct AddFilesDialog {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    paths: Vec<PathBuf>,
    destination: PathBuf,
    project: Option<PathBuf>,
    project_name: Option<String>,
    /// The synchronized folder the destination is in, if any.
    synchronized: Option<String>,
    copy: bool,
    folder_references: bool,
    targets: Vec<TargetRow>,
}

impl EventEmitter<DismissEvent> for AddFilesDialog {}

impl Focusable for AddFilesDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl AddFilesDialog {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        paths: Vec<PathBuf>,
        destination: PathBuf,
        project: Option<(PathBuf, ProjectFiles)>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let synchronized = project
            .as_ref()
            .and_then(|(_, files)| files.synchronized_root(&destination).and_then(|r| r.display_name().map(str::to_string)));
        // Preselect the targets that build files like these next to them.
        let suggested: Vec<String> = project
            .as_ref()
            .map(|(_, files)| {
                let mut suggested = vec![];
                for path in &paths {
                    let landed = destination.join(path.file_name().unwrap_or_default());
                    for target in files.suggested_targets(&landed) {
                        if !suggested.contains(&target) {
                            suggested.push(target);
                        }
                    }
                }
                suggested
            })
            .unwrap_or_default();
        let targets = project
            .as_ref()
            .map(|(_, files)| {
                files
                    .targets
                    .iter()
                    .filter(|t| t.isa == "PBXNativeTarget")
                    .map(|t| TargetRow { id: t.id.clone(), name: t.name.clone(), checked: suggested.contains(&t.id) })
                    .collect()
            })
            .unwrap_or_default();
        // Xcode copies by default; files already in the folder stay put.
        let copy = paths.iter().any(|p| !p.starts_with(&destination));
        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            paths,
            project_name: project.as_ref().and_then(|(p, _)| p.file_stem()).map(|s| s.to_string_lossy().to_string()),
            project: project.map(|(p, _)| p),
            destination,
            synchronized,
            copy,
            folder_references: false,
            targets,
        }
    }

    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "paths": self.paths.len(),
            "destination": self.destination,
            "copy": self.copy,
            "synchronized": self.synchronized,
            "targets": self.targets.iter().map(|t| format!("{}: {}", t.name, if t.checked { "on" } else { "off" })).collect::<Vec<_>>(),
        })
    }

    fn confirm(&mut self, _: &PickerConfirm, window: &mut Window, cx: &mut Context<Self>) {
        // Files outside a synchronized folder only join it by being copied in.
        let copy = self.copy || self.synchronized.is_some();
        let request = AddFilesRequest {
            paths: self.paths.clone(),
            destination: self.destination.clone(),
            copy,
            project: self.project.clone().filter(|_| self.synchronized.is_none()),
            targets: self.targets.iter().filter(|t| t.checked).map(|t| t.id.clone()).collect(),
            folder_references: self.folder_references,
        };
        self.workspace.update(cx, |ws, cx| ws.finish_add_files(request, window, cx)).ok();
    }

    fn dismiss(&mut self, _: &PickerDismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn destination_label(&self) -> String {
        let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        name(&self.destination)
    }
}

impl Render for AddFilesDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let has_folders = self.paths.iter().any(|p| p.is_dir());
        let names: Vec<(String, bool)> = self
            .paths
            .iter()
            .take(6)
            .map(|p| (p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), p.is_dir()))
            .collect();
        let more = self.paths.len().saturating_sub(names.len());
        let title = match &self.project_name {
            Some(name) => format!("Add Files to “{name}”"),
            None => "Add Files".to_string(),
        };
        let section = |label: &'static str| {
            div().pt_2().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child(label)
        };

        let mut body = v_flex()
            .gap_1()
            .px_4()
            .pb_3()
            .text_sm()
            .child(
                v_flex().gap_0p5().children(names.into_iter().map(|(name, is_dir)| {
                    let icon = if is_dir { IconName::Folder } else { IconName::File };
                    h_flex()
                        .gap_1p5()
                        .child(Icon::new(icon).xsmall().text_color(theme.muted_foreground))
                        .child(name)
                })),
            )
            .when(more > 0, |el| el.child(div().text_color(theme.muted_foreground).child(format!("and {more} more"))))
            .child(section("DESTINATION"))
            .child(div().child(format!("Folder “{}”", self.destination_label())))
            .child(
                Checkbox::new("af-copy")
                    .label("Copy files to destination")
                    .checked(self.copy || self.synchronized.is_some())
                    .disabled(self.synchronized.is_some())
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.copy = *checked;
                        cx.notify();
                    })),
            );
        if let Some(folder) = &self.synchronized {
            body = body.child(div().pt_2().text_color(theme.muted_foreground).child(format!(
                "“{folder}” is a synchronized folder: files copied into it are built by its targets."
            )));
        } else if self.project.is_some() {
            if has_folders {
                body = body.child(section("FOLDERS")).child(
                    RadioGroup::horizontal("af-folders")
                        .child("Create groups")
                        .child("Create folder references")
                        .selected_index(Some(self.folder_references as usize))
                        .on_click(cx.listener(|this, ix: &usize, _, cx| {
                            this.folder_references = *ix == 1;
                            cx.notify();
                        })),
                );
            }
            body = body.child(section("TARGETS"));
            if self.targets.is_empty() {
                body = body.child(div().text_color(theme.muted_foreground).child("This project has no targets."));
            }
            for (ix, target) in self.targets.iter().enumerate() {
                body = body.child(
                    Checkbox::new(("af-target", ix))
                        .label(target.name.clone())
                        .checked(target.checked)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.targets[ix].checked = *checked;
                            cx.notify();
                        })),
                );
            }
        } else {
            body = body.child(
                div()
                    .pt_2()
                    .text_color(theme.muted_foreground)
                    .child("The files are copied into the folder; no Xcode project tracks it."),
            );
        }

        v_flex()
            .id("add-files")
            .key_context("AddFilesDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .w(px(460.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .overflow_hidden()
            .child(div().px_4().pt_3().pb_2().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(body)
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("af-cancel")
                            .label("Cancel")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| this.dismiss(&PickerDismiss, window, cx))),
                    )
                    .child(
                        Button::new("af-add")
                            .label("Add")
                            .small()
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(&PickerConfirm, window, cx))),
                    ),
            )
    }
}

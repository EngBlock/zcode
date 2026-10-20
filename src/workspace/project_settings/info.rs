//! The target's Info tab: Custom Target Properties, as Xcode shows them —
//! the keys of the target's Info.plist (`INFOPLIST_FILE`) in the property
//! list editor, with its `INFOPLIST_KEY_` build settings (for generated
//! Info.plists) listed among them. Edits are written as they're made. A
//! target without an Info.plist gets one when the first key is added: next
//! to its sources, set as `INFOPLIST_FILE`, and kept out of the target's
//! resources (a membership exception in a synchronized folder).

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use zcode_editor::EditorState;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Pane, ProjectSettings, ProjectSettingsEvent};
use crate::plist::{self, Format, Value};
use crate::workspace::plist_editor::{PlistEditor, PlistEditorEvent, SettingRow};
use crate::xcodeproj::{self, XcodeProject, files::ProjectFiles};

pub(super) struct InfoTab {
    target: String,
    /// The Info.plist, or where one will be created.
    path: PathBuf,
    exists: bool,
    format: Format,
    editor: Entity<PlistEditor>,
    /// The text as last read from or written to disk.
    disk_text: String,
    _subscriptions: Vec<Subscription>,
}

const SETTING_PREFIX: &str = "INFOPLIST_KEY_";

/// Expands the build settings that Info.plist paths use.
pub(super) fn expand_path(value: &str, project_dir: &Path, target: &str) -> PathBuf {
    let dir = project_dir.to_string_lossy();
    let mut out = value.trim().trim_matches('"').to_string();
    for (name, with) in [("SRCROOT", dir.as_ref()), ("PROJECT_DIR", dir.as_ref()), ("SOURCE_ROOT", dir.as_ref()), ("TARGET_NAME", target)] {
        out = out.replace(&format!("$({name})"), with).replace(&format!("${{{name}}}"), with).replace(&format!("${name}"), with);
    }
    let path = PathBuf::from(out);
    if path.is_absolute() { path } else { project_dir.join(path) }
}

/// A target's platform name for "Custom iOS Target Properties".
pub(super) fn platform_name(sdk: Option<&str>) -> &'static str {
    match sdk.unwrap_or("") {
        s if s.starts_with("iphone") => "iOS",
        s if s.starts_with("macosx") => "macOS",
        s if s.starts_with("watch") => "watchOS",
        s if s.starts_with("appletv") => "tvOS",
        s if s.starts_with("xr") => "visionOS",
        _ => "",
    }
}

impl ProjectSettings {
    /// The selected project's files and folders.
    pub(super) fn project_files(&self) -> Option<(PathBuf, ProjectFiles)> {
        let path = self.project_path()?;
        let text = std::fs::read_to_string(XcodeProject::pbxproj_path(&path)).ok()?;
        ProjectFiles::parse(&path, &text).ok().map(|files| (path, files))
    }

    pub(super) fn target_id(&self) -> Option<String> {
        let target = self.selection.target?;
        self.project()?.targets.get(target).map(|t| t.id.clone())
    }

    pub(super) fn target_name(&self) -> Option<String> {
        let target = self.selection.target?;
        self.project()?.targets.get(target).map(|t| t.name.clone())
    }

    /// The target's Info.plist: the one `INFOPLIST_FILE` names (and whether
    /// it exists), or where Xcode would create one.
    pub(super) fn info_plist_path(&self) -> Option<(PathBuf, bool)> {
        let (_, files) = self.project_files()?;
        let target = self.target_name()?;
        if let Some(value) = self.target_value("INFOPLIST_FILE").filter(|v| !v.is_empty()) {
            let path = expand_path(&value, &files.project_dir, &target);
            let exists = path.is_file();
            return Some((path, exists));
        }
        let path = files.target_folder(&self.target_id()?).join("Info.plist");
        let exists = path.is_file();
        Some((path, exists))
    }

    /// The target's `INFOPLIST_KEY_` settings.
    fn info_setting_rows(&self) -> Vec<SettingRow> {
        let configs = self.selected_configurations();
        let mut keys: Vec<String> = configs
            .iter()
            .flat_map(|c| c.settings.iter().map(|s| s.key.clone()))
            .filter(|k| k.starts_with(SETTING_PREFIX))
            .collect();
        keys.sort();
        keys.dedup();
        keys.into_iter()
            .map(|setting| {
                let value = self.target_value(&setting);
                let key = setting[SETTING_PREFIX.len()..].to_string();
                SettingRow { key, setting, value }
            })
            .collect()
    }

    pub(super) fn rebuild_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane != Pane::Info {
            self.info = None;
            return;
        }
        let (Some(target), Some((path, exists))) = (self.target_id(), self.info_plist_path()) else {
            self.info = None;
            return;
        };
        let rows = self.info_setting_rows();
        let on_disk = exists.then(|| std::fs::read(&path).ok().and_then(|b| plist::load(b).ok())).flatten();
        if let Some(info) = self.info.as_ref()
            && info.target == target
            && info.path == path
            && on_disk.as_ref().map_or(!info.exists, |(_, text)| *text == info.disk_text)
        {
            info.editor.update(cx, |editor, cx| editor.set_settings(rows, cx));
            return;
        }
        let (format, text) = on_disk.unwrap_or_else(|| (Format::Xml, plist::to_xml(&Value::Dict(vec![]))));
        let state = cx.new(|cx| EditorState::new(window, cx).default_value(text.clone()));
        let editor = cx.new(|cx| {
            let mut editor = PlistEditor::new(state, Some(&plist::definitions::INFO_PLIST), cx).hide_root();
            editor.set_settings(rows, cx);
            editor
        });
        let subscription = cx.subscribe_in(&editor, window, |this, _, event: &PlistEditorEvent, window, cx| match event {
            PlistEditorEvent::Edited => this.save_info(cx),
            PlistEditorEvent::SetBuildSetting { setting, value } => {
                let configs = this.configuration_ids();
                if this.write(setting, value.as_deref(), &configs, false, cx) {
                    this.rebuild_info(window, cx);
                }
            }
            PlistEditorEvent::OpenAsSource => this.open_info_plist(cx),
        });
        self.info = Some(InfoTab { target, path, exists, format, editor, disk_text: text, _subscriptions: vec![subscription] });
    }

    /// Writes the Info.plist after an edit, creating it (and wiring it into
    /// the project) the first time.
    fn save_info(&mut self, cx: &mut Context<Self>) {
        let Some(info) = self.info.as_ref() else { return };
        let text = info.editor.read(cx).text_entity().read(cx).text().to_string();
        if text == info.disk_text {
            return;
        }
        let (path, exists, format, target) = (info.path.clone(), info.exists, info.format, info.target.clone());
        let written = plist::encode(&text, format).map_err(|e| format!("{e:#}")).and_then(|bytes| self.write_file(&path, &bytes, cx));
        if let Err(err) = written {
            self.error = Some(format!("Couldn’t save {}: {err}", path.display()));
            cx.notify();
            return;
        }
        if let Some(info) = self.info.as_mut() {
            info.disk_text = text;
            info.exists = true;
        }
        if !exists {
            self.wire_info_plist(&path, &target, cx);
        }
        cx.notify();
    }

    /// Points the target at a new Info.plist: `INFOPLIST_FILE`, and the file
    /// in the project without being copied as a resource.
    pub(super) fn wire_info_plist(&mut self, path: &Path, target: &str, cx: &mut Context<Self>) {
        let configs = self.configuration_ids();
        let path = path.to_path_buf();
        let target = target.to_string();
        self.edit_project("Couldn’t add the Info.plist to the project", cx, move |xcodeproj, text| {
            let text = xcodeproj::files::add_target_file(xcodeproj, &text, &path, &target, true)?.unwrap_or(text);
            let project_dir = ProjectFiles::parse(xcodeproj, &text)?.project_dir;
            let relative = crate::paths::relative_path(&project_dir, &path).to_string_lossy().to_string();
            let mut text = text;
            for id in &configs {
                text = xcodeproj::set_build_setting(&text, id, "INFOPLIST_FILE", Some(&relative), false)?;
            }
            Ok(text)
        });
    }

    fn open_info_plist(&mut self, cx: &mut Context<Self>) {
        if let Some(info) = self.info.as_ref().filter(|i| i.exists) {
            cx.emit(ProjectSettingsEvent::OpenFile(info.path.clone()));
        }
    }

    pub fn info_editor(&self) -> Option<Entity<PlistEditor>> {
        self.info.as_ref().filter(|_| self.pane == Pane::Info).map(|i| i.editor.clone())
    }

    pub(super) fn info_summary(&self, cx: &App) -> Option<serde_json::Value> {
        let info = self.info.as_ref()?;
        let mut summary = info.editor.read(cx).summary(cx);
        summary["path"] = info.path.to_string_lossy().to_string().into();
        summary["exists"] = info.exists.into();
        Some(summary)
    }

    pub(super) fn render_info(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(info) = self.info.as_ref() else {
            return div().p_4().text_sm().text_color(theme.muted_foreground).child("Select a target.").into_any_element();
        };
        let platform = platform_name(self.effective_value("SDKROOT").as_deref());
        let title = if platform.is_empty() { "Custom Target Properties".to_string() } else { format!("Custom {platform} Target Properties") };
        let project_dir = self.project_files().map(|(_, f)| f.project_dir).unwrap_or_default();
        let relative = crate::paths::relative_path(&project_dir, &info.path).to_string_lossy().to_string();
        let note = if info.exists {
            format!("Info.plist File: {relative}")
        } else {
            format!("Generated from build settings. Adding a key creates {relative}.")
        };
        let editor = info.editor.clone();
        let exists = info.exists;
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
                            .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(note)),
                    )
                    .child(
                        Button::new("ps-info-add")
                            .icon(IconName::Plus)
                            .label("Add Item")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(info) = this.info.as_ref() {
                                    info.editor.clone().update(cx, |editor, cx| editor.add_item(None, window, cx));
                                }
                            })),
                    )
                    .when(exists, |el| {
                        el.child(
                            Button::new("ps-info-open")
                                .icon(IconName::FileCode)
                                .label("Info.plist")
                                .small()
                                .ghost()
                                .tooltip("Open the Info.plist in a tab")
                                .on_click(cx.listener(|this, _, _, cx| this.open_info_plist(cx))),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().child(editor))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{expand_path, platform_name};

    #[test]
    fn expands_info_plist_paths() {
        let dir = Path::new("/p");
        assert_eq!(expand_path("App/Info.plist", dir, "App"), PathBuf::from("/p/App/Info.plist"));
        assert_eq!(expand_path("$(SRCROOT)/$(TARGET_NAME)/Info.plist", dir, "App"), PathBuf::from("/p/App/Info.plist"));
        assert_eq!(expand_path("\"${PROJECT_DIR}/X.plist\"", dir, "App"), PathBuf::from("/p/X.plist"));
        assert_eq!(expand_path("/abs/Info.plist", dir, "App"), PathBuf::from("/abs/Info.plist"));
    }

    #[test]
    fn names_platforms() {
        assert_eq!(platform_name(Some("iphoneos")), "iOS");
        assert_eq!(platform_name(Some("macosx")), "macOS");
        assert_eq!(platform_name(None), "");
    }
}

//! The project editor Xcode shows when you click the project in the
//! navigator: the project and its targets on the left, and General, Signing
//! & Capabilities (`signing.rs`), Info (`info.rs`) and Build Settings on the
//! right. Edits are written straight to `project.pbxproj` (and the target's
//! Info.plist and entitlements). A field is written when it's left, as in
//! Xcode; ⌘Z and ⇧⌘Z put the files back as they were, byte for byte.

mod info;
mod signing;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::xcodeproj::{self, BuildConfiguration, XcodeProject};
use zcode_editor::{Redo, Undo};

pub enum ProjectSettingsEvent {
    /// Open a file (the raw `project.pbxproj`) in an editor tab.
    OpenFile(PathBuf),
    /// The project file was written.
    Changed,
    /// A file was created or deleted (an Info.plist, entitlements, or one
    /// of them undone).
    FilesChanged,
}

/// One file as an edit left it: its bytes before and after (`None`: it
/// didn't exist).
#[derive(Clone, Debug, PartialEq)]
struct FileChange {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

/// Puts files back as `changes` found them (`undo`), or as they left them,
/// after checking that each is still as the step expects. Returns whether a
/// file was created or deleted.
fn restore(changes: &[FileChange], undo: bool) -> Result<bool, String> {
    let from = |c: &FileChange| if undo { (c.after.clone(), c.before.clone()) } else { (c.before.clone(), c.after.clone()) };
    for change in changes {
        let (expected, _) = from(change);
        if std::fs::read(&change.path).ok() != expected {
            return Err(format!("“{}” changed since", change.path.file_name().unwrap_or_default().to_string_lossy()));
        }
    }
    let mut created_or_deleted = false;
    let ordered: Vec<&FileChange> = if undo { changes.iter().rev().collect() } else { changes.iter().collect() };
    for change in ordered {
        let (expected, bytes) = from(change);
        created_or_deleted |= expected.is_none() != bytes.is_none();
        let result = match bytes {
            Some(bytes) => write_bytes(&change.path, &bytes),
            None => std::fs::remove_file(&change.path),
        };
        result.map_err(|err| format!("{}: {err}", change.path.display()))?;
    }
    Ok(created_or_deleted)
}

/// Writes a file, making its folder if needed.
fn write_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, bytes)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    General,
    /// Signing & Capabilities (targets only).
    Signing,
    /// The target's Info.plist (targets only).
    Info,
    BuildSettings,
}

/// Where fields and targets were last drawn (window points), for the
/// control channel's clicks.
type Places = Rc<RefCell<BTreeMap<String, Bounds<Pixels>>>>;

/// Records the bounds of the element it's put in (which must be relative).
fn place(places: &Places, key: String) -> impl IntoElement {
    let places = places.clone();
    canvas(move |bounds, _, _| {
        places.borrow_mut().insert(key, bounds);
    }, |_, _, _, _| {})
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// What is selected in the sidebar: a project, or one of its targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    project: usize,
    target: Option<usize>,
}

/// A General-pane row, backed by one build setting.
struct GeneralField {
    label: &'static str,
    key: &'static str,
    /// Shown for targets only, not the project.
    target_only: bool,
}

const GENERAL: &[GeneralField] = &[
    GeneralField { label: "Display Name", key: "INFOPLIST_KEY_CFBundleDisplayName", target_only: true },
    GeneralField { label: "Bundle Identifier", key: "PRODUCT_BUNDLE_IDENTIFIER", target_only: true },
    GeneralField { label: "Version", key: "MARKETING_VERSION", target_only: true },
    GeneralField { label: "Build", key: "CURRENT_PROJECT_VERSION", target_only: true },
    GeneralField { label: "Product Name", key: "PRODUCT_NAME", target_only: true },
    GeneralField { label: "iOS Deployment Target", key: "IPHONEOS_DEPLOYMENT_TARGET", target_only: false },
    GeneralField { label: "macOS Deployment Target", key: "MACOSX_DEPLOYMENT_TARGET", target_only: false },
    GeneralField { label: "watchOS Deployment Target", key: "WATCHOS_DEPLOYMENT_TARGET", target_only: false },
    GeneralField { label: "tvOS Deployment Target", key: "TVOS_DEPLOYMENT_TARGET", target_only: false },
    GeneralField { label: "visionOS Deployment Target", key: "XROS_DEPLOYMENT_TARGET", target_only: false },
    GeneralField { label: "Base SDK", key: "SDKROOT", target_only: false },
    GeneralField { label: "Supported Platforms", key: "SUPPORTED_PLATFORMS", target_only: false },
    GeneralField { label: "Targeted Device Family", key: "TARGETED_DEVICE_FAMILY", target_only: false },
    GeneralField { label: "Swift Language Version", key: "SWIFT_VERSION", target_only: false },
    GeneralField { label: "Code Signing Style", key: "CODE_SIGN_STYLE", target_only: false },
    GeneralField { label: "Development Team", key: "DEVELOPMENT_TEAM", target_only: false },
    GeneralField { label: "Info.plist File", key: "INFOPLIST_FILE", target_only: true },
    GeneralField { label: "Entitlements File", key: "CODE_SIGN_ENTITLEMENTS", target_only: true },
];

/// Deployment targets and platform rows that only make sense when set.
fn shown_when_unset(key: &str) -> bool {
    !matches!(
        key,
        "IPHONEOS_DEPLOYMENT_TARGET"
            | "MACOSX_DEPLOYMENT_TARGET"
            | "WATCHOS_DEPLOYMENT_TARGET"
            | "TVOS_DEPLOYMENT_TARGET"
            | "XROS_DEPLOYMENT_TARGET"
            | "SUPPORTED_PLATFORMS"
            | "INFOPLIST_FILE"
            | "CODE_SIGN_ENTITLEMENTS"
    )
}

/// An editable value. General fields write every configuration at once,
/// like Xcode's General tab; build-settings rows write one.
struct Field {
    label: SharedString,
    key: String,
    configs: Vec<String>,
    is_list: bool,
    /// The value as stored, or `None` when unset or different per
    /// configuration.
    original: Option<String>,
    input: Entity<InputState>,
    _subscription: Subscription,
}

pub struct ProjectSettings {
    focus_handle: FocusHandle,
    projects: Vec<Result<XcodeProject, String>>,
    paths: Vec<PathBuf>,
    mtimes: Vec<Option<SystemTime>>,
    selection: Selection,
    pane: Pane,
    configuration: usize,
    filter: Entity<InputState>,
    new_key: Entity<InputState>,
    new_value: Entity<InputState>,
    fields: Vec<Field>,
    info: Option<info::InfoTab>,
    signing: signing::SigningTab,
    error: Option<String>,
    /// The field being edited, as of the last render: when another takes
    /// the keyboard (or nothing does), it's written.
    editing: Option<String>,
    /// Edits' file changes, for ⌘Z and ⇧⌘Z; a step is everything one
    /// action wrote.
    undo: Vec<Vec<FileChange>>,
    redo: Vec<Vec<FileChange>>,
    /// Writes join the last undo step until the current action is done.
    grouping: bool,
    places: Places,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ProjectSettingsEvent> for ProjectSettings {}

impl Focusable for ProjectSettings {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(XcodeProject::pbxproj_path(path)).and_then(|m| m.modified()).ok()
}

impl ProjectSettings {
    pub fn new(paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let new_key = cx.new(|cx| InputState::new(window, cx).placeholder("SETTING_NAME"));
        let new_value = cx.new(|cx| InputState::new(window, cx).placeholder("Value"));
        let subscriptions = vec![
            cx.observe(&filter, |_, _, cx| cx.notify()),
            cx.subscribe_in(&new_value, window, |this, _, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_setting(window, cx);
                }
            }),
        ];
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            projects: vec![],
            mtimes: vec![],
            paths,
            selection: Selection { project: 0, target: None },
            pane: Pane::General,
            configuration: 0,
            filter,
            new_key,
            new_value,
            fields: vec![],
            info: None,
            signing: Default::default(),
            error: None,
            editing: None,
            undo: vec![],
            redo: vec![],
            grouping: false,
            places: Places::default(),
            _subscriptions: subscriptions,
        };
        this.load();
        // Start on the first target, as Xcode does when it has one.
        if this.project().is_some_and(|p| !p.targets.is_empty()) {
            this.selection.target = Some(0);
        }
        this.rebuild_fields(window, cx);
        this
    }

    fn load(&mut self) {
        self.projects = self
            .paths
            .iter()
            .map(|path| XcodeProject::load(path).map_err(|err| format!("{err:#}")))
            .collect();
        self.mtimes = self.paths.iter().map(|p| mtime(p)).collect();
    }

    /// Takes the keyboard when what had it (a field, the Info tab's outline)
    /// was just replaced, so shortcuts keep working.
    fn keep_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if window.focused(cx).is_none() {
            self.focus_handle.focus(window, cx);
        }
    }

    /// Re-reads the project files if something else changed them.
    pub fn reload_if_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current: Vec<_> = self.paths.iter().map(|p| mtime(p)).collect();
        if current != self.mtimes {
            self.reload(window, cx);
        }
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load();
        let targets = self.project().map_or(0, |p| p.targets.len());
        if self.selection.target.is_some_and(|t| t >= targets) {
            self.selection.target = None;
        }
        self.rebuild_fields(window, cx);
        cx.notify();
    }

    fn project(&self) -> Option<&XcodeProject> {
        self.projects.get(self.selection.project)?.as_ref().ok()
    }

    fn selected_configurations(&self) -> &[BuildConfiguration] {
        let Some(project) = self.project() else { return &[] };
        match self.selection.target {
            Some(t) => project.targets.get(t).map(|t| t.configurations.as_slice()).unwrap_or(&[]),
            None => &project.configurations,
        }
    }

    pub fn summary(&self, cx: &App) -> serde_json::Value {
        let project = self.project();
        serde_json::json!({
            "project": project.map(|p| p.name.clone()),
            "target": self.selection.target.and_then(|t| project?.targets.get(t)).map(|t| t.name.clone()),
            "pane": format!("{:?}", self.pane),
            "configuration": self.selected_configurations().get(self.configuration).map(|c| c.name.clone()),
            "fields": self.fields.iter().map(|f| format!("{} = {}", f.key, f.original.clone().unwrap_or_default())).collect::<Vec<_>>(),
            "info": self.info_summary(cx),
            "signing": self.signing_summary(cx),
            "error": self.error,
            "undo": self.undo.len(),
            "redo": self.redo.len(),
            "places": self.places.borrow().iter().map(|(k, b)| (k.clone(), serde_json::json!([f32::from(b.center().x), f32::from(b.center().y)]))).collect::<serde_json::Map<_, _>>(),
        })
    }

    fn select(&mut self, selection: Selection, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection == selection {
            return;
        }
        // Into the project the fields are for, before it changes.
        self.commit_pending(cx);
        let config_name = self.selected_configurations().get(self.configuration).map(|c| c.name.clone());
        self.selection = selection;
        if selection.target.is_none() && matches!(self.pane, Pane::Signing | Pane::Info) {
            self.pane = Pane::General;
        }
        self.configuration = config_name
            .and_then(|name| self.selected_configurations().iter().position(|c| c.name == name))
            .unwrap_or(0);
        self.error = None;
        self.rebuild_fields(window, cx);
        cx.notify();
    }

    /// Selects a target by name (or the project, for an empty name) and a
    /// pane, as clicking them does (for the debug control channel).
    pub fn show(&mut self, target: &str, pane: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let project = self.selection.project;
        let target = match target {
            "" => None,
            name => Some(self.project().and_then(|p| p.targets.iter().position(|t| t.name == name)).ok_or(format!("no target {name}"))?),
        };
        let pane = match pane {
            "General" | "" => Pane::General,
            "Signing" => Pane::Signing,
            "Info" => Pane::Info,
            "BuildSettings" => Pane::BuildSettings,
            other => return Err(format!("no pane {other}")),
        };
        self.select(Selection { project, target }, window, cx);
        self.set_pane(pane, window, cx);
        Ok(())
    }

    /// The Signing & Capabilities tab of the selected target (or the first).
    pub fn show_signing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.target.is_none() && self.project().is_some_and(|p| !p.targets.is_empty()) {
            self.select(Selection { project: self.selection.project, target: Some(0) }, window, cx);
        }
        if self.selection.target.is_some() {
            self.set_pane(Pane::Signing, window, cx);
        }
    }

    fn set_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.pane = pane;
        self.rebuild_fields(window, cx);
        cx.notify();
    }

    fn set_configuration(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.configuration = ix;
        self.rebuild_fields(window, cx);
        cx.notify();
    }

    /// Recreates the inputs for the current selection and pane, first
    /// writing what was typed in the old ones.
    fn rebuild_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_pending(cx);
        self.editing = None;
        self.places.borrow_mut().clear();
        self.rebuild_info(window, cx);
        self.rebuild_signing(window, cx);
        self.keep_focus(window, cx);
        let configs = self.selected_configurations().to_vec();
        let is_target = self.selection.target.is_some();
        // (label, key, configuration ids, is a list, value, placeholder)
        type Spec = (SharedString, String, Vec<String>, bool, Option<String>, Option<String>);
        let mut specs: Vec<Spec> = vec![];
        match self.pane {
            Pane::Signing | Pane::Info => {}
            Pane::General => {
                let ids: Vec<String> = configs.iter().map(|c| c.id.clone()).collect();
                for field in GENERAL {
                    if field.target_only && !is_target {
                        continue;
                    }
                    let values: Vec<Option<&xcodeproj::Setting>> = configs.iter().map(|c| c.get(field.key)).collect();
                    let first = values.first().copied().flatten();
                    let same = values.iter().all(|v| v.map(|s| &s.value) == first.map(|s| &s.value));
                    let original = if same { first.map(|s| s.value.clone()) } else { None };
                    let inherited = is_target.then(|| self.project_value(field.key)).flatten();
                    if first.is_none() && same && inherited.is_none() && !shown_when_unset(field.key) {
                        continue;
                    }
                    let placeholder = if !same {
                        Some("Multiple values".to_string())
                    } else {
                        inherited.map(|v| format!("{v} (project)"))
                    };
                    let is_list = first.is_some_and(|s| s.is_list);
                    specs.push((field.label.into(), field.key.to_string(), ids.clone(), is_list, original, placeholder));
                }
            }
            Pane::BuildSettings => {
                if let Some(config) = configs.get(self.configuration) {
                    for setting in &config.settings {
                        specs.push((
                            setting.key.clone().into(),
                            setting.key.clone(),
                            vec![config.id.clone()],
                            setting.is_list,
                            Some(setting.value.clone()),
                            None,
                        ));
                    }
                }
            }
        }

        self.fields = specs
            .into_iter()
            .map(|(label, key, configs, is_list, original, placeholder)| {
                let input = cx.new(|cx| {
                    let mut state = InputState::new(window, cx);
                    if let Some(p) = placeholder {
                        state = state.placeholder(p);
                    }
                    state.default_value(original.clone().unwrap_or_default())
                });
                let subscription = cx.subscribe_in(&input, window, {
                    let key = key.clone();
                    move |this, _, event: &InputEvent, _, cx| {
                        if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                            this.commit(&key, cx);
                        }
                    }
                });
                Field { label, key, configs, is_list, original, input, _subscription: subscription }
            })
            .collect();
    }

    fn project_value(&self, key: &str) -> Option<String> {
        let project = self.project()?;
        let values: Vec<_> = project.configurations.iter().map(|c| c.get(key).map(|s| s.value.clone())).collect();
        let first = values.first().cloned().flatten()?;
        values.iter().all(|v| v.as_ref() == Some(&first)).then_some(first)
    }

    /// Writes every field whose value was typed but not written yet (also
    /// as the editor closes).
    pub fn commit_pending(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<String> = self.fields.iter().map(|f| f.key.clone()).collect();
        for key in keys {
            self.commit(&key, cx);
        }
    }

    /// Writes the field that was being edited once the keyboard has left
    /// it. Blur says so too, but GPUI doesn't send it while the window is
    /// inactive, nor when the field is gone before the next frame.
    fn follow_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.fields.iter().find(|f| f.input.read(cx).focus_handle(cx).is_focused(window)).map(|f| f.key.clone());
        if focused == self.editing {
            return;
        }
        if let Some(left) = std::mem::replace(&mut self.editing, focused) {
            cx.defer_in(window, move |this, _, cx| this.commit(&left, cx));
        }
    }

    /// Writes a field's value if it changed. An empty value removes the
    /// setting so it inherits again.
    fn commit(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(field) = self.fields.iter().find(|f| f.key == key) else { return };
        let value = field.input.read(cx).value().trim().to_string();
        let new = (!value.is_empty()).then_some(value);
        if new == field.original || (new.is_none() && field.original.is_none()) {
            return;
        }
        let (configs, is_list) = (field.configs.clone(), field.is_list);
        if self.write(key, new.as_deref(), &configs, is_list, cx)
            && let Some(field) = self.fields.iter_mut().find(|f| f.key == key)
        {
            field.original = new;
        }
    }

    /// Types `value` into the field for `key` and commits it, as a user
    /// would (for the debug control channel).
    pub fn set_field(&mut self, key: &str, value: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let field = self.fields.iter().find(|f| f.key == key).ok_or_else(|| format!("no field {key}"))?;
        field.input.clone().update(cx, |input, cx| input.set_value(value.to_string(), window, cx));
        self.commit(key, cx);
        match &self.error {
            Some(err) => Err(err.clone()),
            None => Ok(()),
        }
    }

    /// Applies a setting to the given configurations and saves the file.
    fn write(&mut self, key: &str, value: Option<&str>, configs: &[String], is_list: bool, cx: &mut Context<Self>) -> bool {
        self.edit_project(&format!("Couldn’t save {key}"), cx, |_, mut text| {
            for id in configs {
                text = xcodeproj::set_build_setting(&text, id, key, value, is_list)?;
            }
            Ok(text)
        })
    }

    /// Rewrites the selected project's `project.pbxproj` through `edit`
    /// (given the `.xcodeproj` and the file's text), then re-reads it.
    fn edit_project(
        &mut self,
        what: &str,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&Path, String) -> anyhow::Result<String>,
    ) -> bool {
        let Some(path) = self.paths.get(self.selection.project).cloned() else { return false };
        let file = XcodeProject::pbxproj_path(&path);
        let result = std::fs::read_to_string(&file)
            .map_err(anyhow::Error::from)
            .and_then(|before| {
                let text = edit(&path, before.clone())?;
                if text != before {
                    xcodeproj::write_project_file(&file, &text)?;
                }
                Ok((before, text))
            })
            .and_then(|(before, text)| Ok((XcodeProject::from_text(&path, &text)?, before, text)));
        match result {
            Ok((project, before, text)) => {
                self.record(&file, Some(before.into_bytes()), Some(text.into_bytes()), cx);
                self.projects[self.selection.project] = Ok(project);
                self.mtimes[self.selection.project] = mtime(&path);
                self.error = None;
                cx.emit(ProjectSettingsEvent::Changed);
                cx.notify();
                true
            }
            Err(err) => {
                self.error = Some(format!("{what}: {err:#}"));
                cx.notify();
                false
            }
        }
    }

    /// Writes one of the target's files (its Info.plist or entitlements),
    /// as an undoable change.
    pub(super) fn write_file(&mut self, path: &Path, bytes: &[u8], cx: &mut Context<Self>) -> Result<(), String> {
        let before = std::fs::read(path).ok();
        write_bytes(path, bytes).map_err(|e| e.to_string())?;
        if before.is_none() {
            cx.emit(ProjectSettingsEvent::FilesChanged);
        }
        self.record(path, before, Some(bytes.to_vec()), cx);
        Ok(())
    }

    /// Adds a file's change to the undo step of the action under way, or
    /// starts one.
    fn record(&mut self, path: &Path, before: Option<Vec<u8>>, after: Option<Vec<u8>>, cx: &mut Context<Self>) {
        if before == after {
            return;
        }
        self.redo.clear();
        if self.grouping
            && let Some(step) = self.undo.last_mut()
        {
            match step.iter_mut().find(|c| c.path == path) {
                Some(change) => change.after = after,
                None => step.push(FileChange { path: path.to_path_buf(), before, after }),
            }
            return;
        }
        self.undo.push(vec![FileChange { path: path.to_path_buf(), before, after }]);
        self.grouping = true;
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            this.update(cx, |this, _| this.grouping = false).ok();
        });
    }

    /// ⌘Z: the last edit's files as they were before it.
    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        self.step(true, window, cx)
    }

    /// ⇧⌘Z.
    pub fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        self.step(false, window, cx)
    }

    fn step(&mut self, undo: bool, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        // A value typed and not written yet is an edit of its own first.
        self.commit_pending(cx);
        self.grouping = false;
        let stack = if undo { &mut self.undo } else { &mut self.redo };
        let step = stack.pop().ok_or(if undo { "nothing to undo" } else { "nothing to redo" })?;
        let result = restore(&step, undo);
        match &result {
            Ok(_) => {
                if undo { self.redo.push(step) } else { self.undo.push(step) }
                self.error = None;
            }
            Err(err) => {
                // The files moved on without zcode: the steps no longer apply.
                self.undo.clear();
                self.redo.clear();
                self.error = Some(format!("Couldn’t {}: {err}.", if undo { "undo" } else { "redo" }));
            }
        }
        self.reload(window, cx);
        cx.emit(ProjectSettingsEvent::Changed);
        if result.as_ref().is_ok_and(|created_or_deleted| *created_or_deleted) {
            cx.emit(ProjectSettingsEvent::FilesChanged);
        }
        result.map(|_| ())
    }

    /// A target setting's value when every configuration has the same one.
    fn target_value(&self, key: &str) -> Option<String> {
        let configs = self.selected_configurations();
        let first = configs.first()?.get(key).map(|s| s.value.clone());
        configs.iter().all(|c| c.get(key).map(|s| s.value.clone()) == first).then_some(first).flatten()
    }

    /// The setting's value as the target builds with it: its own, or the
    /// project's.
    fn effective_value(&self, key: &str) -> Option<String> {
        self.target_value(key).or_else(|| self.project_value(key))
    }

    fn configuration_ids(&self) -> Vec<String> {
        self.selected_configurations().iter().map(|c| c.id.clone()).collect()
    }

    /// The selected `.xcodeproj`.
    fn project_path(&self) -> Option<PathBuf> {
        self.paths.get(self.selection.project).cloned()
    }

    fn remove_setting(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = self.selected_configurations().get(self.configuration) else { return };
        let id = config.id.clone();
        if self.write(&key, None, &[id], false, cx) {
            self.rebuild_fields(window, cx);
        }
    }

    fn add_setting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.new_key.read(cx).value().trim().to_string();
        let value = self.new_value.read(cx).value().trim().to_string();
        if key.is_empty() {
            self.new_key.update(cx, |input, cx| input.focus(window, cx));
            return;
        }
        if !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'[' || b == b']' || b == b'=' || b == b'*') {
            self.error = Some(format!("“{key}” isn’t a valid build setting name."));
            cx.notify();
            return;
        }
        let Some(config) = self.selected_configurations().get(self.configuration) else { return };
        let id = config.id.clone();
        if self.write(&key, Some(&value), &[id], false, cx) {
            self.new_key.update(cx, |input, cx| input.set_value("", window, cx));
            self.new_value.update(cx, |input, cx| input.set_value("", window, cx));
            self.rebuild_fields(window, cx);
        }
    }

    fn open_pbxproj(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.paths.get(self.selection.project) {
            cx.emit(ProjectSettingsEvent::OpenFile(XcodeProject::pbxproj_path(path)));
        }
    }

    fn open_in_xcode(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.paths.get(self.selection.project).cloned() else { return };
        cx.background_spawn(async move { crate::xcode_app::open_project(&path) }).detach();
    }

    // ------------------------------------------------------------------
    // Rendering

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let header = |label: &'static str| {
            div()
                .px_3()
                .pt_3()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(label)
        };
        let row = |id: SharedString, icon: IconName, label: String, selected: bool| {
            h_flex()
                .id(id)
                .mx_1()
                .px_2()
                .py(px(3.))
                .gap_1p5()
                .text_sm()
                .rounded(theme.radius)
                .when(selected, |el| el.bg(theme.accent).text_color(theme.accent_foreground))
                .when(!selected, |el| el.hover(|el| el.bg(theme.secondary_hover)))
                .child(Icon::new(icon).small().text_color(theme.primary))
                .child(div().truncate().child(label))
        };

        let mut list = v_flex()
            .id("project-settings-sidebar")
            .w(px(210.))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(header("PROJECT"));
        for (ix, path) in self.paths.iter().enumerate() {
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let selected = self.selection == Selection { project: ix, target: None };
            list = list.child(
                row(format!("ps-project-{ix}").into(), IconName::FolderCode, name, selected).on_click(cx.listener(
                    move |this, _, window, cx| this.select(Selection { project: ix, target: None }, window, cx),
                )),
            );
        }
        if let Some(project) = self.project() {
            let project_ix = self.selection.project;
            list = list.child(header("TARGETS"));
            if project.targets.is_empty() {
                list = list.child(div().px_3().text_sm().text_color(theme.muted_foreground).child("No targets"));
            }
            let rows: Vec<_> = project
                .targets
                .iter()
                .enumerate()
                .map(|(ix, target)| {
                    let selected = self.selection.target == Some(ix);
                    row(format!("ps-target-{ix}").into(), target_icon(target), target.name.clone(), selected)
                        .relative()
                        .child(place(&self.places, format!("target:{}", target.name)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select(Selection { project: project_ix, target: Some(ix) }, window, cx)
                        }))
                })
                .collect();
            list = list.children(rows);
        }
        list
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (title, subtitle) = match (self.project(), self.selection.target) {
            (Some(project), Some(t)) => {
                let target = &project.targets[t];
                (target.name.clone(), format!("{} · {}", target.kind_label(), project.name))
            }
            (Some(project), None) => (
                project.name.clone(),
                format!("Project · objectVersion {}", project.object_version.as_deref().unwrap_or("?")),
            ),
            (None, _) => ("Project".into(), String::new()),
        };
        let pane_button = |id: &'static str, label: &'static str, pane: Pane, current: Pane| {
            Button::new(id).label(label).small().ghost().selected(pane == current)
        };
        let pane = self.pane;
        v_flex()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).truncate().child(title))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(subtitle)),
                    )
                    .child(
                        Button::new("ps-open-pbxproj")
                            .icon(IconName::FileCode)
                            .label("project.pbxproj")
                            .small()
                            .ghost()
                            .tooltip("Open the project file as text")
                            .on_click(cx.listener(|this, _, _, cx| this.open_pbxproj(cx))),
                    )
                    .child(
                        Button::new("ps-open-xcode")
                            .icon(IconName::ExternalLink)
                            .label("Open in Xcode")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.open_in_xcode(cx))),
                    )
                    .child(
                        Button::new("ps-reload")
                            .icon(IconName::RefreshCw)
                            .small()
                            .ghost()
                            .tooltip("Reload from Disk")
                            .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
                    ),
            )
            .child(
                h_flex()
                    .px_3()
                    .pb_1()
                    .gap_1()
                    .child(
                        pane_button("ps-pane-general", if self.selection.target.is_some() { "General" } else { "Info" }, Pane::General, pane)
                            .on_click(cx.listener(|this, _, window, cx| this.set_pane(Pane::General, window, cx))),
                    )
                    .when(self.selection.target.is_some(), |el| {
                        el.child(
                            pane_button("ps-pane-signing", "Signing & Capabilities", Pane::Signing, pane)
                                .on_click(cx.listener(|this, _, window, cx| this.set_pane(Pane::Signing, window, cx))),
                        )
                        .child(
                            pane_button("ps-pane-info", "Info", Pane::Info, pane)
                                .on_click(cx.listener(|this, _, window, cx| this.set_pane(Pane::Info, window, cx))),
                        )
                    })
                    .child(
                        pane_button("ps-pane-build", "Build Settings", Pane::BuildSettings, pane)
                            .on_click(cx.listener(|this, _, window, cx| this.set_pane(Pane::BuildSettings, window, cx))),
                    ),
            )
    }

    fn render_general(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut body = v_flex().gap_2();
        if let Some(project) = self.project().filter(|_| self.selection.target.is_none()) {
            let configurations = project
                .configurations
                .iter()
                .map(|c| match &c.base_configuration {
                    Some(base) => format!("{} (based on {base})", c.name),
                    None => c.name.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let mut info = vec![
                ("Configurations", configurations),
                ("Default Configuration", project.default_configuration.clone().unwrap_or_default()),
                ("Targets", project.targets.iter().map(|t| t.name.clone()).collect::<Vec<_>>().join(", ")),
            ];
            if let Some(region) = &project.development_region {
                info.push(("Development Region", region.clone()));
            }
            if !project.known_regions.is_empty() {
                info.push(("Localizations", project.known_regions.join(", ")));
            }
            body = body.children(info.into_iter().map(|(label, value)| static_row(label, value, cx)));
            body = body.child(div().h(px(1.)).my_1().bg(theme.border));
        }
        if let Some(t) = self.selection.target
            && let Some(target) = self.project().and_then(|p| p.targets.get(t))
        {
            body = body.child(static_row("Product Type", target.product_type.clone().unwrap_or_else(|| target.isa.clone()), cx));
            let configs = target.configurations.iter().map(|c| c.name.clone()).collect::<Vec<_>>().join(", ");
            body = body.child(static_row("Configurations", configs, cx));
            body = body.child(div().h(px(1.)).my_1().bg(theme.border));
        }
        body = body.children(self.fields.iter().map(|field| field_row(field, None, &self.places, cx)));
        body = body.child(
            div()
                .pt_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Changes apply to every configuration. Leave a field empty to inherit the default."),
        );
        div()
            .id("ps-general")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .child(body)
            .into_any_element()
    }

    fn render_build_settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let configs = self.selected_configurations().to_vec();
        let filter = self.filter.read(cx).value().to_lowercase();
        let base = configs.get(self.configuration).and_then(|c| c.base_configuration.clone());

        let toolbar = h_flex()
            .px_4()
            .py_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .children(configs.iter().enumerate().map(|(ix, config)| {
                Button::new(("ps-config", ix))
                    .label(config.name.clone())
                    .xsmall()
                    .ghost()
                    .selected(ix == self.configuration)
                    .on_click(cx.listener(move |this, _, window, cx| this.set_configuration(ix, window, cx)))
            }))
            .child(div().flex_1())
            .child(div().w(px(220.)).child(Input::new(&self.filter).small().cleanable(true)));

        let mut rows = v_flex().gap_1();
        if let Some(base) = base {
            rows = rows.child(static_row("Based on", base, cx));
        }
        let visible: Vec<&Field> = self
            .fields
            .iter()
            .filter(|f| filter.is_empty() || f.key.to_lowercase().contains(&filter) || f.original.as_deref().unwrap_or("").to_lowercase().contains(&filter))
            .collect();
        if visible.is_empty() {
            rows = rows.child(
                div().py_2().text_sm().text_color(theme.muted_foreground).child(if self.fields.is_empty() {
                    "No build settings at this level."
                } else {
                    "No matching settings."
                }),
            );
        }
        for field in visible {
            let key = field.key.clone();
            let remove = Button::new(SharedString::from(format!("ps-remove-{key}")))
                .icon(IconName::X)
                .xsmall()
                .ghost()
                .tooltip("Remove Setting")
                .on_click(cx.listener(move |this, _, window, cx| this.remove_setting(key.clone(), window, cx)));
            rows = rows.child(field_row(field, Some(remove.into_any_element()), &self.places, cx));
        }

        // Settings the target inherits from the project, read-only.
        if self.selection.target.is_some()
            && let Some(project) = self.project()
            && let Some(name) = configs.get(self.configuration).map(|c| c.name.clone())
            && let Some(project_config) = project.configurations.iter().find(|c| c.name == name)
        {
            let inherited: Vec<_> = project_config
                .settings
                .iter()
                .filter(|s| !self.fields.iter().any(|f| f.key == s.key))
                .filter(|s| filter.is_empty() || s.key.to_lowercase().contains(&filter) || s.value.to_lowercase().contains(&filter))
                .cloned()
                .collect();
            if !inherited.is_empty() {
                rows = rows.child(
                    div()
                        .pt_4()
                        .pb_1()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted_foreground)
                        .child(format!("INHERITED FROM PROJECT “{}”", project.name)),
                );
                rows = rows.children(inherited.into_iter().map(|s| static_row_mono(s.key, s.value, cx)));
            }
        }

        let add = h_flex()
            .px_4()
            .py_2()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .child(div().w(px(260.)).child(Input::new(&self.new_key).small()))
            .child(div().flex_1().child(Input::new(&self.new_value).small()))
            .child(
                Button::new("ps-add-setting")
                    .icon(IconName::Plus)
                    .label("Add")
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.add_setting(window, cx))),
            );

        v_flex()
            .size_full()
            .child(toolbar)
            .child(div().id("ps-build-settings").flex_1().min_h_0().overflow_y_scroll().px_4().py_2().child(rows))
            .when(!configs.is_empty(), |el| el.child(add))
            .into_any_element()
    }
}

fn target_icon(target: &xcodeproj::Target) -> IconName {
    match target.kind_label() {
        "Application" | "Watch App" => IconName::AppWindow,
        "Unit Tests" | "UI Tests" => IconName::FlaskConical,
        "Framework" | "Static Library" | "Dynamic Library" => IconName::Library,
        "App Extension" | "Extension" => IconName::Puzzle,
        "Command Line Tool" => IconName::SquareTerminal,
        _ => IconName::Box,
    }
}

const LABEL_WIDTH: f32 = 220.;

fn static_row(label: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_3()
        .text_sm()
        .child(div().w(px(LABEL_WIDTH)).flex_none().text_right().text_color(cx.theme().muted_foreground).child(label.into()))
        .child(div().flex_1().min_w_0().child(value.into()))
}

fn static_row_mono(label: String, value: String, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .gap_3()
        .py(px(3.))
        .text_xs()
        .font_family(theme.mono_font_family.clone())
        .text_color(theme.muted_foreground)
        .child(div().w(px(LABEL_WIDTH)).flex_none().truncate().child(label))
        .child(div().flex_1().min_w_0().truncate().child(value))
}

fn field_row(field: &Field, suffix: Option<AnyElement>, places: &Places, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let mono = field.label.as_ref() == field.key;
    h_flex()
        .gap_3()
        .text_sm()
        .child(
            div()
                .w(px(LABEL_WIDTH))
                .flex_none()
                .truncate()
                .when(mono, |el| el.text_xs().font_family(theme.mono_font_family.clone()))
                .when(!mono, |el| el.text_right().text_color(theme.muted_foreground))
                .child(field.label.clone()),
        )
        .child(div().flex_1().min_w_0().relative().child(Input::new(&field.input).small()).child(place(places, format!("field:{}", field.key))))
        .when_some(suffix, |el, suffix| el.child(suffix))
}

impl Render for ProjectSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Focus moving redraws the window, this view included.
        self.follow_editing(window, cx);
        let theme = cx.theme().clone();
        let load_error = self.projects.get(self.selection.project).and_then(|p| p.as_ref().err()).cloned();
        let content = if let Some(err) = load_error {
            v_flex()
                .p_4()
                .gap_2()
                .text_sm()
                .child(div().text_color(theme.danger).child("Couldn’t read the project file."))
                .child(div().text_color(theme.muted_foreground).child(err))
                .into_any_element()
        } else {
            match self.pane {
                Pane::General => self.render_general(cx),
                Pane::Signing => self.render_signing(cx),
                Pane::Info => self.render_info(cx),
                Pane::BuildSettings => self.render_build_settings(cx),
            }
        };
        h_flex()
            .id("project-settings")
            .key_context("ProjectSettings")
            .track_focus(&self.focus_handle)
            // Ahead of the Info tab's property list editor, whose own undo
            // would only reach the Info.plist's text.
            .capture_action(cx.listener(|this, _: &Undo, window, cx| {
                this.undo(window, cx).ok();
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &Redo, window, cx| {
                this.redo(window, cx).ok();
                cx.stop_propagation();
            }))
            .size_full()
            .bg(theme.background)
            .child(self.render_sidebar(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_header(cx))
                    .when_some(self.error.clone(), |el, err| {
                        el.child(
                            div()
                                .px_4()
                                .py_1()
                                .text_sm()
                                .bg(theme.danger.opacity(0.12))
                                .text_color(theme.danger)
                                .child(err),
                        )
                    })
                    .child(div().flex_1().min_h_0().child(content)),
            )
    }
}

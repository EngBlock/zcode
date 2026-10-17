//! Product ▸ Profile (⌘I) in the window: Instruments' template chooser,
//! Profile… (always asks), Profile Without Building and Build For ▸
//! Profiling, and the recording's state for the control channel and MCP.
//! The building, recording and summary are `crate::build::profile`'s.

use gpui_kit::assets::IconName;
use gpui_kit::*;
use serde_json::{Value, json};

use super::{DebugTab, Workspace};
use crate::actions::{BuildForProfiling, Profile, ProfileWithOptions, ProfileWithoutBuilding};
use crate::build::profile::{ProfileRequest, Template, Templates};
use crate::fuzzy::fuzzy_match;
use crate::picker::{Picker, PickerDelegate, PickerItem};

impl Workspace {
    pub(super) fn profile(&mut self, _: &Profile, window: &mut Window, cx: &mut Context<Self>) {
        self.start_profile(false, true, window, cx);
    }

    pub(super) fn profile_with_options(&mut self, _: &ProfileWithOptions, window: &mut Window, cx: &mut Context<Self>) {
        self.start_profile(true, true, window, cx);
    }

    pub(super) fn profile_without_building(&mut self, _: &ProfileWithoutBuilding, window: &mut Window, cx: &mut Context<Self>) {
        self.start_profile(false, false, window, cx);
    }

    pub(super) fn build_for_profiling(&mut self, _: &BuildForProfiling, window: &mut Window, cx: &mut Context<Self>) {
        self.save_all(window, cx);
        self.index_refresh = None;
        self.build.update(cx, |b, cx| b.profile(ProfileRequest { template: None, build: true, time_limit: None }, cx));
        cx.notify();
    }

    /// Records with the scheme's instrument, or asks for a template first
    /// (always when `choose`).
    fn start_profile(&mut self, choose: bool, build: bool, window: &mut Window, cx: &mut Context<Self>) {
        let saved = if choose { None } else { self.build.update(cx, |b, _| b.scheme_template()) };
        match saved {
            Some(template) => self.record_profile(template, build, window, cx),
            None => {
                self.profile_chooser = Some(build);
                self.build.update(cx, |b, cx| b.load_templates(cx));
                self.show_template_chooser(window, cx);
            }
        }
    }

    /// The chooser, once the templates are listed.
    pub(super) fn show_template_chooser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(build) = self.profile_chooser else { return };
        let templates = match self.build.read(cx).templates.clone() {
            Templates::Loaded(templates) => templates,
            Templates::Failed(error) => {
                self.profile_chooser = None;
                self.build.update(cx, |b, cx| b.profile_failed(format!("Could not list Instruments' templates: {error}"), cx));
                return;
            }
            Templates::Unknown | Templates::Loading => return,
        };
        self.profile_chooser = None;
        let last = self.build.read(cx).last_template.clone();
        let delegate = TemplateDelegate::new(cx.weak_entity(), templates, last, build);
        let picker = cx.new(|cx| Picker::new(delegate, window, cx));
        self.show_modal(picker, window, cx);
    }

    /// Builds (unless `build` is off) and records with `template`, for
    /// `profiling.time_limit` seconds or until Stop.
    pub(super) fn record_profile(&mut self, template: String, build: bool, window: &mut Window, cx: &mut Context<Self>) {
        let time_limit = self.latest_settings(cx).profiling_time_limit;
        self.start_recording(ProfileRequest { template: Some(template), build, time_limit }, window, cx);
    }

    pub(super) fn start_recording(&mut self, request: ProfileRequest, window: &mut Window, cx: &mut Context<Self>) {
        if request.build {
            // As for Run: edited files are saved, the build refreshes the index.
            self.save_all(window, cx);
            self.index_refresh = None;
        }
        self.show_debug_area = true;
        self.debug_tab = DebugTab::Console;
        self.build.update(cx, |b, cx| b.profile(request, cx));
        cx.notify();
    }

    /// `dump`'s `profile`: the templates, the recording and how the last
    /// one ended.
    pub(super) fn profile_summary(&self, cx: &App) -> Value {
        let build = self.build.read(cx);
        let templates = match &build.templates {
            Templates::Loaded(templates) => json!(templates.iter().map(|t| t.name.clone()).collect::<Vec<_>>()),
            Templates::Failed(error) => json!({ "error": error }),
            Templates::Loading => json!("loading"),
            Templates::Unknown => Value::Null,
        };
        json!({
            "templates": templates,
            "recording": build.recording.as_ref().map(|r| json!({
                "template": r.template,
                "trace": r.trace,
                "stopping": r.stopping,
            })),
            "last": build.last_profile.as_ref().map(|o| json!({
                "template": o.template,
                "trace": o.trace,
                "opened": o.opened,
                "error": o.error,
                "summary": o.summary.as_ref().map(|s| json!({
                    "table": s.table,
                    "process": s.process,
                    "samples": s.samples,
                    "top": s.functions.iter().take(10).map(|f| json!({
                        "function": f.name,
                        "binary": f.binary,
                        "self_weight": f.self_weight,
                        "total_weight": f.total_weight,
                    })).collect::<Vec<_>>(),
                })),
            })),
        })
    }
}

/// Instruments' template chooser: the common templates first, the last one
/// used selected.
pub struct TemplateDelegate {
    workspace: WeakEntity<Workspace>,
    templates: Vec<Template>,
    names: Vec<String>,
    last: Option<String>,
    build: bool,
    items: Vec<PickerItem>,
    matches: Vec<usize>,
}

impl TemplateDelegate {
    fn new(workspace: WeakEntity<Workspace>, templates: Vec<Template>, last: Option<String>, build: bool) -> Self {
        let names = templates.iter().map(|t| t.name.clone()).collect();
        Self { workspace, templates, names, last, build, items: vec![], matches: vec![] }
    }
}

impl PickerDelegate for TemplateDelegate {
    fn placeholder(&self) -> SharedString {
        if self.build { "Choose a Template for Profiling" } else { "Choose a Template (Profile Without Building)" }.into()
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn empty_message(&self) -> SharedString {
        "No templates".into()
    }

    fn initial_selection(&self) -> usize {
        self.matches
            .iter()
            .position(|&ix| Some(&self.names[ix]) == self.last.as_ref())
            .unwrap_or(0)
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        let found = fuzzy_match(&query, &self.names, false, self.names.len());
        self.matches = found.iter().map(|m| m.index).collect();
        self.items = found
            .into_iter()
            .map(|m| PickerItem {
                label: self.names[m.index].clone().into(),
                detail: Some(format!("{} template", self.templates[m.index].section).into()),
                icon: Some(IconName::LayoutTemplate),
                positions: m.positions,
                checked: self.last.as_ref() == Some(&self.names[m.index]),
                ..Default::default()
            })
            .collect();
    }

    fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let Some(&ix) = self.matches.get(index) else { return false };
        let template = self.names[ix].clone();
        let build = self.build;
        let workspace = self.workspace.clone();
        // After the picker has closed: recording opens the debug area.
        window.defer(cx, move |window, cx| {
            workspace.update(cx, |ws, cx| ws.record_profile(template, build, window, cx)).ok();
        });
        true
    }
}

//! A target's Signing & Capabilities tab: automatic signing, the team,
//! bundle identifier, signing certificate and provisioning profile (build
//! settings, with the teams, identities and profiles found on this Mac), and
//! the capabilities list. Capabilities are edited in the target's
//! entitlements file (created and set as `CODE_SIGN_ENTITLEMENTS` the first
//! time), its Info.plist (Background Modes) or build settings (App Sandbox,
//! Hardened Runtime). Nothing talks to Apple's developer portal; builds use
//! `-allowProvisioningUpdates` for that.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::info::expand_path;
use super::{LABEL_WIDTH, Pane, ProjectSettings};
use crate::plist::{self, Format, Node, Value};
use crate::signing::Catalog;
use crate::signing::capabilities::{self as caps, Access, CAPABILITIES, Control, Edit, Id, Platform, State, Store};
use crate::xcodeproj::{self, files::ProjectFiles};

/// A property list file of the target's (its entitlements or Info.plist).
#[derive(Clone)]
struct TargetFile {
    path: PathBuf,
    /// Whether the target already points at it (and it exists).
    wired: bool,
    format: Format,
    text: Option<String>,
    root: Option<Node>,
}

impl TargetFile {
    fn read(path: PathBuf, wired: bool) -> Self {
        let loaded = std::fs::read(&path).ok().and_then(|b| plist::load(b).ok());
        let (format, text) = match loaded {
            Some((format, text)) => (format, Some(text)),
            None => (Format::Xml, None),
        };
        let root = text.as_deref().and_then(|t| plist::parse_xml(t).ok());
        Self { path, wired: wired && text.is_some(), format, text, root }
    }
}

#[derive(Default)]
pub(super) struct SigningTab {
    /// Teams, identities and profiles, read in the background when the tab
    /// is first shown.
    catalog: Option<Arc<Catalog>>,
    loading: Option<Task<()>>,
    team_input: Option<Entity<InputState>>,
    bundle_input: Option<Entity<InputState>>,
    /// The "add" field of each list on a capability's card.
    list_inputs: Vec<(Store, Entity<InputState>)>,
    entitlements: Option<TargetFile>,
    info: Option<TargetFile>,
    _subscriptions: Vec<Subscription>,
}

const CERTIFICATES: &[(&str, &str)] = &[("Apple Development", "Development"), ("Apple Distribution", "Distribution")];
const SIGN_LOCALLY: &str = "-";

impl ProjectSettings {
    fn platforms(&self) -> Vec<Platform> {
        caps::platforms(self.effective_value("SDKROOT").as_deref(), self.effective_value("SUPPORTED_PLATFORMS").as_deref())
    }

    fn state<'a>(&'a self, setting: &'a dyn Fn(&str) -> Option<String>, platforms: &'a [Platform]) -> State<'a> {
        State {
            entitlements: self.signing.entitlements.as_ref().and_then(|f| f.root.as_ref()),
            info: self.signing.info.as_ref().and_then(|f| f.root.as_ref()),
            setting,
            platforms,
        }
    }

    /// Runs `f` with the target's capability state.
    fn with_state<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        let setting = |key: &str| self.effective_value(key);
        let platforms = self.platforms();
        f(&self.state(&setting, &platforms))
    }

    fn bundle_id(&self) -> String {
        self.effective_value("PRODUCT_BUNDLE_IDENTIFIER").unwrap_or_default()
    }

    /// The entitlements file `CODE_SIGN_ENTITLEMENTS` names, or where Xcode
    /// would create one (`Target/Target.entitlements`).
    fn entitlements_file(&self) -> Option<TargetFile> {
        let (_, files) = self.project_files()?;
        let name = self.target_name()?;
        Some(match self.target_value("CODE_SIGN_ENTITLEMENTS").filter(|v| !v.is_empty()) {
            Some(value) => TargetFile::read(expand_path(&value, &files.project_dir, &name), true),
            None => TargetFile::read(files.target_folder(&self.target_id()?).join(format!("{name}.entitlements")), false),
        })
    }

    pub(super) fn rebuild_signing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.signing._subscriptions.clear();
        self.signing.list_inputs.clear();
        if self.pane != Pane::Signing || self.selection.target.is_none() {
            self.signing.team_input = None;
            self.signing.bundle_input = None;
            return;
        }
        if self.signing.catalog.is_none() && self.signing.loading.is_none() {
            self.signing.loading = Some(cx.spawn(async move |this, cx| {
                let catalog = cx.background_spawn(async move { Catalog::load() }).await;
                this.update(cx, |this, cx| {
                    this.signing.catalog = Some(Arc::new(catalog));
                    this.signing.loading = None;
                    cx.notify();
                })
                .ok();
            }));
        }
        self.signing.entitlements = self.entitlements_file();
        self.signing.info = self.info_plist_path().map(|(path, exists)| TargetFile::read(path, exists && self.target_value("INFOPLIST_FILE").is_some()));

        let field = |this: &mut Self, key: &'static str, value: Option<String>, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_string()).default_value(value.unwrap_or_default()));
            this.signing._subscriptions.push(cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let value = input.read(cx).value().trim().to_string();
                    if this.target_value(key).unwrap_or_default() != value {
                        let configs = this.configuration_ids();
                        this.write(key, (!value.is_empty()).then_some(value.as_str()), &configs, false, cx);
                    }
                }
            }));
            input
        };
        self.signing.team_input = Some(field(self, "DEVELOPMENT_TEAM", self.effective_value("DEVELOPMENT_TEAM"), "Team ID", window, cx));
        self.signing.bundle_input = Some(field(self, "PRODUCT_BUNDLE_IDENTIFIER", self.target_value("PRODUCT_BUNDLE_IDENTIFIER"), "com.example.App", window, cx));

        // An "add" field for every list on the cards shown.
        let lists: Vec<(Store, String)> = self.with_state(|state| {
            CAPABILITIES
                .iter()
                .filter(|c| caps::is_enabled(c.id, state))
                .flat_map(|c| caps::controls(c.id, state))
                .flat_map(|s| s.controls)
                .filter_map(|control| match control {
                    Control::List { store, placeholder } => Some((store, placeholder)),
                    _ => None,
                })
                .collect()
        });
        for (store, placeholder) in lists {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            self.signing._subscriptions.push(cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let item = input.read(cx).value().trim().to_string();
                    this.add_list_item(store, &item, window, cx);
                }
            }));
            self.signing.list_inputs.push((store, input));
        }
    }

    // ------------------------------------------------------------------
    // Edits

    /// Makes capability edits: entitlements and Info.plist keys in their
    /// files (creating and wiring them up as needed), then build settings.
    fn apply_edits(&mut self, edits: Vec<Edit>, window: &mut Window, cx: &mut Context<Self>) {
        let (mut entitlements, mut info, mut settings) = (vec![], vec![], vec![]);
        for edit in edits {
            match edit {
                Edit::Entitlement(key, value) => entitlements.push((key, value)),
                Edit::Info(key, value) => info.push((key, value)),
                Edit::Setting(key, value) => settings.push((key, value)),
            }
        }
        if !entitlements.is_empty()
            && let Some(file) = self.signing.entitlements.clone()
            && let Some(written) = self.edit_target_file(&file, &entitlements, cx)
            && !file.wired
        {
            let configs = self.configuration_ids();
            let target = self.target_id().unwrap_or_default();
            self.edit_project("Couldn’t add the entitlements file to the project", cx, move |xcodeproj, text| {
                let text = xcodeproj::files::add_target_file(xcodeproj, &text, &written, &target, false)?.unwrap_or(text);
                let project_dir = ProjectFiles::parse(xcodeproj, &text)?.project_dir;
                let relative = crate::paths::relative_path(&project_dir, &written).to_string_lossy().to_string();
                let mut text = text;
                for id in &configs {
                    text = xcodeproj::set_build_setting(&text, id, "CODE_SIGN_ENTITLEMENTS", Some(&relative), false)?;
                }
                Ok(text)
            });
        }
        if !info.is_empty()
            && let Some(file) = self.signing.info.clone()
            && let Some(written) = self.edit_target_file(&file, &info, cx)
            && !file.wired
            && let Some(target) = self.target_id()
        {
            self.wire_info_plist(&written, &target, cx);
        }
        if !settings.is_empty() {
            let configs = self.configuration_ids();
            // Off means unset, unless the project turns it on for every target.
            let settings: Vec<(String, Option<String>)> = settings
                .into_iter()
                .map(|(key, value)| match value {
                    None if self.project_value(&key).is_some_and(|v| v.eq_ignore_ascii_case("YES")) => (key, Some("NO".to_string())),
                    other => (key, other),
                })
                .collect();
            self.edit_project("Couldn’t change the build settings", cx, move |_, mut text| {
                for (key, value) in &settings {
                    for id in &configs {
                        text = xcodeproj::set_build_setting(&text, id, key, value.as_deref(), false)?;
                    }
                }
                Ok(text)
            });
        }
        self.rebuild_signing(window, cx);
        self.keep_focus(window, cx);
        cx.notify();
    }

    /// Sets keys in one of the target's property lists, creating it from an
    /// empty dictionary when needed. Returns the path written.
    fn edit_target_file(&mut self, file: &TargetFile, keys: &[(String, Option<Value>)], cx: &mut Context<Self>) -> Option<PathBuf> {
        let mut text = file.text.clone().unwrap_or_else(|| plist::to_xml(&Value::Dict(vec![])));
        for (key, value) in keys {
            match plist::edit::set_top_level(&text, key, value.as_ref()) {
                Ok(Some(new)) => text = new,
                Ok(None) => {}
                Err(err) => {
                    self.error = Some(format!("Couldn’t edit {}: {err:#}", file.path.display()));
                    cx.notify();
                    return None;
                }
            }
        }
        if file.text.as_deref() == Some(text.as_str()) && file.wired {
            return Some(file.path.clone());
        }
        let result = plist::encode(&text, file.format).map_err(|e| format!("{e:#}")).and_then(|bytes| {
            if let Some(dir) = file.path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&file.path, bytes).map_err(|e| e.to_string())
        });
        match result {
            Ok(()) => {
                cx.emit(super::ProjectSettingsEvent::Changed);
                Some(file.path.clone())
            }
            Err(err) => {
                self.error = Some(format!("Couldn’t save {}: {err}", file.path.display()));
                cx.notify();
                None
            }
        }
    }

    fn add_capability(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let bundle = self.bundle_id();
        let edits = self.with_state(|state| if caps::is_enabled(id, state) { vec![] } else { caps::add(id, &bundle, state) });
        self.apply_edits(edits, window, cx);
    }

    fn remove_capability(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let edits = self.with_state(|state| caps::remove(id, state));
        self.apply_edits(edits, window, cx);
    }

    fn toggle_control(&mut self, control: Control, window: &mut Window, cx: &mut Context<Self>) {
        let edits = self.with_state(|state| caps::toggle(&control, state));
        self.apply_edits(edits, window, cx);
    }

    fn set_access(&mut self, store: Store, access: Access, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_edits(caps::set_access(store, access), window, cx);
    }

    fn add_list_item(&mut self, store: Store, item: &str, window: &mut Window, cx: &mut Context<Self>) {
        let edits = self.with_state(|state| caps::edit_list(store, item, false, state));
        self.apply_edits(edits, window, cx);
    }

    fn remove_list_item(&mut self, store: Store, item: &str, window: &mut Window, cx: &mut Context<Self>) {
        let edits = self.with_state(|state| caps::edit_list(store, item, true, state));
        self.apply_edits(edits, window, cx);
    }

    fn set_target_setting(&mut self, key: &str, value: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let configs = self.configuration_ids();
        self.write(key, value, &configs, false, cx);
        self.rebuild_signing(window, cx);
        self.keep_focus(window, cx);
    }

    fn automatic(&self) -> bool {
        self.effective_value("CODE_SIGN_STYLE").is_none_or(|s| s != "Manual")
    }

    fn mac(&self) -> bool {
        self.platforms().contains(&Platform::MacOS)
    }

    /// What's wrong with signing as set up, as Xcode words it.
    fn signing_status(&self) -> Option<String> {
        let name = self.target_name().unwrap_or_default();
        let identity = self.effective_value("CODE_SIGN_IDENTITY");
        let team = self.effective_value("DEVELOPMENT_TEAM").unwrap_or_default();
        if identity.as_deref() == Some(SIGN_LOCALLY) {
            return None;
        }
        if team.is_empty() {
            return Some(format!("Signing for “{name}” requires a development team. Select a development team in the Signing & Capabilities editor."));
        }
        if !self.automatic() && self.effective_value("PROVISIONING_PROFILE_SPECIFIER").is_none_or(|p| p.is_empty()) && !self.mac() {
            return Some(format!("“{name}” requires a provisioning profile. Select a provisioning profile in the Signing & Capabilities editor."));
        }
        None
    }

    // ------------------------------------------------------------------
    // The debug control channel

    pub(super) fn signing_summary(&self, _: &App) -> Option<serde_json::Value> {
        if self.pane != Pane::Signing || self.selection.target.is_none() {
            return None;
        }
        let catalog = self.signing.catalog.clone();
        let (enabled, available) = self.with_state(|state| {
            let platforms = self.platforms();
            let enabled: Vec<&str> = CAPABILITIES.iter().filter(|c| caps::is_enabled(c.id, state)).map(|c| c.name).collect();
            let available: Vec<&str> = CAPABILITIES
                .iter()
                .filter(|c| c.available_on(&platforms) && !caps::is_enabled(c.id, state))
                .map(|c| c.name)
                .collect();
            (enabled, available)
        });
        Some(serde_json::json!({
            "loaded": catalog.is_some(),
            "automatic": self.automatic(),
            "team": self.effective_value("DEVELOPMENT_TEAM"),
            "teams": catalog.as_ref().map(|c| c.teams.iter().map(|t| t.label()).collect::<Vec<_>>()),
            "identities": catalog.as_ref().map(|c| c.identities.len()),
            "profiles": catalog.as_ref().map(|c| c.profiles.iter().map(|p| p.name.clone()).collect::<Vec<_>>()),
            "bundle": self.bundle_id(),
            "certificate": self.effective_value("CODE_SIGN_IDENTITY"),
            "profile": self.effective_value("PROVISIONING_PROFILE_SPECIFIER"),
            "status": self.signing_status(),
            "capabilities": enabled,
            "available": available,
            "entitlements": self.signing.entitlements.as_ref().map(|f| f.path.to_string_lossy().to_string()),
        }))
    }

    /// Finds a control by its capability and label (`App Sandbox/Camera`).
    fn find_control(&self, spec: &str) -> Result<Control, String> {
        let (name, label) = spec.split_once('/').ok_or("usage: <Capability>/<option>")?;
        let capability = CAPABILITIES.iter().find(|c| c.name == name).ok_or(format!("no capability {name}"))?;
        self.with_state(|state| {
            caps::controls(capability.id, state)
                .into_iter()
                .flat_map(|s| s.controls)
                .find(|c| match c {
                    Control::Check { label: l, .. } | Control::Member { label: l, .. } | Control::Access { label: l, .. } => *l == label,
                    Control::List { .. } => label == "list",
                })
                .ok_or(format!("no option {label} in {name}"))
        })
    }

    /// `add <Capability>`, `remove <Capability>`, `toggle <Capability>/<option>`,
    /// `access <Capability>/<option> none|readonly|readwrite`, `list-add
    /// <Capability> <item>`, `list-remove <Capability> <item>`, `automatic
    /// on|off`, `team <id>`, `certificate <identity>|sign-locally|none`,
    /// `profile <name>|none`, `wait` (for the teams and profiles).
    pub fn signing_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        if self.pane != Pane::Signing {
            return Err("the Signing & Capabilities tab isn’t shown".into());
        }
        let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
        let capability = |name: &str| CAPABILITIES.iter().find(|c| c.name == name).map(|c| c.id).ok_or(format!("no capability {name}"));
        self.error = None;
        match verb {
            "add" => self.add_capability(capability(arg)?, window, cx),
            "remove" => self.remove_capability(capability(arg)?, window, cx),
            "toggle" => {
                let control = self.find_control(arg)?;
                self.toggle_control(control, window, cx);
            }
            "access" => {
                let (spec, level) = arg.rsplit_once(' ').ok_or("usage: access <Capability>/<option> none|readonly|readwrite")?;
                let Control::Access { store, .. } = self.find_control(spec)? else { return Err("not a file access option".into()) };
                let access = match level {
                    "readonly" => Access::ReadOnly,
                    "readwrite" => Access::ReadWrite,
                    _ => Access::None,
                };
                self.set_access(store, access, window, cx);
            }
            "list-add" | "list-remove" => {
                let names: Vec<&str> = CAPABILITIES.iter().map(|c| c.name).collect();
                let name = names.iter().find(|n| arg.starts_with(&format!("{n} "))).ok_or("usage: list-add <Capability> <item>")?;
                let item = arg[name.len() + 1..].trim().to_string();
                let Control::List { store, .. } = self.find_control(&format!("{name}/list"))? else { unreachable!() };
                if verb == "list-add" {
                    self.add_list_item(store, &item, window, cx);
                } else {
                    self.remove_list_item(store, &item, window, cx);
                }
            }
            "automatic" => self.set_target_setting("CODE_SIGN_STYLE", Some(if arg == "on" { "Automatic" } else { "Manual" }), window, cx),
            "team" => self.set_target_setting("DEVELOPMENT_TEAM", (!arg.is_empty()).then_some(arg), window, cx),
            "certificate" => {
                let value = match arg {
                    "sign-locally" => Some(SIGN_LOCALLY),
                    "none" | "" => None,
                    other => Some(other),
                };
                self.set_target_setting("CODE_SIGN_IDENTITY", value, window, cx);
            }
            "profile" => self.set_target_setting("PROVISIONING_PROFILE_SPECIFIER", (arg != "none" && !arg.is_empty()).then_some(arg), window, cx),
            "wait" => {
                if self.signing.catalog.is_none() {
                    return Err("still reading teams and profiles".into());
                }
            }
            other => return Err(format!("unknown signing command {other}")),
        }
        match &self.error {
            Some(err) => Err(err.clone()),
            None => Ok(()),
        }
    }

    // ------------------------------------------------------------------
    // Rendering

    fn render_signing_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let automatic = self.automatic();
        let catalog = self.signing.catalog.clone();
        let team = self.effective_value("DEVELOPMENT_TEAM").unwrap_or_default();
        let bundle = self.bundle_id();
        let mac = self.mac();
        let row = |label: &'static str, content: AnyElement| {
            h_flex()
                .gap_3()
                .text_sm()
                .child(div().w(px(LABEL_WIDTH)).flex_none().text_right().text_color(theme.muted_foreground).child(label))
                .child(div().flex_1().min_w_0().child(content))
        };
        let this = cx.weak_entity();

        let team_label = match catalog.as_ref().and_then(|c| c.team(&team)) {
            Some(t) => t.label(),
            None if team.is_empty() => "None".into(),
            None => team.clone(),
        };
        let teams = catalog.as_ref().map(|c| c.teams.clone()).unwrap_or_default();
        let team_menu = {
            let this = this.clone();
            let current = team.clone();
            Button::new("ps-signing-team")
                .label(team_label)
                .small()
                .outline()
                .loading(catalog.is_none())
                .dropdown_menu(move |mut menu: PopupMenu, _, _| {
                    for t in &teams {
                        let (this, id) = (this.clone(), t.id.clone());
                        menu = menu.item(PopupMenuItem::new(t.label()).checked(t.id == current).on_click(move |_, window, cx| {
                            this.update(cx, |this, cx| this.set_target_setting("DEVELOPMENT_TEAM", Some(&id), window, cx)).ok();
                        }));
                    }
                    if teams.is_empty() {
                        menu = menu.label("No teams found: enter a team ID");
                    }
                    let this = this.clone();
                    menu.separator().item(PopupMenuItem::new("None").checked(current.is_empty()).on_click(move |_, window, cx| {
                        this.update(cx, |this, cx| this.set_target_setting("DEVELOPMENT_TEAM", None, window, cx)).ok();
                    }))
                })
        };
        let team_row = h_flex()
            .gap_2()
            .child(team_menu)
            .when_some(self.signing.team_input.clone(), |el, input| el.child(div().w(px(140.)).child(Input::new(&input).small())));

        // The certificate: Development, Distribution or (on the Mac) Sign to
        // Run Locally; manual signing also lists the keychain's identities.
        let identity = self.effective_value("CODE_SIGN_IDENTITY");
        let identity_label = match identity.as_deref() {
            Some(SIGN_LOCALLY) => "Sign to Run Locally".to_string(),
            Some(name) => CERTIFICATES.iter().find(|(v, _)| *v == name).map_or(name.to_string(), |(_, l)| l.to_string()),
            None => "Development".to_string(),
        };
        let mut choices: Vec<(String, Option<String>)> = CERTIFICATES.iter().map(|(v, l)| (l.to_string(), Some(v.to_string()))).collect();
        if mac {
            choices.push(("Sign to Run Locally".into(), Some(SIGN_LOCALLY.into())));
        }
        if !automatic && let Some(catalog) = &catalog {
            choices.extend(catalog.identities.iter().map(|i| (i.name.clone(), Some(i.name.clone()))));
        }
        let certificate = {
            let this = this.clone();
            let current = identity.clone();
            Button::new("ps-signing-certificate").label(identity_label).small().outline().dropdown_menu(move |mut menu: PopupMenu, _, _| {
                for (label, value) in &choices {
                    let (this, value) = (this.clone(), value.clone());
                    menu = menu.item(PopupMenuItem::new(label.clone()).checked(value == current).on_click(move |_, window, cx| {
                        this.update(cx, |this, cx| this.set_target_setting("CODE_SIGN_IDENTITY", value.as_deref(), window, cx)).ok();
                    }));
                }
                menu
            })
        };

        let profile = if automatic {
            div().text_color(theme.muted_foreground).child("Xcode Managed Profile").into_any_element()
        } else {
            let current = self.effective_value("PROVISIONING_PROFILE_SPECIFIER");
            let profiles: Vec<String> = catalog
                .as_ref()
                .map(|c| c.profiles.iter().filter(|p| p.matches(&team, &bundle) && !p.expired()).map(|p| p.name.clone()).collect())
                .unwrap_or_default();
            let this = this.clone();
            let label = current.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| "None".into());
            Button::new("ps-signing-profile")
                .label(label)
                .small()
                .outline()
                .dropdown_menu(move |mut menu: PopupMenu, _, _| {
                    for name in &profiles {
                        let (this, name) = (this.clone(), name.clone());
                        menu = menu.item(PopupMenuItem::new(name.clone()).checked(current.as_deref() == Some(name.as_str())).on_click(move |_, window, cx| {
                            this.update(cx, |this, cx| this.set_target_setting("PROVISIONING_PROFILE_SPECIFIER", Some(&name), window, cx)).ok();
                        }));
                    }
                    if profiles.is_empty() {
                        menu = menu.label("No matching profiles installed");
                    }
                    let this = this.clone();
                    menu.separator().item(PopupMenuItem::new("None").on_click(move |_, window, cx| {
                        this.update(cx, |this, cx| this.set_target_setting("PROVISIONING_PROFILE_SPECIFIER", None, window, cx)).ok();
                    }))
                })
                .into_any_element()
        };

        let status = self.signing_status();
        v_flex()
            .gap_2()
            .child(section_title("Signing", cx))
            .child(row(
                "",
                Checkbox::new("ps-signing-automatic")
                    .label("Automatically manage signing")
                    .checked(automatic)
                    .on_click(cx.listener(|this, checked: &bool, window, cx| {
                        let style = if *checked { "Automatic" } else { "Manual" };
                        this.set_target_setting("CODE_SIGN_STYLE", Some(style), window, cx);
                    }))
                    .into_any_element(),
            ))
            .child(row("Team", team_row.into_any_element()))
            .when_some(self.signing.bundle_input.clone(), |el, input| el.child(row("Bundle Identifier", Input::new(&input).small().into_any_element())))
            .child(row("Signing Certificate", div().w(px(320.)).child(certificate).into_any_element()))
            .child(row("Provisioning Profile", div().w(px(320.)).child(profile).into_any_element()))
            .when_some(status, |el, status| {
                el.child(row("Status", div().text_color(theme.danger).child(status).into_any_element()))
            })
            .into_any_element()
    }

    fn render_capability(&mut self, id: Id, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let capability = caps::capability(id);
        let sections = self.with_state(|state| {
            caps::controls(id, state)
                .into_iter()
                .map(|section| {
                    let controls: Vec<(Control, bool, Access, Vec<String>)> = section
                        .controls
                        .into_iter()
                        .map(|control| {
                            let on = match &control {
                                Control::Check { store, .. } => state.is_on(*store),
                                Control::Member { store, value, .. } => state.strings(*store).iter().any(|v| v == value),
                                _ => false,
                            };
                            let access = match &control {
                                Control::Access { store, .. } => state.access(*store),
                                _ => Access::None,
                            };
                            let items = match &control {
                                Control::List { store, .. } => state.strings(*store),
                                _ => vec![],
                            };
                            (control, on, access, items)
                        })
                        .collect();
                    (section.title, controls)
                })
                .collect::<Vec<_>>()
        });
        let mut card = v_flex()
            .gap_1()
            .p_3()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).child(capability.name))
                    .child(
                        Button::new(SharedString::from(format!("ps-cap-remove-{}", capability.name)))
                            .icon(IconName::Trash)
                            .xsmall()
                            .ghost()
                            .tooltip("Delete Capability")
                            .on_click(cx.listener(move |this, _, window, cx| this.remove_capability(id, window, cx))),
                    ),
            );
        for (title, controls) in sections {
            if !title.is_empty() {
                card = card.child(div().pt_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child(title));
            }
            for (ix, (control, on, access, items)) in controls.into_iter().enumerate() {
                let key = format!("{}-{title}-{ix}", capability.name);
                match control.clone() {
                    Control::Check { label, .. } | Control::Member { label, .. } => {
                        card = card.child(
                            Checkbox::new(SharedString::from(format!("ps-cap-{key}")))
                                .label(label)
                                .checked(on)
                                .on_click(cx.listener(move |this, _: &bool, window, cx| this.toggle_control(control.clone(), window, cx))),
                        );
                    }
                    Control::Access { label, store } => {
                        let this = cx.weak_entity();
                        card = card.child(
                            h_flex()
                                .gap_2()
                                .text_sm()
                                .child(div().w(px(160.)).child(label))
                                .child(Button::new(SharedString::from(format!("ps-cap-{key}"))).label(access.label()).xsmall().outline().dropdown_menu(
                                    move |mut menu: PopupMenu, _, _| {
                                        for level in Access::ALL {
                                            let this = this.clone();
                                            menu = menu.item(PopupMenuItem::new(level.label()).checked(level == access).on_click(move |_, window, cx| {
                                                this.update(cx, |this, cx| this.set_access(store, level, window, cx)).ok();
                                            }));
                                        }
                                        menu
                                    },
                                )),
                        );
                    }
                    Control::List { store, .. } => {
                        for (item_ix, item) in items.into_iter().enumerate() {
                            let value = item.clone();
                            card = card.child(
                                h_flex()
                                    .gap_2()
                                    .text_sm()
                                    .child(div().flex_1().min_w_0().truncate().font_family(theme.mono_font_family.clone()).child(item))
                                    .child(
                                        Button::new(SharedString::from(format!("ps-cap-{key}-remove-{item_ix}")))
                                            .icon(IconName::Minus)
                                            .xsmall()
                                            .ghost()
                                            .on_click(cx.listener(move |this, _, window, cx| this.remove_list_item(store, &value, window, cx))),
                                    ),
                            );
                        }
                        if let Some(input) = self.signing.list_inputs.iter().find(|(s, _)| *s == store).map(|(_, i)| i.clone()) {
                            let add_input = input.clone();
                            card = card.child(
                                h_flex().gap_2().child(div().flex_1().child(Input::new(&input).small())).child(
                                    Button::new(SharedString::from(format!("ps-cap-{key}-add")))
                                        .icon(IconName::Plus)
                                        .xsmall()
                                        .ghost()
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            let item = add_input.read(cx).value().trim().to_string();
                                            this.add_list_item(store, &item, window, cx);
                                        })),
                                ),
                            );
                        }
                    }
                }
            }
        }
        card.into_any_element()
    }

    pub(super) fn render_signing(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if self.selection.target.is_none() {
            return div().p_4().text_sm().text_color(theme.muted_foreground).child("Select a target.").into_any_element();
        }
        let platforms = self.platforms();
        let (enabled, addable): (Vec<Id>, Vec<(Id, &'static str, &'static str)>) = self.with_state(|state| {
            let enabled = CAPABILITIES.iter().filter(|c| caps::is_enabled(c.id, state)).map(|c| c.id).collect();
            let addable = CAPABILITIES
                .iter()
                .filter(|c| c.available_on(&platforms) && !caps::is_enabled(c.id, state))
                .map(|c| (c.id, c.name, c.summary))
                .collect();
            (enabled, addable)
        });
        let this = cx.weak_entity();
        let add = Button::new("ps-add-capability").icon(IconName::Plus).label("Capability").small().ghost().dropdown_menu(
            move |mut menu: PopupMenu, _, _| {
                for (id, name, _) in &addable {
                    let (this, id) = (this.clone(), *id);
                    menu = menu.item(PopupMenuItem::new(*name).on_click(move |_, window, cx| {
                        this.update(cx, |this, cx| this.add_capability(id, window, cx)).ok();
                    }));
                }
                if addable.is_empty() {
                    menu = menu.label("Every capability zcode can add is on");
                }
                menu
            },
        );
        let entitlements_note = self.signing.entitlements.as_ref().map(|f| {
            let dir = self.project_files().map(|(_, files)| files.project_dir).unwrap_or_default();
            let relative = crate::paths::relative_path(&dir, &f.path).to_string_lossy().to_string();
            if f.wired { format!("Entitlements File: {relative}") } else { format!("Adding a capability with entitlements creates {relative}.") }
        });
        let signing = self.render_signing_section(cx);
        let cards: Vec<AnyElement> = enabled.into_iter().map(|id| self.render_capability(id, cx)).collect();
        div()
            .id("ps-signing")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .child(
                v_flex()
                    .gap_4()
                    .child(signing)
                    .child(
                        v_flex()
                            .gap_2()
                            .child(h_flex().child(div().flex_1().child(section_title("Capabilities", cx))).child(add))
                            .when_some(entitlements_note, |el, note| el.child(div().text_xs().text_color(theme.muted_foreground).child(note)))
                            .when(cards.is_empty(), |el| {
                                el.child(div().text_sm().text_color(theme.muted_foreground).child("No capabilities. Add one with + Capability."))
                            })
                            .children(cards),
                    ),
            )
            .into_any_element()
    }
}

fn section_title(title: &'static str, cx: &App) -> impl IntoElement {
    div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(cx.theme().foreground).child(title)
}

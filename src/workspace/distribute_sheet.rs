//! The Organizer's Distribute App sheet: a method (App Store Connect,
//! TestFlight Internal Only, Release Testing, Enterprise, Debugging for
//! devices; Developer ID and Copy App for the Mac), its options, and where
//! the export goes. App Store Connect and TestFlight upload unless told to
//! export; Developer ID can be notarized with a keychain profile. The
//! export itself is `crate::build::archive`'s.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;

use super::Workspace;
use crate::actions::{PickerConfirm, PickerDismiss};
use crate::build::archive::archives::{self, Archive};
use crate::build::archive::{DistributeRequest, ExportChoices, Method};

pub struct DistributeSheet {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    archive: Archive,
    methods: &'static [Method],
    choices: ExportChoices,
    notarize: bool,
    folder: Entity<InputState>,
    profile: Entity<InputState>,
    /// Where an upload's export goes (zcode's cache).
    upload_dir: PathBuf,
}

impl EventEmitter<DismissEvent> for DistributeSheet {}

impl Focusable for DistributeSheet {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl DistributeSheet {
    pub fn new(workspace: WeakEntity<Workspace>, archive: Archive, notary_profile: String, cache_dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = archive.app.clone().unwrap_or_default();
        let methods = Method::for_app(app.is_mac());
        let now = archives::now_unix();
        let folder = archives::export_folder(&crate::paths::exports_dir(), &archive.name, now);
        let folder = cx.new(|cx| InputState::new(window, cx).placeholder("Folder").default_value(crate::paths::display_path(&folder)));
        let profile = cx.new(|cx| InputState::new(window, cx).placeholder("notarytool keychain profile").default_value(notary_profile));
        let upload_dir = archives::export_folder(&cache_dir.join("Uploads"), &archive.name, now);
        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            choices: ExportChoices::new(methods[0], &app.team),
            archive,
            methods,
            notarize: true,
            folder,
            profile,
            upload_dir,
        }
    }

    fn set_method(&mut self, method: Method, cx: &mut Context<Self>) {
        let team = self.choices.team.clone();
        self.choices = ExportChoices::new(method, &team);
        cx.notify();
    }

    fn uploading(&self) -> bool {
        self.choices.upload && self.choices.method.can_upload()
    }

    fn folder(&self, cx: &App) -> PathBuf {
        let text = self.folder.read(cx).value().trim().to_string();
        match text.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
            None => PathBuf::from(text),
        }
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Export".into()) });
        let name = self.folder(cx).file_name().map(|n| n.to_os_string());
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else { return };
            let Some(dir) = paths.into_iter().next() else { return };
            // The export's own folder inside the one chosen.
            let path = match name {
                Some(name) => dir.join(name),
                None => dir,
            };
            this.update_in(cx, |this, window, cx| {
                this.folder.update(cx, |input, cx| input.set_value(crate::paths::display_path(&path), window, cx));
            })
            .ok();
        })
        .detach();
    }

    pub fn confirm(&mut self, _: &PickerConfirm, window: &mut Window, cx: &mut Context<Self>) {
        let export_path = if self.uploading() { self.upload_dir.clone() } else { self.folder(cx) };
        if export_path.as_os_str().is_empty() {
            return;
        }
        let profile = self.profile.read(cx).value().trim().to_string();
        let notarize = (self.choices.method.can_notarize() && self.notarize && !profile.is_empty()).then(|| profile.clone());
        let request = DistributeRequest { archive: self.archive.clone(), choices: self.choices.clone(), export_path, notarize: notarize.clone() };
        cx.emit(DismissEvent);
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace.update(cx, |ws, cx| ws.distribute(request, window, cx)).ok();
        });
    }

    fn dismiss(&mut self, _: &PickerDismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    /// The control channel's `distribute <command>`: `method <id>`, `upload
    /// on|off`, `symbols on|off`, `versions on|off`, `strip on|off`,
    /// `notarize on|off`, `profile <name>`, `folder <path>`, `confirm`,
    /// `cancel`.
    pub fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
        let on = arg == "on";
        match verb {
            "method" => {
                let method = Method::from_id(arg).filter(|m| self.methods.contains(m)).ok_or_else(|| format!("no method {arg} for this archive"))?;
                self.set_method(method, cx);
            }
            "upload" => self.choices.upload = on,
            "symbols" => self.choices.upload_symbols = on,
            "versions" => self.choices.manage_version = on,
            "strip" => self.choices.strip_swift_symbols = on,
            "notarize" => self.notarize = on,
            "profile" => self.profile.update(cx, |input, cx| input.set_value(arg.to_string(), window, cx)),
            "folder" => self.folder.update(cx, |input, cx| input.set_value(arg.to_string(), window, cx)),
            "confirm" => self.confirm(&PickerConfirm, window, cx),
            "cancel" => self.dismiss(&PickerDismiss, window, cx),
            other => return Err(format!("unknown command {other}")),
        }
        cx.notify();
        Ok(())
    }

    pub fn summary(&self, cx: &App) -> serde_json::Value {
        json!({
            "archive": self.archive.path,
            "methods": self.methods.iter().map(|m| m.id()).collect::<Vec<_>>(),
            "method": self.choices.method.id(),
            "upload": self.uploading(),
            "team": self.choices.team,
            "upload_symbols": self.choices.upload_symbols,
            "manage_version": self.choices.manage_version,
            "strip_swift_symbols": self.choices.strip_swift_symbols,
            "notarize": self.notarize,
            "profile": self.profile.read(cx).value().to_string(),
            "folder": self.folder(cx),
            "export_options": self.choices.plist(),
        })
    }

    fn render_options(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let method = self.choices.method;
        let mut options = v_flex().gap_1p5();
        if method.can_upload() {
            options = options.child(Checkbox::new("distribute-upload").label("Upload to App Store Connect (otherwise export it)").checked(self.choices.upload).on_click(
                cx.listener(|this, checked: &bool, _, cx| {
                    this.choices.upload = *checked;
                    cx.notify();
                }),
            ));
        }
        if method.is_app_store() {
            options = options
                .child(Checkbox::new("distribute-symbols").label("Upload your app’s symbols").checked(self.choices.upload_symbols).on_click(cx.listener(
                    |this, checked: &bool, _, cx| {
                        this.choices.upload_symbols = *checked;
                        cx.notify();
                    },
                )))
                .child(Checkbox::new("distribute-versions").label("Manage version and build number").checked(self.choices.manage_version).on_click(
                    cx.listener(|this, checked: &bool, _, cx| {
                        this.choices.manage_version = *checked;
                        cx.notify();
                    }),
                ));
        }
        if method.is_installable() {
            options = options.child(Checkbox::new("distribute-strip").label("Strip Swift symbols").checked(self.choices.strip_swift_symbols).on_click(
                cx.listener(|this, checked: &bool, _, cx| {
                    this.choices.strip_swift_symbols = *checked;
                    cx.notify();
                }),
            ));
        }
        if method.can_notarize() {
            options = options
                .child(Checkbox::new("distribute-notarize").label("Notarize with Apple, then staple the ticket").checked(self.notarize).on_click(cx.listener(
                    |this, checked: &bool, _, cx| {
                        this.notarize = *checked;
                        cx.notify();
                    },
                )))
                .when(self.notarize, |el| {
                    el.child(
                        h_flex()
                            .gap_2()
                            .child(div().w(px(110.)).text_xs().text_color(theme.muted_foreground).child("Keychain profile"))
                            .child(div().flex_1().child(Input::new(&self.profile).small())),
                    )
                    .child(div().text_xs().text_color(theme.muted_foreground).child("Stored once with `xcrun notarytool store-credentials <name>` in a terminal."))
                });
        }
        if !self.uploading() {
            options = options.child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(110.)).text_xs().text_color(theme.muted_foreground).child("Export to"))
                    .child(div().flex_1().child(Input::new(&self.folder).small()))
                    .child(Button::new("distribute-choose").label("Choose…").xsmall().on_click(cx.listener(|this, _, window, cx| this.choose_folder(window, cx)))),
            );
        }
        options
    }
}

impl Render for DistributeSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let app = self.archive.app.clone().unwrap_or_default();
        let method = self.choices.method;
        let mut methods = v_flex().gap_0p5();
        for (ix, &candidate) in self.methods.iter().enumerate() {
            let selected = candidate == method;
            methods = methods.child(
                h_flex()
                    .id(("distribute-method", ix))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded(theme.radius)
                    .when(selected, |el| el.bg(theme.list_active))
                    .hover(|el| el.bg(theme.list_hover))
                    .child(Icon::new(if selected { IconName::CircleCheck } else { IconName::Circle }).small().text_color(if selected {
                        theme.primary
                    } else {
                        theme.muted_foreground
                    }))
                    .child(
                        v_flex()
                            .flex_1()
                            .child(div().text_sm().child(candidate.title()))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(candidate.description())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.set_method(candidate, cx))),
            );
        }
        let team = if app.team.is_empty() { "None (Sign to Run Locally)".to_string() } else { app.team.clone() };
        let no_team = app.team.is_empty() && method.needs_team();
        let action = if self.uploading() { "Upload" } else { "Export" };
        v_flex()
            .id("distribute-sheet")
            .key_context("DistributeSheet")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .w(px(520.))
            .p_3()
            .gap_2()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .text_sm()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Distribute “{}” {}", self.archive.name, app.version_label())))
            .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{} · {} · Team: {team}", self.archive.kind(), app.bundle_id)))
            .child(methods)
            .when(no_team, |el| {
                el.child(div().text_xs().text_color(theme.warning).child(
                    "This archive has no team, so it can't be signed for distribution: choose one in the target’s Signing & Capabilities tab and archive again.",
                ))
            })
            .child(self.render_options(cx))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("distribute-cancel").label("Cancel").small().on_click(cx.listener(|this, _, window, cx| this.dismiss(&PickerDismiss, window, cx))))
                    .child(
                        Button::new("distribute-confirm")
                            .label(action)
                            .small()
                            .primary()
                            .disabled(self.archive.app.is_none())
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(&PickerConfirm, window, cx))),
                    ),
            )
    }
}

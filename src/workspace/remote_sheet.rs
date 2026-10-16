//! Integrate ▸ Push… and Pull… (⌥⌘X): choose the remote branch (Push
//! offers a new one named after the local branch), with Xcode's option for
//! each: Include tags, or Rebase local changes onto upstream changes.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use super::Workspace;
use crate::actions::{PickerConfirm, PickerDismiss};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteAction {
    Push,
    Pull,
}

pub struct RemoteSheet {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    action: RemoteAction,
    branch: String,
    /// `origin/main`, …; Push's new remote branch last.
    choices: Vec<String>,
    selected: Option<String>,
    /// Push: Include tags. Pull: Rebase local changes onto upstream changes.
    option: bool,
}

impl EventEmitter<DismissEvent> for RemoteSheet {}

impl Focusable for RemoteSheet {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl RemoteSheet {
    fn new(workspace: &Workspace, weak: WeakEntity<Workspace>, action: RemoteAction, cx: &mut Context<Self>) -> Self {
        let branch = workspace.git.status.branch.clone().unwrap_or_default();
        let mut choices: Vec<String> = workspace.git.branches.iter().filter(|b| b.remote).map(|b| b.name.clone()).collect();
        let selected = workspace.default_remote_branch();
        if action == RemoteAction::Push {
            for remote in &workspace.git.remotes {
                let new = format!("{remote}/{branch}");
                if !choices.contains(&new) {
                    choices.push(new);
                }
            }
        }
        let selected = selected.filter(|s| choices.contains(s)).or(choices.first().cloned());
        Self { focus_handle: cx.focus_handle(), workspace: weak, action, branch, choices, selected, option: false }
    }

    fn title(&self) -> &'static str {
        match self.action {
            RemoteAction::Push => "Push",
            RemoteAction::Pull => "Pull",
        }
    }

    fn confirm(&mut self, _: &PickerConfirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(remote_branch) = self.selected.clone() else { return };
        let (action, option) = (self.action, self.option);
        cx.emit(DismissEvent);
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |ws, cx| match action {
                    RemoteAction::Push => ws.push(remote_branch, option, window, cx),
                    RemoteAction::Pull => ws.pull(remote_branch, option, window, cx),
                })
                .ok();
        });
    }

    fn dismiss(&mut self, _: &PickerDismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    /// The control channel's `git remote <command>`: `select <remote/branch>`,
    /// `option on|off`, `confirm`, `cancel`.
    pub fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
        match verb {
            "select" if self.choices.iter().any(|c| c == arg) => self.selected = Some(arg.to_string()),
            "select" => return Err(format!("no remote branch {arg}")),
            "option" => self.option = arg == "on",
            "confirm" => self.confirm(&PickerConfirm, window, cx),
            "cancel" => self.dismiss(&PickerDismiss, window, cx),
            other => return Err(format!("unknown command {other}")),
        }
        cx.notify();
        Ok(())
    }

    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "action": format!("{:?}", self.action),
            "choices": self.choices,
            "selected": self.selected,
            "option": self.option,
        })
    }
}

impl Render for RemoteSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let sheet = cx.entity().downgrade();
        let choices = self.choices.clone();
        let selected = self.selected.clone();
        let label = match (self.action, &self.selected) {
            (RemoteAction::Push, Some(s)) if !self.workspace.upgrade().is_some_and(|ws| ws.read(cx).git.branches.iter().any(|b| b.remote && &b.name == s)) => {
                format!("{s} (create)")
            }
            (_, Some(s)) => s.clone(),
            (_, None) => "No remotes".to_string(),
        };
        let menu = Button::new("remote-branch").label(label).small().outline().w(px(260.)).disabled(choices.is_empty()).dropdown_menu(move |mut menu, _, _| {
            for choice in choices.clone() {
                let (sheet, value) = (sheet.clone(), choice.clone());
                menu = menu.item(PopupMenuItem::new(choice.clone()).checked(selected.as_ref() == Some(&choice)).on_click(move |_, _, cx| {
                    let value = value.clone();
                    sheet
                        .update(cx, |sheet, cx| {
                            sheet.selected = Some(value);
                            cx.notify();
                        })
                        .ok();
                }));
            }
            menu
        });
        let (prompt, option) = match self.action {
            RemoteAction::Push => (format!("Push “{}” to:", self.branch), "Include tags"),
            RemoteAction::Pull => (format!("Pull into “{}” from:", self.branch), "Rebase local changes onto upstream changes"),
        };
        let title = self.title();
        v_flex()
            .id("remote-sheet")
            .key_context("RemoteSheet")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .w(px(440.))
            .p_3()
            .gap_2()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .text_sm()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(h_flex().gap_2().child(div().flex_1().child(prompt)).child(menu))
            .child(Checkbox::new("remote-option").label(option).checked(self.option).on_click(cx.listener(|this, checked: &bool, _, cx| {
                this.option = *checked;
                cx.notify();
            })))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("remote-cancel").label("Cancel").small().on_click(cx.listener(|this, _, window, cx| this.dismiss(&PickerDismiss, window, cx))))
                    .child(
                        Button::new("remote-confirm")
                            .label(title)
                            .small()
                            .primary()
                            .disabled(self.selected.is_none())
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(&PickerConfirm, window, cx))),
                    ),
            )
    }
}

impl Workspace {
    /// Integrate ▸ Push… / Pull….
    pub fn show_remote_sheet(&mut self, action: RemoteAction, window: &mut Window, cx: &mut Context<Self>) {
        if self.git.root.is_none() {
            self.notify_error("The project isn’t in a Git repository.".into(), window, cx);
            return;
        }
        if self.git.remotes.is_empty() {
            self.notify_error("The repository has no remotes.".into(), window, cx);
            return;
        }
        let weak = cx.weak_entity();
        let this = &*self;
        let sheet = cx.new(|cx| RemoteSheet::new(this, weak, action, cx));
        self.git.remote_sheet = Some(sheet.downgrade());
        self.show_modal(sheet, window, cx);
    }
}

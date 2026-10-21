//! Xcode's Add Line Bookmark / Add File Bookmark dialog (Bookmark Line With
//! Dialog ⌃⌥⌘', Bookmark File With Dialog ⌃⌥⇧⌘'): the bookmark's
//! description before it is added. Return adds it, Escape cancels.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use super::Workspace;
use crate::actions::{PickerConfirm, PickerDismiss};
use crate::bookmarks::{Location, file_name};

pub struct BookmarkDialog {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    location: Location,
    title: Entity<InputState>,
}

impl EventEmitter<DismissEvent> for BookmarkDialog {}

impl Focusable for BookmarkDialog {
    /// Typing goes to the description.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.title.read(cx).focus_handle(cx)
    }
}

impl BookmarkDialog {
    pub fn new(workspace: WeakEntity<Workspace>, location: Location, title: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Description").default_value(title));
        title.update(cx, |input, cx| input.select_all(window, cx));
        Self { focus_handle: cx.focus_handle(), workspace, location, title }
    }

    fn heading(&self) -> &'static str {
        match self.location {
            Location::Line { .. } => "Add Line Bookmark",
            Location::File { .. } => "Add File Bookmark",
        }
    }

    pub fn set_title(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.title.update(cx, |input, cx| input.set_value(title.to_string(), window, cx));
    }

    pub fn confirm(&mut self, _: &PickerConfirm, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title.read(cx).value().to_string();
        let location = self.location.clone();
        cx.emit(DismissEvent);
        self.workspace.update(cx, |ws, cx| ws.add_bookmark(location, title, window, cx)).ok();
    }

    fn dismiss(&mut self, _: &PickerDismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    /// For the test harness.
    pub fn summary(&self, cx: &App) -> serde_json::Value {
        serde_json::json!({ "heading": self.heading(), "title": self.title.read(cx).value().to_string() })
    }
}

impl Render for BookmarkDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let place = match &self.location {
            Location::Line { path, line } => format!("{}:{line}", file_name(path)),
            Location::File { path } => file_name(path),
        };
        v_flex()
            .id("bookmark-dialog")
            .key_context("BookmarkDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .w(px(380.))
            .p_3()
            .gap_2()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .text_sm()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(self.heading()))
            .child(div().text_xs().text_color(theme.muted_foreground).child(place))
            .child(Input::new(&self.title).small())
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("bookmark-cancel").label("Cancel").small().on_click(cx.listener(|this, _, window, cx| {
                        this.dismiss(&PickerDismiss, window, cx)
                    })))
                    .child(Button::new("bookmark-add").label("Add").small().primary().on_click(cx.listener(|this, _, window, cx| {
                        this.confirm(&PickerConfirm, window, cx)
                    }))),
            )
    }
}

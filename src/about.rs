//! zcode ▸ About zcode: a small window with the app's icon, name and
//! version.

use std::sync::{Arc, LazyLock};

use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::*;

/// The app icon (scripts/bundle/make_icon.swift's drawing as an SVG), drawn
/// by GPUI in color: the same with or without the bundle around the binary.
static ICON: LazyLock<Arc<Image>> = LazyLock::new(|| Arc::new(Image::from_bytes(ImageFormat::Svg, include_bytes!("about_icon.svg").to_vec())));

/// The open About window, so choosing the item again brings it forward.
#[derive(Default)]
struct AboutWindow(Option<AnyWindowHandle>);
impl Global for AboutWindow {}

pub struct AboutView {
    focus_handle: FocusHandle,
}

impl Focusable for AboutView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AboutView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .items_center()
            .justify_center()
            .gap_2()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(img(ICON.clone()).id("app-icon").size(px(96.)))
            .child(div().text_xl().font_weight(FontWeight::BOLD).child("zcode"))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(format!("Version {}", crate::bundle::version())),
            )
            .on_action(cx.listener(|_, _: &crate::actions::CloseTab, window, _| window.remove_window()))
    }
}

/// The control channel's `app about [close]`: whether the window is open
/// (closing it first), and the icon it shows.
pub fn command(arg: &str, cx: &mut App) -> Result<String, String> {
    let window = cx.try_global::<AboutWindow>().and_then(|w| w.0).filter(|w| cx.windows().contains(w));
    match (arg, window) {
        ("", _) => {}
        ("close", Some(window)) => window.update(cx, |_, window, _| window.remove_window()).map_err(|e| e.to_string())?,
        ("close", None) => {}
        _ => return Err("usage: app about [close]".into()),
    }
    let open = arg.is_empty() && window.is_some();
    Ok(serde_json::json!({ "open": open, "icon": "about_icon.svg" }).to_string())
}

/// Opens the About window, or brings the open one forward.
pub fn open(cx: &mut App) {
    if let Some(window) = cx.default_global::<AboutWindow>().0
        && window.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    let window_size = size(px(280.), px(220.));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, window_size, cx))),
        titlebar: Some(TitlebarOptions {
            title: Some("About zcode".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.), px(13.))),
        }),
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    };
    let result = gpui_kit::open_window(options, cx, |window, cx| {
        crate::settings::themes::sync(window, cx);
        window
            .observe_window_appearance(crate::settings::themes::sync)
            .detach();
        let about = cx.new(|cx| AboutView { focus_handle: cx.focus_handle() });
        window.focus(&about.read(cx).focus_handle.clone(), cx);
        about
    });
    match result {
        Ok((window, _)) => cx.default_global::<AboutWindow>().0 = Some(window),
        Err(err) => log::error!("failed to open the About window: {err:#}"),
    }
}

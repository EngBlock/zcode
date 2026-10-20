//! Images in a tab: a PNG, JPEG, GIF, TIFF, BMP, WebP, ICO, HEIC, ICNS or
//! PDF opened from the navigator, Open Quickly or the command line is shown
//! rather than read as text. It's decoded off the main thread
//! (`xcassets::thumbnails::decode`, the asset catalog's), drawn to fit the
//! pane but never larger than its own size, and read again when its tab is
//! shown after the file changed. SVGs stay text: they're source.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;
use serde_json::{Value, json};

use super::Workspace;
use super::pane::{ImageItem, Item, PaneId};
use crate::xcassets::thumbnails::{self, Stamp, ThumbnailCache};

/// The longest side a raster image is decoded at; larger ones are scaled
/// down to it.
const MAX_RASTER: u32 = 8192;
/// The longest side a PDF's page is drawn at.
const MAX_VECTOR: u32 = 2048;

const EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "tif", "tiff", "bmp", "webp", "ico", "icns", "heic", "heif", "jp2", "pdf"];

/// Whether `path` opens in an image tab.
pub fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|ext| EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

enum State {
    Loading,
    Ready { image: Arc<RenderImage>, source: (u32, u32) },
    Failed(String),
}

pub struct ImageView {
    path: PathBuf,
    /// The file as last read.
    stamp: Stamp,
    state: State,
    focus_handle: FocusHandle,
    _load: Option<Task<()>>,
}

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok();
    (meta.as_ref().and_then(|m| m.modified().ok()), meta.map(|m| m.len()).unwrap_or(0))
}

impl ImageView {
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut view = Self { path, stamp: (None, 0), state: State::Loading, focus_handle: cx.focus_handle(), _load: None };
        view.load(cx);
        view
    }

    /// Reads the file again if it changed since it was read.
    pub fn reload_if_changed(&mut self, cx: &mut Context<Self>) {
        if stamp(&self.path) != self.stamp {
            self.load(cx);
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.stamp = stamp(&self.path);
        let path = self.path.clone();
        let max = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) { MAX_VECTOR } else { MAX_RASTER };
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    thumbnails::decode(&path, max).map(|d| {
                        let source = d.source;
                        (d.into_render_image(), source)
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                let state = match result {
                    Ok((image, source)) => State::Ready { image, source },
                    Err(err) => State::Failed(err),
                };
                this.set_state(state, cx);
            })
            .ok();
        }));
    }

    fn set_state(&mut self, state: State, cx: &mut Context<Self>) {
        if let State::Ready { image, .. } = std::mem::replace(&mut self.state, state) {
            ThumbnailCache::retire(image, cx);
        }
        cx.notify();
    }

    /// Lets go of the decoded image (the tab closed).
    pub fn release(&mut self, cx: &mut Context<Self>) {
        self._load = None;
        self.set_state(State::Loading, cx);
    }

    /// What the footer says: the format, the size, the file's length.
    fn description(&self) -> String {
        let format = self.path.extension().map(|e| e.to_string_lossy().to_ascii_uppercase()).unwrap_or_default();
        let length = format_bytes(self.stamp.1);
        match &self.state {
            State::Ready { source: (w, h), .. } => {
                let unit = if format == "PDF" { "points" } else { "pixels" };
                format!("{format} · {w} × {h} {unit} · {length}")
            }
            _ => format!("{format} · {length}"),
        }
    }

    /// `dump`'s `image`.
    pub fn summary(&self) -> Value {
        let state = match &self.state {
            State::Loading => json!("loading"),
            State::Ready { source, .. } => json!({ "width": source.0, "height": source.1 }),
            State::Failed(err) => json!({ "failed": err }),
        };
        json!({ "path": self.path, "state": state, "description": self.description() })
    }
}

fn format_bytes(len: u64) -> String {
    match len {
        n if n < 1024 => format!("{n} bytes"),
        n if n < 1024 * 1024 => format!("{:.1} KB", n as f64 / 1024.),
        n => format!("{:.1} MB", n as f64 / (1024. * 1024.)),
    }
}

impl Focusable for ImageView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ImageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        ThumbnailCache::release(window, cx);
        let theme = cx.theme().clone();
        let picture = match &self.state {
            // Fits the pane, but a small image stays its own size (a point
            // a pixel, as Xcode shows it).
            State::Ready { image, source: (w, h) } => div()
                .size_full()
                .max_w(px(*w as f32))
                .max_h(px(*h as f32))
                .child(img(ImageSource::Render(image.clone())).size_full().object_fit(ObjectFit::Contain))
                .into_any_element(),
            State::Loading => div().into_any_element(),
            State::Failed(err) => div().text_sm().text_color(theme.muted_foreground).child(format!("Can’t show this image: {err}")).into_any_element(),
        };
        v_flex()
            .id("image-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .child(h_flex().flex_1().min_h_0().p_4().items_center().justify_center().child(picture))
            .child(
                h_flex()
                    .h(px(24.))
                    .px_3()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.description()),
            )
    }
}

impl Workspace {
    /// An image's tab at the end of `pane`.
    pub(super) fn add_image_item(&mut self, pane: PaneId, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        self.pane(pane)?;
        let view = cx.new(|cx| ImageView::new(path.to_path_buf(), cx));
        let id = view.entity_id();
        let focus = view.read(cx).focus_handle(cx);
        let subscriptions = vec![cx.on_focus_in(&focus, window, move |this, window, cx| this.view_focused(id, window, cx))];
        let p = self.pane_mut(pane)?;
        p.items.push(Item::Image(ImageItem { path: path.to_path_buf(), view, _subscriptions: subscriptions }));
        Some(p.items.len() - 1)
    }

    /// `dump`'s `image`: the focused pane's.
    pub(super) fn image_summary(&self, cx: &App) -> Value {
        match self.pane(self.active_pane).and_then(|p| p.active_item()) {
            Some(Item::Image(item)) => item.view.read(cx).summary(),
            _ => Value::Null,
        }
    }
}

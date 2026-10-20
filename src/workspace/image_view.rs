//! Images in a tab: a PNG, JPEG, GIF, TIFF, BMP, WebP, ICO, HEIC, ICNS or
//! PDF opened from the navigator, Open Quickly or the command line is shown
//! rather than read as text. It's decoded off the main thread
//! (`xcassets::thumbnails::decode`, the asset catalog's), drawn to fit the
//! pane but never larger than its own size, and read again when its tab is
//! shown after the file changed. SVGs stay text: they're source.
//!
//! An SVG's preview (Editor ▸ Open Preview, `rendered_view.rs`) is the same
//! view drawing its buffer's text instead of a file: rasterized by resvg at
//! the pane's size in pixels, again when the pane is resized or the text
//! changes, off the main thread. While an edit leaves the text unreadable
//! the last good drawing stays, and the footer says what's wrong.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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
/// How long a preview's resizing pauses before it's drawn at the new size.
const RESIZE_PAUSE: Duration = Duration::from_millis(80);

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

/// What the view draws: the file as it is on disk, or an SVG's text as its
/// buffer has it.
enum Source {
    File,
    Text {
        text: Option<Arc<str>>,
        /// Why the text last given couldn't be drawn (the drawing shown is
        /// of text before it).
        error: Option<String>,
        /// The pane's room for the picture, in pixels, as last laid out.
        room: Option<(u32, u32)>,
    },
}

pub struct ImageView {
    path: PathBuf,
    /// The file as last read.
    stamp: Stamp,
    source: Source,
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
        let mut view =
            Self { path, stamp: (None, 0), source: Source::File, state: State::Loading, focus_handle: cx.focus_handle(), _load: None };
        view.load(cx);
        view
    }

    /// A preview of an SVG's text, drawn once it's given (`set_text`) and
    /// the view is laid out.
    pub fn svg_text(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let source = Source::Text { text: None, error: None, room: None };
        Self { path, stamp: (None, 0), source, state: State::Loading, focus_handle: cx.focus_handle(), _load: None }
    }

    /// Reads the file again if it changed since it was read.
    pub fn reload_if_changed(&mut self, cx: &mut Context<Self>) {
        if matches!(self.source, Source::File) && stamp(&self.path) != self.stamp {
            self.load(cx);
        }
    }

    /// A preview's text changed: it's drawn again.
    pub fn set_text(&mut self, new: Arc<str>, cx: &mut Context<Self>) {
        let Source::Text { text, .. } = &mut self.source else { return };
        if text.as_deref() != Some(&*new) {
            *text = Some(new);
            self.draw(Duration::ZERO, cx);
        }
    }

    /// The pane's room for a preview's picture, from its layout: a new size
    /// draws it again, once the resizing pauses.
    fn set_room(&mut self, size: Size<Pixels>, scale: f32, cx: &mut Context<Self>) {
        let Source::Text { room, .. } = &mut self.source else { return };
        let pixels = ((f32::from(size.width) * scale).round().max(1.) as u32, (f32::from(size.height) * scale).round().max(1.) as u32);
        if *room != Some(pixels) {
            let first = room.is_none();
            *room = Some(pixels);
            self.draw(if first { Duration::ZERO } else { RESIZE_PAUSE }, cx);
        }
    }

    /// Draws a preview's text at its room's size, after `delay`.
    fn draw(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let Source::Text { text: Some(text), room: Some((width, height)), .. } = &self.source else { return };
        let (text, width, height) = (text.clone(), (*width).min(MAX_VECTOR * 2), (*height).min(MAX_VECTOR * 2));
        let resources = self.path.parent().map(Path::to_path_buf);
        self._load = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let result = cx
                .background_spawn(async move {
                    thumbnails::render_svg(&text, resources.as_deref(), width, height).map(|d| {
                        let source = d.source;
                        (d.into_render_image(), source)
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                let Source::Text { error, .. } = &mut this.source else { return };
                match result {
                    Ok((image, source)) => {
                        *error = None;
                        this.set_state(State::Ready { image, source }, cx);
                    }
                    // The last good drawing stays.
                    Err(err) if matches!(this.state, State::Ready { .. }) => {
                        *error = Some(err);
                        cx.notify();
                    }
                    Err(err) => {
                        *error = Some(err.clone());
                        this.set_state(State::Failed(err), cx);
                    }
                }
            })
            .ok();
        }));
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

    /// What the footer says: the format, the size, the file's (or the
    /// preview's text's) length.
    fn description(&self) -> String {
        let format = self.path.extension().map(|e| e.to_string_lossy().to_ascii_uppercase()).unwrap_or_default();
        let length = match &self.source {
            Source::File => format_bytes(self.stamp.1),
            Source::Text { text, .. } => format_bytes(text.as_ref().map_or(0, |t| t.len() as u64)),
        };
        match &self.state {
            State::Ready { source: (w, h), .. } => {
                let unit = if format == "PDF" || format == "SVG" { "points" } else { "pixels" };
                format!("{format} · {w} × {h} {unit} · {length}")
            }
            _ => format!("{format} · {length}"),
        }
    }

    /// Why a preview's latest text couldn't be drawn.
    fn error(&self) -> Option<&str> {
        match &self.source {
            Source::Text { error, .. } => error.as_deref(),
            Source::File => None,
        }
    }

    /// `dump`'s `image` (and a preview's `rendered`).
    pub fn summary(&self) -> Value {
        let state = match &self.state {
            State::Loading => json!("loading"),
            State::Ready { source, .. } => json!({ "width": source.0, "height": source.1 }),
            State::Failed(err) => json!({ "failed": err }),
        };
        let mut summary = json!({ "path": self.path, "state": state, "description": self.description() });
        if let Source::Text { text, error, room } = &self.source {
            summary["text"] = json!(text.as_deref());
            summary["error"] = json!(error);
            summary["room"] = json!(room);
            if let State::Ready { image, .. } = &self.state {
                let size = image.size(0);
                summary["drawn"] = json!([size.width.0, size.height.0]);
            }
        }
        summary
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
        let preview = matches!(self.source, Source::Text { .. });
        let picture = match &self.state {
            // A preview is drawn for the pane's size.
            State::Ready { image, .. } if preview => {
                img(ImageSource::Render(image.clone())).size_full().object_fit(ObjectFit::Contain).into_any_element()
            }
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
        // A preview measures the room it has, to be drawn at that size.
        let room = preview.then(|| {
            let view = cx.entity();
            canvas(
                move |bounds, window, cx| {
                    let scale = window.scale_factor();
                    view.update(cx, |view, cx| view.set_room(bounds.size, scale, cx));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full()
        });
        let error = self.error().map(|err| format!("Can’t draw the SVG: {err}"));
        v_flex()
            .id("image-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .child(div().relative().size_full().flex().items_center().justify_center().children(room).child(picture)),
            )
            .child(
                h_flex()
                    .h(px(24.))
                    .px_3()
                    .gap_2()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().flex_shrink_0().child(self.description()))
                    .children(error.map(|err| div().flex_1().min_w_0().truncate().text_color(theme.danger).child(err))),
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

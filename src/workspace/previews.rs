//! SwiftUI previews in the workspace: a mark in the gutter on each
//! `#Preview` and `PreviewProvider` (over the line number, as test
//! diamonds are), and Editor ▸ Show in Xcode Canvas (⌥⌘↩, Xcode's Canvas
//! key), which hands the file to Xcode at the preview with the canvas
//! showing (`xcode_app`). Clicking a mark shows that preview.
//!
//! zcode doesn't render previews itself: Xcode's canvas runs on private
//! frameworks (see `docs/previews.md` for how far a native preview could
//! go). The file is saved first, so Xcode previews what the editor shows,
//! and Xcode reloads it after each later save here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;
use zcode_editor::{EditorState, GutterItem, GutterLane};

use super::Workspace;
use crate::previews::{self, PreviewDecl};
use crate::xcode_app::{self, CanvasOutcome};

/// The gutter lane of preview marks.
pub(super) const PREVIEW_LANE: &str = "previews";
const MARK_SIZE: f32 = 14.;
/// Swift files larger than this aren't searched for previews.
const MAX_SCAN_SIZE: usize = 2 * 1024 * 1024;

/// A buffer's previews, found again a moment after each edit.
#[derive(Default)]
pub struct BufferPreviews {
    pub decls: Vec<PreviewDecl>,
    task: Option<Task<()>>,
}

/// A Show in Xcode Canvas request: the file, the 1-based line Xcode
/// selects, and how it went (`None` while Xcode opens it).
#[derive(Clone, Debug)]
pub(super) struct CanvasRequest {
    pub file: PathBuf,
    pub line: u32,
    pub preview: Option<String>,
    pub outcome: Option<CanvasOutcome>,
}

#[derive(Clone, Copy)]
struct MarkColors {
    idle: Hsla,
    accent: Hsla,
    on_accent: Hsla,
}

/// Paints a preview mark centered in `bounds`: a canvas (a portrait
/// rounded rectangle) with a play triangle, filled while hovered.
fn paint_mark(window: &mut Window, bounds: Bounds<Pixels>, hovered: bool, colors: MarkColors) {
    let c = bounds.center();
    let h = bounds.size.height.min(bounds.size.width);
    let w = h * 0.78;
    let frame = Bounds::new(point(c.x - w / 2., c.y - h / 2.), size(w, h));
    let radius = h * 0.2;
    let glyph = if hovered {
        window.paint_quad(fill(frame, colors.accent).corner_radii(radius));
        colors.on_accent
    } else {
        window.paint_quad(outline(frame, colors.idle, BorderStyle::Solid).corner_radii(radius));
        colors.idle
    };
    let u = h * 0.2;
    let mut play = PathBuilder::fill();
    play.add_polygon(&[point(c.x - u * 0.8, c.y - u), point(c.x + u, c.y), point(c.x - u * 0.8, c.y + u)], true);
    if let Ok(path) = play.build() {
        window.paint_path(path, glyph);
    }
}

impl Workspace {
    // ------------------------------------------------------------------
    // Finding previews

    /// Finds `path`'s previews after `delay`, in the background.
    pub(super) fn scan_previews(&mut self, path: &Path, delay: Duration, cx: &mut Context<Self>) {
        if path.extension().and_then(|e| e.to_str()) != Some("swift") {
            return;
        }
        let path = path.to_path_buf();
        let task = cx.spawn({
            let path = path.clone();
            async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let Ok(Some(text)) = this.update(cx, |this, cx| this.buffer(&path).map(|b| b.editor.read(cx).text().clone())) else {
                    return;
                };
                let found = cx
                    .background_spawn(async move {
                        if text.len() > MAX_SCAN_SIZE {
                            return vec![];
                        }
                        previews::previews(&text.to_string())
                    })
                    .await;
                this.update(cx, |this, cx| {
                    let Some(buffer) = this.buffers.iter_mut().find(|b| b.path == path) else { return };
                    if buffer.previews.decls != found {
                        buffer.previews.decls = found;
                        cx.notify();
                    }
                })
                .ok();
            }
        });
        if let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) {
            buffer.previews.task = Some(task);
        }
    }

    /// Follows an edit: the marks move with the text once typing stops.
    pub(super) fn previews_edited(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.scan_previews(path, Duration::from_millis(300), cx);
    }

    // ------------------------------------------------------------------
    // The gutter

    /// The preview marks in `editor`'s gutter, over the line numbers.
    pub(super) fn preview_lane(&self, editor: &Entity<EditorState>, cx: &Context<Self>) -> GutterLane {
        let workspace = cx.weak_entity();
        let editor_id = editor.entity_id();
        GutterLane::new(PREVIEW_LANE, move |rows, cx| {
            let Some(this) = workspace.upgrade() else { return vec![] };
            let this = this.read(cx);
            let Some(path) = this.buffer_path_of(editor_id) else { return vec![] };
            let Some(buffer) = this.buffer(&path) else { return vec![] };
            let theme = cx.theme();
            let colors = MarkColors { idle: theme.muted_foreground, accent: theme.primary, on_accent: theme.primary_foreground };
            buffer
                .previews
                .decls
                .iter()
                .filter(|decl| rows.rows.contains(&decl.line))
                .map(|decl| {
                    let line = decl.line;
                    let workspace = workspace.clone();
                    let path = path.clone();
                    GutterItem::new(line, move |paint, window, _| {
                        // The mark replaces the line number, as a test diamond does.
                        let target = paint.bounds;
                        let cover = Bounds::new(
                            point(paint.cell.left(), target.origin.y - px(1.)),
                            size(target.right() - paint.cell.left() + px(2.), target.size.height + px(2.)),
                        );
                        window.paint_quad(fill(cover, paint.background));
                        paint_mark(window, target, paint.hovered, colors);
                    })
                    .bounds(|cell, line_height| {
                        let side = px(MARK_SIZE).min(line_height - px(2.));
                        let right = cell.left() + cell.size.width.max(px(MARK_SIZE));
                        Bounds::new(point(right - side - px(2.), cell.top() + (line_height - side) / 2.), size(side, side))
                    })
                    .cursor(CursorStyle::PointingHand)
                    .on_click(move |_, window, cx| {
                        let path = path.clone();
                        workspace.update(cx, |this, cx| this.show_preview_in_xcode(&path, line, window, cx)).is_ok()
                    })
                })
                .collect()
        })
        .order(3)
    }

    // ------------------------------------------------------------------
    // Show in Xcode Canvas

    /// Editor ▸ Show in Xcode Canvas (⌥⌘↩): the preview the cursor is in,
    /// else the file at the cursor (the canvas shows all its previews).
    pub(super) fn show_in_xcode_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.active_item() else { return };
        let (path, cursor) = (item.path.clone(), item.editor.read(cx).cursor_position().line);
        let Some(buffer) = self.buffer(&path) else { return };
        if buffer.previews.decls.is_empty() {
            let name = buffer.title();
            window.push_notification(Notification::info(format!("{name} has no SwiftUI previews (#Preview or PreviewProvider).")), cx);
            return;
        }
        let line = previews::preview_at(&buffer.previews.decls, cursor).map(|p| p.line).unwrap_or(cursor);
        self.show_preview_in_xcode(&path, line, window, cx);
    }

    /// Saves `path`, opens it in Xcode at `line` (0-based) and turns on the
    /// canvas, in the background; tells the user when the canvas couldn't
    /// be shown.
    pub(super) fn show_preview_in_xcode(&mut self, path: &Path, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.buffers.iter().position(|b| b.path == path && b.dirty) {
            self.save_buffer(ix, window, cx);
        }
        let preview = self.buffer(path).and_then(|b| previews::preview_at(&b.previews.decls, line)).map(PreviewDecl::title);
        let project = xcode_app::project_to_open(&self.project.kind, &self.project.root);
        let file = path.to_path_buf();
        self.canvas_request = Some(CanvasRequest { file: file.clone(), line: line + 1, preview, outcome: None });
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_spawn({
                    let file = file.clone();
                    async move { xcode_app::show_in_canvas(project.as_deref(), &file, line + 1) }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                match &outcome {
                    CanvasOutcome::OpenedWithoutCanvas(why) => {
                        log::info!("Show in Xcode Canvas: {why}");
                        window.push_notification(Notification::warning(format!("Opened in Xcode; press ⌥⌘↩ there to show the canvas ({why}).")), cx);
                    }
                    CanvasOutcome::Failed(why) => this.notify_error(format!("Couldn’t open {} in Xcode: {why}", file.display()), window, cx),
                    CanvasOutcome::Shown | CanvasOutcome::AlreadyShown | CanvasOutcome::Recorded => {}
                }
                if let Some(request) = this.canvas_request.as_mut().filter(|r| r.file == file && r.line == line + 1) {
                    request.outcome = Some(outcome);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // The debug harness

    /// The active file's previews and their marks, and the last request.
    pub(super) fn previews_summary(&self, cx: &App) -> serde_json::Value {
        let decls = self.active_buffer().map(|b| b.previews.decls.clone()).unwrap_or_default();
        let title = |line: u32| decls.iter().find(|d| d.line + 1 == line).map(PreviewDecl::title);
        serde_json::json!({
            "file": decls.iter().map(|d| format!("{:?} {} @{}", d.kind, d.title(), d.line + 1)).collect::<Vec<_>>(),
            "gutter": self.painted_gutter(PREVIEW_LANE, cx).into_iter().map(|(line, bounds)| serde_json::json!({
                "line": line,
                "preview": title(line),
                "x": f32::from(bounds.center().x),
                "y": f32::from(bounds.center().y),
            })).collect::<Vec<_>>(),
            "last": self.canvas_request.as_ref().map(|r| serde_json::json!({
                "file": r.file,
                "line": r.line,
                "preview": r.preview,
                "outcome": r.outcome.as_ref().map(CanvasOutcome::label),
            })),
        })
    }
}

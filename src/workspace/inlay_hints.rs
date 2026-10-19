//! Inlay hints (Editor ▸ Inlay Hints): the types sourcekit-lsp infers for
//! bindings without one, and the parameter names clangd shows before C and
//! Objective-C arguments, drawn in the text as the editor's inlays.
//!
//! Each pane's view asks for what it shows: its visible rows and a screen
//! above and below, again when it scrolls past those and a moment after
//! typing stops. A new request replaces (and so cancels) the one before,
//! and an answer for text that has changed since is dropped; the hints on
//! screen sit on anchors meanwhile, so they move with the typing. Off by
//! default, as in Zed (Xcode has none).

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use serde_json::json;
use zcode_editor::{EditorState, Inlay};

use super::Workspace;
use crate::lsp::{inlay_hints, path_to_uri};

const KEY: &str = "inlay-hints";
/// How long typing pauses before a view asks again.
const AFTER_EDIT: Duration = Duration::from_millis(400);
/// How long scrolling pauses before a view asks for rows it hasn't got.
const AFTER_SCROLL: Duration = Duration::from_millis(75);

#[derive(Default)]
pub struct InlayHints {
    pub enabled: bool,
    views: HashMap<EntityId, ViewHints>,
}

#[derive(Default)]
struct ViewHints {
    /// The rows and document version the hints shown were asked for.
    fetched: Option<(Range<u32>, i32)>,
    task: Option<Task<()>>,
}

impl InlayHints {
    pub fn new(enabled: bool) -> Self {
        Self { enabled, views: HashMap::new() }
    }
}

impl Workspace {
    /// Editor ▸ Inlay Hints (`inlay_hints.enabled`), as the settings changed.
    pub(super) fn set_inlay_hints(&mut self, on: bool, cx: &mut Context<Self>) {
        self.inlay_hints.enabled = on;
        if self.inlay_hints.enabled {
            self.refresh_inlay_hints(cx);
        } else {
            for (_, view) in self.inlay_hints.views.drain() {
                drop(view.task);
            }
            for item in self.panes.iter().flat_map(|pane| pane.items.iter().filter_map(super::Item::editor)) {
                item.editor.update(cx, |state, cx| state.clear_inlays(KEY, cx));
            }
        }
        cx.notify();
    }

    /// Asks again for every view (the server said its answers changed, or
    /// it just started).
    pub(super) fn refresh_inlay_hints(&mut self, cx: &mut Context<Self>) {
        if !self.inlay_hints.enabled {
            return;
        }
        let views: Vec<(Entity<EditorState>, PathBuf)> = self
            .panes
            .iter()
            .flat_map(|pane| pane.items.iter().filter_map(super::Item::editor))
            .filter(|item| item.plist.is_none())
            .map(|item| (item.editor.clone(), item.path.clone()))
            .collect();
        for (editor, path) in views {
            self.inlay_hints.views.entry(editor.entity_id()).or_default().fetched = None;
            self.schedule_inlay_hints(editor, path, Duration::ZERO, cx);
        }
    }

    /// A view scrolled (or was resized): ask for rows it has no hints for.
    pub(super) fn inlay_hints_viewport_changed(&mut self, editor: &Entity<EditorState>, rows: &Range<u32>, cx: &mut Context<Self>) {
        if !self.inlay_hints.enabled {
            return;
        }
        let Some(path) = self.buffer_path_of(editor.entity_id()) else { return };
        let version = self.lsp.read(cx).document_version(&path);
        let covered = self.inlay_hints.views.get(&editor.entity_id()).and_then(|view| view.fetched.as_ref()).is_some_and(
            |(fetched, at)| Some(*at) == version && fetched.start <= rows.start && rows.end <= fetched.end,
        );
        if !covered {
            self.schedule_inlay_hints(editor.clone(), path, AFTER_SCROLL, cx);
        }
    }

    /// The buffer at `path` was edited: each view asks again once typing
    /// pauses.
    pub(super) fn inlay_hints_edited(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.inlay_hints.enabled {
            return;
        }
        let views: Vec<Entity<EditorState>> = self.views_of(path).iter().map(|item| item.editor.clone()).collect();
        for editor in views {
            self.schedule_inlay_hints(editor, path.to_path_buf(), AFTER_EDIT, cx);
        }
    }

    /// `path` was opened at its servers (again): each view asks now.
    pub(super) fn inlay_hints_opened(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.inlay_hints.enabled {
            return;
        }
        let views: Vec<Entity<EditorState>> = self.views_of(path).iter().map(|item| item.editor.clone()).collect();
        for editor in views {
            self.schedule_inlay_hints(editor, path.to_path_buf(), Duration::ZERO, cx);
        }
    }

    pub(super) fn inlay_hints_view_closed(&mut self, view: EntityId) {
        self.inlay_hints.views.remove(&view);
    }

    fn schedule_inlay_hints(&mut self, editor: Entity<EditorState>, path: PathBuf, delay: Duration, cx: &mut Context<Self>) {
        if !self.lsp.read(cx).document_features(&path).iter().any(|f| f.inlay_hints) {
            return;
        }
        let view = editor.entity_id();
        let task = cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let request = this.update(cx, |this, cx| {
                let rows = editor.read(cx).visible_rows()?;
                // A screen above and below, so scrolling a little needs nothing new.
                let margin = rows.len() as u32;
                let rows = rows.start.saturating_sub(margin)..rows.end + margin;
                let text = editor.read(cx).text().clone();
                let (server, _) = this.lsp.update(cx, |lsp, cx| lsp.synced_server_with(&path, &text, |f| f.inlay_hints, cx))?;
                let version = this.lsp.read(cx).document_version(&path)?;
                let params = json!({
                    "textDocument": { "uri": path_to_uri(&path).as_str() },
                    "range": inlay_hints::request_range(&text, rows.clone()),
                });
                Some((server.request_raw("textDocument/inlayHint", params), text, rows, version))
            });
            let Ok(Some((request, text, rows, version))) = request else { return };
            // Dropping this task (a newer request replaced it) cancels the request.
            let Ok(response) = request.await else { return };
            let hints = cx
                .background_spawn(async move {
                    let hints: Vec<lsp_types::InlayHint> = serde_json::from_value(response).unwrap_or_default();
                    inlay_hints::to_hints(&text, hints)
                })
                .await;
            this.update(cx, |this, cx| {
                // Typed since: the hints are for other text; the next request has it.
                if this.lsp.read(cx).document_version(&path) != Some(version) || !this.inlay_hints.enabled {
                    return;
                }
                let color = cx.theme().highlight_theme.style.syntax.style("hint").and_then(|style| style.color);
                let inlays = hints
                    .into_iter()
                    .map(|hint| {
                        let inlay = Inlay::new(hint.offset, hint.text);
                        let inlay = if hint.before_text { inlay.before_text() } else { inlay };
                        match color {
                            Some(color) => inlay.color(color),
                            None => inlay,
                        }
                    })
                    .collect();
                editor.update(cx, |state, cx| state.set_inlays(KEY, inlays, cx));
                if let Some(view) = this.inlay_hints.views.get_mut(&editor.entity_id()) {
                    view.fetched = Some((rows, version));
                }
            })
            .ok();
        });
        self.inlay_hints.views.entry(view).or_default().task = Some(task);
    }

    /// The control channel's view of the active editor's hints.
    pub(super) fn inlay_hints_summary(&self, cx: &App) -> serde_json::Value {
        let hints = self.active_editor().map(|editor| {
            editor.read(cx).inlays(KEY).into_iter().map(|inlay| json!([inlay.offset, inlay.text.to_string()])).collect::<Vec<_>>()
        });
        json!({ "enabled": self.inlay_hints.enabled, "active": hints })
    }
}

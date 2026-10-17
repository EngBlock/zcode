//! Predictive code completion in the window (3.3): asking the provider in
//! `edit_predictions.provider` for the focused editor tab, showing what it
//! predicts, and the status bar's indicator.
//!
//! Typing in the focused tab starts a request once typing pauses
//! (`debounce_ms`); Editor ▸ Show Predictive Completion (⌃⌥Space, Xcode's
//! key) asks at once, also with `automatic` off. A new request drops the
//! one before it, which cancels it (a dropped Claude stream closes its
//! connection; a language server gets `$/cancelRequest`); so does moving
//! the caret before an answer comes. An answer for text that has changed
//! since is dropped. A prediction shows as the editor draws it
//! (`zcode_editor::EditPrediction`) and streams in a piece at a time; typing
//! what it predicts keeps it (and its stream) going instead of asking
//! again. Tab accepts (bound in the `edit_prediction` key context, so Tab
//! indents as usual otherwise), Escape or ⌥] dismisses.
//!
//! Requests come from file tabs only: not multibuffers, Code Review, the
//! property list editor, terminals or sheets' text fields; not read-only
//! files, Vim's normal and visual modes, or files matching
//! `disabled_globs` (secrets, keys). With the provider `none` nothing here
//! runs: no subscriptions, no tasks, one comparison per edit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::{FutureExt as _, StreamExt as _};
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::*;
use zcode_editor::{BufferEdit, EditPrediction, EditPredictionId, EditorState, Rope, RopeExt as _, VimMode};

use super::Workspace;
use crate::actions::{AcceptEditPrediction, DismissEditPrediction, ShowPredictiveCompletion, TabOrPlaceholder};
use crate::predictions::{self, PredictionRequest, RecentEdits, Update, claude};
use crate::settings::{EditPredictionSettings, PredictionProvider};

#[derive(Default)]
pub struct Predictions {
    settings: EditPredictionSettings,
    /// The request waiting out the debounce or in flight.
    pending: Option<Pending>,
    /// Each open file's text as of its last edit, for the recent edits a
    /// request carries.
    shadows: HashMap<PathBuf, Shadow>,
    recent: RecentEdits,
    pub status: Status,
    stats: Stats,
    /// What the last request asked, for the control channel.
    last_request: Option<serde_json::Value>,
}

struct Pending {
    view: EntityId,
    cursor: usize,
    /// Whether it has answered (or shown something): no longer stale when
    /// the caret moves.
    answered: bool,
    /// Whether it got past the debounce and was sent.
    sent: bool,
    _task: Task<()>,
}

struct Shadow {
    editor: EntityId,
    text: Rope,
    _subscription: Subscription,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Requesting,
    Failed(String),
    /// The focused file is in `disabled_globs`.
    Excluded,
}

#[derive(Default, Debug)]
struct Stats {
    requests: u64,
    cancelled: u64,
    shown: u64,
    accepted: u64,
    failed: u64,
}

impl Predictions {
    pub fn new(settings: &EditPredictionSettings) -> Self {
        Self { settings: settings.clone(), ..Default::default() }
    }

    fn on(&self) -> bool {
        self.settings.provider != PredictionProvider::None
    }
}

impl Workspace {
    /// The settings changed: off drops everything (and every prediction on
    /// screen), on starts following the open files.
    pub(super) fn apply_prediction_settings(&mut self, cx: &mut Context<Self>) {
        let new = self.settings.current.edit_predictions.clone();
        if new == self.predictions.settings {
            return;
        }
        let was_on = self.predictions.on();
        self.predictions.settings = new;
        if self.predictions.on() {
            if !was_on {
                let paths: Vec<PathBuf> = self.buffers.iter().map(|b| b.path.clone()).collect();
                for path in paths {
                    self.follow_for_predictions(&path, cx);
                }
            }
        } else {
            self.predictions = Predictions::new(&self.predictions.settings);
            for editor in self.prediction_editors() {
                editor.update(cx, |state, cx| state.dismiss_edit_prediction(cx));
            }
        }
        cx.notify();
    }

    fn prediction_editors(&self) -> Vec<Entity<EditorState>> {
        self.panes.iter().flat_map(|pane| pane.items.iter().filter_map(super::Item::editor)).map(|item| item.editor.clone()).collect()
    }

    /// The focused tab's editor and file, when predictions may go there.
    fn prediction_target(&self) -> Option<(Entity<EditorState>, PathBuf)> {
        let item = self.pane(self.active_pane)?.active_editor_item()?;
        item.shown_plist().is_none().then(|| (item.editor.clone(), item.path.clone()))
    }

    /// Records `path`'s edits (as the lines they changed) from now on.
    fn follow_for_predictions(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffer(path) else { return };
        let editor = buffer.editor.clone();
        if self.predictions.shadows.get(path).is_some_and(|s| s.editor == editor.entity_id()) {
            return;
        }
        let key = path.to_path_buf();
        let subscription = cx.subscribe(&editor, move |this, editor, edit: &BufferEdit, cx| {
            let display = this.display_path(&key);
            let Some(shadow) = this.predictions.shadows.get_mut(&key) else { return };
            let before = shadow.text.clone();
            if edit.range.end > before.len() {
                shadow.text = editor.read(cx).text().clone();
                return;
            }
            shadow.text.replace(edit.range.clone(), &edit.new_text);
            // A reload replaces the whole text: not something the user typed.
            if !(edit.range.start == 0 && edit.range.end == before.len() && before.len() > 0) {
                this.predictions.recent.record(&display, &before, edit.range.clone(), &shadow.text, edit.new_text.len());
            }
        });
        let text = editor.read(cx).text().clone();
        self.predictions.shadows.insert(path.to_path_buf(), Shadow { editor: editor.entity_id(), text, _subscription: subscription });
    }

    fn display_path(&self, path: &Path) -> String {
        path.strip_prefix(&self.project.root).unwrap_or(path).to_string_lossy().to_string()
    }

    /// A file was edited (from any view): if the focused tab shows it and
    /// its prediction didn't survive the edit, ask again once typing pauses.
    pub(super) fn predictions_edited(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.predictions.on() {
            return;
        }
        self.follow_for_predictions(path, cx);
        let Some((editor, target)) = self.prediction_target() else { return };
        if target != path {
            return;
        }
        // Typed what's predicted: the prediction, and its stream, go on.
        if editor.read(cx).edit_prediction_id().is_some() {
            return;
        }
        if self.predictions.settings.automatic {
            let delay = Duration::from_millis(self.predictions.settings.debounce_ms);
            self.request_prediction(editor, target, delay, false, cx);
        } else {
            self.cancel_prediction_request();
        }
    }

    /// The editor notified (a caret move, anything): a request for a caret
    /// that has moved is no use.
    pub(super) fn predictions_editor_changed(&mut self, editor: &Entity<EditorState>, cx: &mut Context<Self>) {
        let Some(pending) = &self.predictions.pending else { return };
        if pending.view == editor.entity_id() && !pending.answered && editor.read(cx).cursor() != pending.cursor {
            self.cancel_prediction_request();
            cx.notify();
        }
    }

    fn cancel_prediction_request(&mut self) {
        if let Some(pending) = self.predictions.pending.take()
            && !pending.answered
        {
            if pending.sent {
                self.predictions.stats.cancelled += 1;
            }
            if self.predictions.status == Status::Requesting {
                self.predictions.status = Status::Idle;
            }
        }
    }

    /// Editor ▸ Show Predictive Completion (⌃⌥Space): ask now.
    pub(super) fn show_predictive_completion(&mut self, _: &ShowPredictiveCompletion, window: &mut Window, cx: &mut Context<Self>) {
        if !self.predictions.on() {
            self.notify_error(
                "Predictive completion is off: set \"edit_predictions\": { \"provider\": \"claude\" } (or \"lsp\") in your settings.".into(),
                window,
                cx,
            );
            return;
        }
        let Some((editor, path)) = self.prediction_target() else { return };
        self.follow_for_predictions(&path, cx);
        self.request_prediction(editor, path, Duration::ZERO, true, cx);
    }

    /// Tab while a prediction shows: type it (and otherwise what Tab does).
    pub(super) fn accept_edit_prediction(&mut self, _: &AcceptEditPrediction, window: &mut Window, cx: &mut Context<Self>) {
        let accepted = self.active_editor().is_some_and(|editor| editor.update(cx, |state, cx| state.accept_edit_prediction(window, cx)));
        if accepted {
            self.predictions.stats.accepted += 1;
            self.predictions.pending = None;
            cx.notify();
        } else {
            self.tab_or_placeholder(&TabOrPlaceholder, window, cx);
        }
    }

    /// ⌥] (and Escape, in the editor): drop the prediction.
    pub(super) fn dismiss_edit_prediction(&mut self, _: &DismissEditPrediction, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |state, cx| state.dismiss_edit_prediction(cx));
        }
        self.predictions.pending = None;
        cx.notify();
    }

    fn request_prediction(&mut self, editor: Entity<EditorState>, path: PathBuf, delay: Duration, invoked: bool, cx: &mut Context<Self>) {
        self.cancel_prediction_request();
        let view = editor.entity_id();
        let cursor = editor.read(cx).cursor();
        let weak = editor.downgrade();
        let task = cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let Ok(Some(source)) = this.update(cx, |this, cx| this.start_prediction(&weak, &path, invoked, cx)) else { return };
            match source {
                Source::Claude(mut stream, asked) => {
                    let mut shown: Option<EditPredictionId> = None;
                    let mut text = String::new();
                    while let Some(update) = stream.updates.next().await {
                        let go_on = this
                            .update(cx, |this, cx| this.prediction_update(&weak, &asked, &mut shown, &mut text, update, cx))
                            .unwrap_or(false);
                        if !go_on {
                            break;
                        }
                    }
                }
                Source::Lsp(response, asked) => {
                    let update = match response.await {
                        Ok(value) => match predictions::lsp::parse(&value, &asked.text, asked.cursor) {
                            Some((range, text)) => Update::Replace(range, text),
                            None => Update::Done,
                        },
                        Err(err) => Update::Failed(format!("{err:#}")),
                    };
                    let (mut shown, mut text) = (None, String::new());
                    this.update(cx, |this, cx| {
                        if this.prediction_update(&weak, &asked, &mut shown, &mut text, update, cx) {
                            this.prediction_update(&weak, &asked, &mut shown, &mut text, Update::Done, cx);
                        }
                    })
                    .ok();
                }
            }
        });
        self.predictions.pending = Some(Pending { view, cursor, answered: false, sent: false, _task: task });
    }

    /// What to ask about, and the request: `None` when there's nothing to
    /// ask (a read-only file, Vim's normal mode, an excluded file).
    fn start_prediction(&mut self, editor: &WeakEntity<EditorState>, path: &Path, invoked: bool, cx: &mut Context<Self>) -> Option<Source> {
        let editor = editor.upgrade()?;
        let state = editor.read(cx);
        let vim_mode = state.vim_status().map(|status| status.mode);
        if !state.is_editable() || vim_mode.is_some_and(|mode| mode != VimMode::Insert) || !state.selected_range().is_empty() {
            self.predictions.pending = None;
            return None;
        }
        if predictions::disabled(path, &self.project.root, &self.predictions.settings.disabled_globs) {
            self.predictions.status = Status::Excluded;
            self.predictions.pending = None;
            cx.notify();
            return None;
        }
        let request = PredictionRequest {
            path: path.to_path_buf(),
            display_path: self.display_path(path),
            language: self.buffer(path).map(|b| b.language.to_string()).unwrap_or_default(),
            text: state.text().clone(),
            cursor: state.cursor(),
            recent_edits: self.predictions.recent.list(),
            invoked,
        };
        let revision = state.text_revision();
        let settings = &self.predictions.settings;
        let source = match settings.provider {
            PredictionProvider::None => return None,
            PredictionProvider::Claude => {
                self.predictions.last_request = Some(serde_json::json!({
                    "provider": "claude",
                    "model": settings.claude.model,
                    "path": request.display_path,
                    "cursor": request.cursor,
                    "recent_edits": request.recent_edits.len(),
                    "invoked": invoked,
                }));
                Source::Claude(claude::predict(&request, &settings.claude), Asked { revision, ..Asked::from(request) })
            }
            PredictionProvider::Lsp => {
                // The server answers about the text it was last sent: send this.
                let text = request.text.clone();
                let server = match self.lsp.read(cx).features().inline_completion {
                    true => self.lsp.update(cx, |lsp, cx| lsp.synced_server_with_text(path, &text, cx)),
                    false => None,
                };
                let Some(server) = server else {
                    self.predictions.status = Status::Failed("the language server has no inline completion".into());
                    self.predictions.pending = None;
                    cx.notify();
                    return None;
                };
                let params = predictions::lsp::params(path, &request.text, request.cursor, invoked);
                self.predictions.last_request = Some(serde_json::json!({ "provider": "lsp", "path": request.display_path, "cursor": request.cursor, "invoked": invoked }));
                Source::Lsp(server.request_raw(predictions::lsp::METHOD, params).boxed(), Asked { revision, ..Asked::from(request) })
            }
        };
        self.predictions.stats.requests += 1;
        if let Some(pending) = self.predictions.pending.as_mut() {
            pending.sent = true;
        }
        self.predictions.status = Status::Requesting;
        cx.notify();
        Some(source)
    }

    /// A piece of an answer: shown, or the request is over. Returns whether
    /// to keep reading.
    fn prediction_update(
        &mut self,
        editor: &WeakEntity<EditorState>,
        asked: &Asked,
        shown: &mut Option<EditPredictionId>,
        text: &mut String,
        update: Update,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = editor.upgrade() else { return false };
        let (go_on, done) = match update {
            Update::Text(so_far) => {
                *text = so_far;
                (self.show_prediction(&editor, asked, shown, EditPrediction::insertion(asked.cursor, text.clone()), cx), false)
            }
            Update::Replace(range, new) => {
                *text = new.clone();
                (self.show_prediction(&editor, asked, shown, EditPrediction::replacement(range, new), cx), false)
            }
            Update::Done => {
                // The whole answer, tidied: what repeats the line's rest goes.
                if let Some(id) = *shown {
                    let cleaned = predictions::clean(text, &asked.rest_of_line);
                    editor.update(cx, |state, cx| match cleaned {
                        Some(cleaned) if cleaned != *text => {
                            state.update_edit_prediction(id, cleaned, cx);
                        }
                        Some(_) => {}
                        None => {
                            state.dismiss_edit_prediction(cx);
                        }
                    });
                }
                self.predictions.status = Status::Idle;
                (false, true)
            }
            Update::Failed(message) => {
                self.predictions.stats.failed += 1;
                self.predictions.status = Status::Failed(message);
                if let Some(id) = *shown {
                    editor.update(cx, |state, cx| {
                        if state.edit_prediction_id() == Some(id) {
                            state.dismiss_edit_prediction(cx);
                        }
                    });
                }
                (false, true)
            }
        };
        if done || !go_on {
            // The task ends with this; it's no longer something to cancel.
            if let Some(pending) = self.predictions.pending.as_mut()
                && pending.view == editor.entity_id()
            {
                pending.answered = true;
            }
            if !done && self.predictions.status == Status::Requesting {
                self.predictions.status = Status::Idle;
            }
        }
        cx.notify();
        go_on
    }

    /// Puts `prediction` on screen (or more of it); false when the text or
    /// caret moved on, or the editor dropped it.
    fn show_prediction(
        &mut self,
        editor: &Entity<EditorState>,
        asked: &Asked,
        shown: &mut Option<EditPredictionId>,
        prediction: EditPrediction,
        cx: &mut Context<Self>,
    ) -> bool {
        match *shown {
            Some(id) => editor.update(cx, |state, cx| state.update_edit_prediction(id, prediction.text, cx)),
            None => {
                let state = editor.read(cx);
                if state.text_revision() != asked.revision || state.cursor() != asked.cursor {
                    return false;
                }
                if prediction.text.trim().is_empty() {
                    // Nothing worth showing yet (a line break, indentation).
                    return true;
                }
                *shown = editor.update(cx, |state, cx| state.set_edit_prediction(prediction, cx));
                if shown.is_some() {
                    self.predictions.stats.shown += 1;
                    if let Some(pending) = self.predictions.pending.as_mut() {
                        pending.answered = true;
                    }
                }
                shown.is_some()
            }
        }
    }

    /// For the status bar: an icon, a label and whether it's a problem;
    /// `None` while predictions are off.
    pub(super) fn prediction_indicator(&self, cx: &App) -> Option<(bool, String, bool)> {
        if !self.predictions.on() {
            return None;
        }
        let shown = self.active_editor().is_some_and(|editor| editor.read(cx).has_edit_prediction());
        Some(match &self.predictions.status {
            Status::Requesting => (true, "Predicting…".into(), false),
            Status::Failed(message) => (false, format!("Predictions: {message}"), true),
            Status::Excluded => (false, "Predictions: not for this file".into(), false),
            Status::Idle if shown => (false, "Prediction: ⇥ to accept".into(), false),
            Status::Idle if !self.predictions.settings.automatic => (false, "Predictions: ⌃⌥Space".into(), false),
            Status::Idle => (false, "Predictions".into(), false),
        })
    }

    /// The status bar indicator's menu.
    pub(super) fn show_predictions_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let automatic = self.predictions.settings.automatic;
        let provider = match self.predictions.settings.provider {
            PredictionProvider::Claude => format!("Claude ({})", self.predictions.settings.claude.model),
            PredictionProvider::Lsp => "Language server".into(),
            PredictionProvider::None => "Off".into(),
        };
        let workspace = cx.weak_entity();
        self.show_popup_menu(position, window, cx, move |menu, _, _| {
            let toggle = workspace.clone();
            menu.item(PopupMenuItem::new(format!("Provider: {provider}")).disabled(true))
                .separator()
                .item(PopupMenuItem::new("Show Predictive Completion").on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(ShowPredictiveCompletion), cx)
                }))
                .item(PopupMenuItem::new("Predict While Typing").checked(automatic).on_click(move |_, window, cx| {
                    toggle
                        .update(cx, |this, cx| {
                            this.set_setting(&["edit_predictions", "automatic"], serde_json::json!(!automatic), window, cx)
                        })
                        .ok();
                }))
                .item(PopupMenuItem::new("Open Settings").on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::OpenSettings), cx)
                }))
        });
    }

    /// For the control channel: `status`, `request` (⌃⌥Space), `accept`,
    /// `dismiss`, `edits` (the recent edits a request would carry).
    pub fn prediction_command(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        match arg.trim() {
            "" | "status" => Ok(self.predictions_summary(cx).to_string()),
            "request" => {
                self.show_predictive_completion(&ShowPredictiveCompletion, window, cx);
                Ok(String::new())
            }
            "accept" => {
                self.accept_edit_prediction(&AcceptEditPrediction, window, cx);
                Ok(String::new())
            }
            "dismiss" => {
                self.dismiss_edit_prediction(&DismissEditPrediction, window, cx);
                Ok(String::new())
            }
            "edits" => Ok(serde_json::to_string(
                &self.predictions.recent.list().iter().map(|e| serde_json::json!({ "path": e.path, "row": e.row, "old": e.old, "new": e.new })).collect::<Vec<_>>(),
            )
            .unwrap_or_default()),
            other => Err(format!("usage: predict status|request|accept|dismiss|edits (not “{other}”)")),
        }
    }

    pub(super) fn predictions_summary(&self, cx: &App) -> serde_json::Value {
        let editor = self.active_editor();
        let shown = editor.as_ref().and_then(|editor| editor.read(cx).edit_prediction().cloned());
        // How the editor draws it: inlays and blocks, never the buffer.
        let ghost = editor.map(|editor| {
            let state = editor.read(cx);
            serde_json::json!({
                "inlays": state.inlays("edit-prediction").iter().map(|i| serde_json::json!([i.offset, i.text.to_string()])).collect::<Vec<_>>(),
                "blocks": state.blocks("edit-prediction").iter().map(|b| serde_json::json!([b.row, b.lines.iter().map(|l| l.to_string()).collect::<Vec<_>>()])).collect::<Vec<_>>(),
                "struck": state.highlight_layer_ranges("edit-prediction").iter().map(|r| [r.start, r.end]).collect::<Vec<_>>(),
            })
        });
        let stats = &self.predictions.stats;
        serde_json::json!({
            "provider": self.predictions.settings.provider.name(),
            "automatic": self.predictions.settings.automatic,
            "status": match &self.predictions.status {
                Status::Idle => "idle".to_string(),
                Status::Requesting => "requesting".to_string(),
                Status::Failed(message) => format!("failed: {message}"),
                Status::Excluded => "excluded".to_string(),
            },
            "pending": self.predictions.pending.is_some(),
            "following": self.predictions.shadows.len(),
            "shown": shown.map(|p| serde_json::json!({ "start": p.range.start, "end": p.range.end, "text": p.text })),
            "ghost": ghost,
            "indicator": self.prediction_indicator(cx).map(|(_, label, _)| label),
            "requests": stats.requests,
            "cancelled": stats.cancelled,
            "shown_count": stats.shown,
            "accepted": stats.accepted,
            "failed": stats.failed,
            "last_request": self.predictions.last_request,
        })
    }
}

/// What a request asked, to tell whether its answer still fits.
struct Asked {
    cursor: usize,
    revision: u64,
    /// The cursor's line after it, which a prediction shouldn't repeat.
    rest_of_line: String,
    text: Rope,
}

impl From<PredictionRequest> for Asked {
    fn from(request: PredictionRequest) -> Self {
        let row = request.text.offset_to_point(request.cursor).row;
        let end = request.text.line_end_offset(row).max(request.cursor);
        let rest_of_line = request.text.slice(request.cursor..end).to_string();
        Self { cursor: request.cursor, revision: 0, rest_of_line, text: request.text }
    }
}

enum Source {
    Claude(claude::Stream, Asked),
    /// Dropping it sends `$/cancelRequest`.
    Lsp(futures::future::BoxFuture<'static, anyhow::Result<serde_json::Value>>, Asked),
}

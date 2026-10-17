//! Semantic highlighting: sourcekit-lsp's semantic tokens laid over
//! tree-sitter's colors as a highlight layer in every view of a file, so a
//! type reads as a type at a call (`Greeter(name:)`), a property as a
//! property and a parameter as a parameter, as in Xcode.
//!
//! A buffer asks for its whole document (sourcekit-lsp answers a 6,000-line
//! file in a few milliseconds) when it opens, when the server says its
//! answers changed, and a moment after typing stops; a new request replaces
//! (and cancels) the one before, and an answer for text that has changed
//! since is dropped. Meanwhile the layer's ranges move with the edits.

use std::path::Path;
use std::time::Duration;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use serde_json::json;
use zcode_editor::{EditorState, HighlightPriority};

use super::Workspace;
use crate::lsp::{offset_to_lsp, path_to_uri, semantic_tokens};

const KEY: &str = "semantic-tokens";
/// How long typing pauses before a buffer asks again.
const AFTER_EDIT: Duration = Duration::from_millis(300);

/// A buffer's semantic tokens: the request in flight and how many tokens
/// it shows.
#[derive(Default)]
pub struct BufferTokens {
    task: Option<Task<()>>,
    pub count: Option<usize>,
}

impl Workspace {
    /// Asks again for every open file.
    pub(super) fn refresh_semantic_tokens(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<_> = self.buffers.iter().map(|b| b.path.clone()).collect();
        for path in paths {
            self.schedule_semantic_tokens(&path, Duration::ZERO, cx);
        }
    }

    pub(super) fn semantic_tokens_edited(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.schedule_semantic_tokens(path, AFTER_EDIT, cx);
    }

    /// The server published diagnostics for `path`, so it has the file's
    /// build settings: a file that has no tokens yet asks now.
    pub(super) fn semantic_tokens_diagnosed(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.buffer(path).is_some_and(|b| b.tokens.count.is_none_or(|count| count == 0) && b.tokens.task.is_none()) {
            self.schedule_semantic_tokens(path, Duration::ZERO, cx);
        }
    }

    pub(super) fn schedule_semantic_tokens(&mut self, path: &Path, delay: Duration, cx: &mut Context<Self>) {
        if !self.lsp.read(cx).document_features(path).iter().any(|f| f.semantic_tokens.is_some()) {
            return;
        }
        let Some(ix) = self.buffers.iter().position(|b| b.path == path) else { return };
        let path = path.to_path_buf();
        let task = cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let request = this.update(cx, |this, cx| {
                let text = this.buffer(&path)?.editor.read(cx).text().clone();
                let (server, features) = this.lsp.update(cx, |lsp, _| lsp.synced_server_with(&path, &text, |f| f.semantic_tokens.is_some()))?;
                let legend = features.semantic_tokens?;
                let version = this.lsp.read(cx).document_version(&path)?;
                let document = json!({ "uri": path_to_uri(&path).as_str() });
                let request = if features.semantic_tokens_full {
                    server.request_raw("textDocument/semanticTokens/full", json!({ "textDocument": document }))
                } else {
                    let range = lsp_types::Range::new(lsp_types::Position::new(0, 0), offset_to_lsp(&text, text.len()));
                    server.request_raw("textDocument/semanticTokens/range", json!({ "textDocument": document, "range": range }))
                };
                Some((request, text, version, legend))
            });
            let Ok(Some((request, text, version, legend))) = request else {
                this.update(cx, |this, _| this.semantic_tokens_done(&path)).ok();
                return;
            };
            // Dropping this task (a newer request replaced it) cancels the request.
            let response = request.await;
            let theme = this.update(cx, |_, cx| cx.theme().highlight_theme.clone()).ok();
            let (Ok(response), Some(theme)) = (response, theme) else {
                this.update(cx, |this, _| this.semantic_tokens_done(&path)).ok();
                return;
            };
            let styles = cx
                .background_spawn(async move {
                    let data: Vec<u32> = response
                        .get("data")
                        .and_then(|data| serde_json::from_value(data.clone()).ok())
                        .unwrap_or_default();
                    semantic_tokens::decode(&text, &data, &legend)
                        .into_iter()
                        .filter_map(|(range, name)| Some((range, theme.style.syntax.style(name)?)))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.semantic_tokens_done(&path);
                // Typed since: the tokens are for other text; the next request has it.
                if this.lsp.read(cx).document_version(&path) != Some(version) {
                    return;
                }
                let count = styles.len();
                for editor in this.token_views(&path, cx) {
                    editor.update(cx, |state, cx| state.set_highlight_layer(KEY, HighlightPriority::SEMANTIC_TOKENS, styles.clone(), cx));
                }
                if let Some(buffer) = this.buffers.iter_mut().find(|b| b.path == path) {
                    buffer.tokens.count = Some(count);
                }
            })
            .ok();
        });
        self.buffers[ix].tokens.task = Some(task);
    }

    fn semantic_tokens_done(&mut self, path: &Path) {
        if let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) {
            buffer.tokens.task = None;
        }
    }

    /// Every editor showing `path`'s text: its panes' views and the local
    /// side of its comparisons.
    fn token_views(&self, path: &Path, cx: &App) -> Vec<Entity<EditorState>> {
        let mut views: Vec<Entity<EditorState>> = self.views_of(path).iter().map(|item| item.editor.clone()).collect();
        views.extend(self.diff_views_of(path).iter().map(|diff| diff.read(cx).current.clone()));
        views
    }

    /// The control channel's view of the active editor's tokens.
    pub(super) fn semantic_tokens_summary(&self, cx: &App) -> serde_json::Value {
        let Some(editor) = self.active_editor() else { return serde_json::Value::Null };
        let state = editor.read(cx);
        let text = state.text().to_string();
        let ranges = state.highlight_layer_ranges(KEY);
        json!({
            "count": ranges.len(),
            // The first tokens, as text, for checks.
            "first": ranges.iter().take(40).filter_map(|r| text.get(r.clone())).collect::<Vec<_>>(),
        })
    }
}

//! Blame: who last changed each line. Inline blame (Zed's) shows the
//! author, how long ago and the commit's summary after the end of the
//! cursor's line, a moment after the cursor rests there; Editor ▸ Authors
//! (Xcode's, ⌃⇧⌘A) shows each run of lines' author and date in a column
//! of the gutter, where a click shows the commit.
//!
//! The blame is of the editor's text (`git blame --contents`), so lines
//! edited and not committed say so; it is read in the background a second
//! after the last edit, and inline blame hides while you type.

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zcode_editor::{EditorState, GutterItem, GutterLane, GutterPlacement, Inlay, RopeExt as _};

use super::Workspace;
use crate::actions::PickerDismiss;
use crate::git::{self, blame::Commit};

/// The gutter lane of Editor ▸ Authors.
pub(super) const AUTHORS_LANE: &str = "git-authors";
const AUTHORS_WIDTH: f32 = 190.;
/// The inlay group of inline blame.
const INLINE_KEY: &str = "inline-blame";
/// From the last edit to reading the blame again.
const BLAME_DELAY: Duration = Duration::from_millis(1000);
/// From the cursor resting on a line to its inline blame.
const INLINE_DELAY: Duration = Duration::from_millis(150);

/// Where inline blame is shown.
#[derive(Default)]
pub(super) struct InlineBlame {
    view: Option<WeakEntity<EditorState>>,
    row: Option<u32>,
    /// What it says, once shown.
    pub text: Option<String>,
    task: Option<Task<()>>,
}

impl Workspace {
    fn wants_blame(&self) -> bool {
        self.git.inline_blame || self.git.show_authors
    }

    /// Reads the blame of `path`'s text after `delay`, in the background.
    pub(super) fn schedule_blame(&mut self, path: &Path, delay: Duration, cx: &mut Context<Self>) {
        if !self.wants_blame() {
            return;
        }
        let Some((root, rel)) = self.git_rel(path) else { return };
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else { return };
        // Nothing to blame in a file git doesn't have.
        if buffer.git.bases.as_ref().is_none_or(|b| b.head.is_none() && b.index.is_none()) {
            return;
        }
        let text = buffer.editor.read(cx).text().clone();
        let path = path.to_path_buf();
        buffer.git.blame_task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let blame = cx.background_spawn(async move { git::blame(&root, &rel, &text.to_string()).ok() }).await;
            this.update(cx, |this, cx| {
                if let Some(buffer) = this.buffers.iter_mut().find(|b| b.path == path) {
                    buffer.git.blame = blame.map(Rc::new);
                    buffer.git.blame_stale = false;
                }
                this.git.inline.row = None;
                this.refresh_inline_blame(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// After an edit: inline blame hides, and the blame is read again once
    /// typing stops.
    pub(super) fn buffer_edited_for_blame(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else { return };
        if buffer.git.blame.is_none() && buffer.git.blame_task.is_none() {
            return;
        }
        buffer.git.blame_stale = true;
        self.clear_inline_blame(cx);
        self.schedule_blame(path, BLAME_DELAY, cx);
    }

    /// After `HEAD` moved: every blame is read again.
    pub(super) fn invalidate_blame(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<_> = self.buffers.iter_mut().filter(|b| b.git.blame.is_some()).map(|b| {
            b.git.blame_stale = true;
            b.path.clone()
        }).collect();
        for path in paths {
            self.schedule_blame(&path, Duration::ZERO, cx);
        }
    }

    /// Something in an editor changed (the cursor, focus): inline blame
    /// follows the focused editor's cursor line.
    pub(super) fn refresh_inline_blame(&mut self, cx: &mut Context<Self>) {
        let target = self
            .git
            .inline_blame
            .then(|| self.active_item().filter(|item| item.shown_plist().is_none()))
            .flatten()
            .map(|item| (item.path.clone(), item.editor.clone()));
        let Some((path, editor)) = target else {
            self.clear_inline_blame(cx);
            return;
        };
        let row = editor.read(cx).cursor_position().line;
        let same_view = self.git.inline.view.as_ref().and_then(|v| v.upgrade()).is_some_and(|v| v == editor);
        if same_view && self.git.inline.row == Some(row) {
            return;
        }
        self.clear_inline_blame(cx);
        self.git.inline.view = Some(editor.downgrade());
        self.git.inline.row = Some(row);
        let buffer = self.buffer(&path);
        if buffer.is_some_and(|b| b.git.blame.is_none() && b.git.blame_task.is_none()) {
            self.schedule_blame(&path, Duration::ZERO, cx);
            return;
        }
        self.git.inline.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INLINE_DELAY).await;
            this.update(cx, |this, cx| this.show_inline_blame(&path, &editor, row, cx)).ok();
        }));
    }

    fn show_inline_blame(&mut self, path: &Path, editor: &Entity<EditorState>, row: u32, cx: &mut Context<Self>) {
        if self.git.inline.row != Some(row) {
            return;
        }
        let Some(buffer) = self.buffer(path).filter(|b| !b.git.blame_stale) else { return };
        let Some(commit) = buffer.git.blame.as_ref().and_then(|b| b.commit(row)).filter(|c| !c.uncommitted) else { return };
        let text = format!("{}, {} • {}", self.author_name(commit), git::blame::relative_time(commit.time, git::blame::now()), commit.summary);
        let color = cx.theme().muted_foreground.opacity(0.75);
        let shown = format!("      {text}");
        editor.update(cx, |state, cx| {
            let end = state.text().line_end_offset(row as usize);
            state.set_inlays(INLINE_KEY, vec![Inlay::new(end, shown).color(color)], cx);
        });
        self.git.inline.text = Some(text);
    }

    fn clear_inline_blame(&mut self, cx: &mut Context<Self>) {
        let inline = std::mem::take(&mut self.git.inline);
        if let Some(view) = inline.view.and_then(|v| v.upgrade())
            && inline.text.is_some()
        {
            view.update(cx, |state, cx| state.clear_inlays(INLINE_KEY, cx));
        }
    }

    /// "You" for the user's own commits.
    fn author_name<'a>(&self, commit: &'a Commit) -> &'a str {
        if self.git.user_email.as_deref() == Some(commit.email.as_str()) { "You" } else { &commit.author }
    }

    /// Editor ▸ Inline Blame (`git.inline_blame.enabled`), as the settings
    /// changed.
    pub(super) fn set_inline_blame(&mut self, on: bool, cx: &mut Context<Self>) {
        self.git.inline_blame = on;
        self.git.inline.row = None;
        self.refresh_inline_blame(cx);
        cx.notify();
    }

    /// Editor ▸ Authors (`git.show_authors`): the authors column in every
    /// editor.
    pub(super) fn set_authors(&mut self, on: bool, cx: &mut Context<Self>) {
        self.git.show_authors = on;
        let views: Vec<Entity<EditorState>> =
            self.panes.iter().flat_map(|p| p.items.iter().filter_map(super::Item::editor)).map(|item| item.editor.clone()).collect();
        for view in views {
            if self.git.show_authors {
                let lane = self.authors_lane(&view, cx);
                view.update(cx, |state, cx| state.set_gutter_lane(lane, cx));
            } else {
                view.update(cx, |state, cx| state.remove_gutter_lane(AUTHORS_LANE, cx));
            }
        }
        if self.git.show_authors {
            let paths: Vec<_> = self.buffers.iter().filter(|b| b.git.blame.is_none()).map(|b| b.path.clone()).collect();
            for path in paths {
                self.schedule_blame(&path, Duration::ZERO, cx);
            }
        }
        cx.notify();
    }

    /// The authors column: each run of lines from one commit gets its
    /// author and date on its first line, and a bar whose strength is the
    /// commit's age.
    pub(super) fn authors_lane(&self, editor: &Entity<EditorState>, cx: &Context<Self>) -> GutterLane {
        let workspace = cx.weak_entity();
        let editor_id = editor.entity_id();
        GutterLane::new(AUTHORS_LANE, move |rows, cx| {
            let Some(this) = workspace.upgrade() else { return vec![] };
            let this = this.read(cx);
            let Some(path) = this.buffer_path_of(editor_id) else { return vec![] };
            let Some(blame) = this.buffer(&path).and_then(|b| b.git.blame.clone()) else { return vec![] };
            let theme = cx.theme();
            let (accent, text, muted, border) = (theme.primary, theme.foreground, theme.muted_foreground, theme.border);
            let now = git::blame::now();
            let mut items = vec![];
            for row in rows.rows.clone() {
                let Some(&ix) = blame.lines.get(row as usize) else { break };
                let commit = blame.commits[ix as usize].clone();
                let first = row == rows.rows.start || blame.lines.get(row as usize - 1) != Some(&ix);
                let label = first.then(|| {
                    let when = if commit.uncommitted { String::new() } else { git::blame::relative_time(commit.time, now) };
                    (this.author_name(&commit).to_string(), when)
                });
                // Newer commits have a stronger bar.
                let age_days = ((now - commit.time).max(0) / 86_400) as f32;
                let strength = if commit.uncommitted { 1. } else { (1. - age_days / 365.).clamp(0.15, 1.) };
                let separator = first && row > 0;
                let workspace = workspace.clone();
                let path = path.clone();
                items.push(
                    GutterItem::new(row, move |paint, window, cx| {
                        let cell = paint.cell;
                        window.paint_quad(fill(Bounds::new(cell.origin, size(px(3.), cell.size.height)), accent.opacity(strength)));
                        if separator {
                            window.paint_quad(fill(Bounds::new(cell.origin, size(cell.size.width - px(4.), px(1.))), border));
                        }
                        if let Some((author, when)) = &label {
                            let (left, right) = (cell.left() + px(8.), cell.right() - px(8.));
                            let date = paint.paint_label(when, left + px(100.)..right, true, 0.85, muted, window, cx);
                            paint.paint_label(author, left..right - date - px(8.), false, 0.85, text, window, cx);
                        }
                    })
                    .cursor(CursorStyle::PointingHand)
                    .on_click(move |event, window, cx| {
                        let position = point(event.position.x, event.bounds.bottom());
                        let (workspace, path, commit) = (workspace.clone(), path.clone(), commit.clone());
                        window.defer(cx, move |window, cx| {
                            workspace.update(cx, |this, cx| this.show_commit_popover(&path, commit, position, window, cx)).ok();
                        });
                        true
                    }),
                );
            }
            items
        })
        .order(-1)
        .placement(GutterPlacement::BeforeNumbers(px(AUTHORS_WIDTH)))
    }

    fn show_commit_popover(&mut self, path: &Path, commit: Commit, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((root, _)) = self.git_rel(path) else { return };
        let author = self.author_name(&commit).to_string();
        let popover = cx.new(|cx| CommitPopover::new(root, commit, author, cx));
        self.show_modal_at(popover, Some(position), window, cx);
    }

    /// The control channel's view of blame in the active file.
    pub(super) fn blame_summary(&self, cx: &App) -> serde_json::Value {
        let Some(buffer) = self.active_buffer() else { return serde_json::Value::Null };
        let blame = buffer.git.blame.as_ref();
        let authors: Vec<serde_json::Value> = self
            .painted_gutter(AUTHORS_LANE, cx)
            .into_iter()
            .filter_map(|(line, bounds)| {
                let blame = blame?;
                let row = line - 1;
                let ix = *blame.lines.get(row as usize)?;
                let first = row == 0 || blame.lines.get(row as usize - 1) != Some(&ix);
                first.then(|| serde_json::json!({ "line": line, "author": blame.commits[ix as usize].author, "x": f32::from(bounds.center().x), "y": f32::from(bounds.center().y) }))
            })
            .collect();
        serde_json::json!({
            "lines": blame.map(|b| b.lines.len()),
            "stale": buffer.git.blame_stale,
            "authors": blame.map(|b| b.lines.iter().map(|&ix| b.commits[ix as usize].author.clone()).collect::<Vec<_>>()),
            "inline": self.git.inline.text.as_ref().map(|text| serde_json::json!({ "line": self.git.inline.row.map(|r| r + 1), "text": text })),
            "painted_authors": authors,
        })
    }
}

/// A commit, from the Authors column: who, when, and its message.
pub struct CommitPopover {
    focus_handle: FocusHandle,
    commit: Commit,
    author: String,
    message: Option<String>,
    _task: Task<()>,
}

impl EventEmitter<DismissEvent> for CommitPopover {}

impl Focusable for CommitPopover {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CommitPopover {
    fn new(root: std::path::PathBuf, commit: Commit, author: String, cx: &mut Context<Self>) -> Self {
        let sha = commit.sha.clone();
        let uncommitted = commit.uncommitted;
        let task = cx.spawn(async move |this, cx| {
            if uncommitted {
                return;
            }
            let message = cx.background_spawn(async move { git::commit_message(&root, &sha) }).await;
            this.update(cx, |this, cx| {
                this.message = message;
                cx.notify();
            })
            .ok();
        });
        Self { focus_handle: cx.focus_handle(), commit, author, message: None, _task: task }
    }

    fn dismiss(&mut self, _: &PickerDismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }
}

impl Render for CommitPopover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let commit = &self.commit;
        let message = self.message.clone().unwrap_or_else(|| commit.summary.clone());
        let sha = commit.sha.clone();
        v_flex()
            .id("commit-popover")
            .key_context("CommitPopover")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::dismiss))
            .w(px(380.))
            .p_3()
            .gap_1()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .text_sm()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(if commit.uncommitted { "Not Committed Yet".to_string() } else { self.author.clone() }))
            .when(!commit.uncommitted, |el| {
                el.child(div().text_xs().text_color(theme.muted_foreground).child(format!(
                    "{} · {} · {}",
                    commit.email,
                    git::blame::absolute_time(commit.time),
                    git::blame::relative_time(commit.time, git::blame::now())
                )))
                .child(div().pt_1().whitespace_normal().child(message))
                .child(
                    h_flex()
                        .pt_1()
                        .gap_2()
                        .child(div().flex_1().text_xs().font_family(theme.mono_font_family.clone()).text_color(theme.muted_foreground).child(commit.short_sha().to_string()))
                        .child(Button::new("commit-copy").label("Copy Commit ID").xsmall().ghost().on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()));
                        })),
                )
            })
    }
}

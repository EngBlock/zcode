//! Xcode's Code Review (Editor ▸ Code Review, ⌥⇧⌘↩) as a tab: a file's
//! local changes against `HEAD`, side by side or inline (Editor ▸ Side by
//! Side Comparison / Inline Comparison).
//!
//! The local side is an editor linked to the file's buffer
//! (`EditorState::link`), so it can be edited, undone and saved like the
//! file's own tab, and the comparison follows as you type. Side by side,
//! `HEAD`'s version is a read-only editor on the left; blank blocks pad
//! whichever side has fewer lines in a change so the two stay row for row,
//! and they scroll together. Inline, deleted lines are blocks under the
//! line before them, or above the first line for lines deleted at the top.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use zcode_editor::{Block, Editor, EditorState, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle, RopeExt as _};

use crate::git::diff::{self, Hunk};

const PAD_KEY: &str = "diff-padding";
const DELETED_KEY: &str = "diff-deleted";
const DIFF_DELAY: Duration = Duration::from_millis(30);

pub struct DiffView {
    pub path: PathBuf,
    /// The file as it is: a view of its buffer.
    pub current: Entity<EditorState>,
    /// `HEAD`'s version, read-only.
    base: Entity<EditorState>,
    base_text: String,
    pub side_by_side: bool,
    pub hunks: Vec<Hunk>,
    font_size: f32,
    current_marks: Option<RangeDecorationCollection>,
    base_marks: Option<RangeDecorationCollection>,
    /// The vertical scroll position both sides were last given.
    synced_y: Pixels,
    diff_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for DiffView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.current.read(cx).focus_handle(cx)
    }
}

impl DiffView {
    pub fn new(
        path: PathBuf,
        current: Entity<EditorState>,
        base: Entity<EditorState>,
        base_text: String,
        side_by_side: bool,
        font_size: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&current, |this, _, cx| this.sync_scroll(true, cx)),
            cx.observe(&base, |this, _, cx| this.sync_scroll(false, cx)),
        ];
        let mut this = Self {
            path,
            current,
            base,
            base_text,
            side_by_side,
            hunks: vec![],
            font_size,
            current_marks: None,
            base_marks: None,
            synced_y: px(0.),
            diff_task: None,
            _subscriptions: subscriptions,
        };
        this.schedule_diff(Duration::ZERO, cx);
        this
    }

    /// `HEAD`'s version changed (a commit, a checkout).
    pub fn set_base(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if text == self.base_text {
            return;
        }
        self.base.update(cx, |state, cx| state.set_value(text.clone(), window, cx));
        self.base_text = text;
        self.schedule_diff(Duration::ZERO, cx);
    }

    /// The file was edited (here or in another pane).
    pub fn edited(&mut self, cx: &mut Context<Self>) {
        self.schedule_diff(DIFF_DELAY, cx);
    }

    pub fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        if self.font_size != size {
            self.font_size = size;
            cx.notify();
        }
    }

    fn schedule_diff(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let base = self.base_text.clone();
        let current = self.current.read(cx).text().clone();
        self.diff_task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let hunks = cx.background_spawn(async move { diff::line_hunks(&base, &current.to_string()) }).await;
            this.update(cx, |this, cx| {
                this.hunks = hunks;
                this.mark(cx);
            })
            .ok();
        }));
    }

    /// Side by Side or Inline.
    pub fn set_side_by_side(&mut self, side_by_side: bool, cx: &mut Context<Self>) {
        if self.side_by_side != side_by_side {
            self.side_by_side = side_by_side;
            self.mark(cx);
        }
    }

    /// Colors the changed lines and lays out the blocks for the mode.
    fn mark(&mut self, cx: &mut Context<Self>) {
        let theme = cx.theme();
        let (added, deleted, padding) = (theme.success.opacity(0.16), theme.danger.opacity(0.16), theme.muted.opacity(0.6));
        let deleted_text = theme.danger;
        let base_lines: Vec<String> = diff::lines(&self.base_text).iter().map(|l| l.trim_end_matches('\n').to_string()).collect();
        let current_text = self.current.read(cx).text().clone();
        let base_rope = self.base.read(cx).text().clone();
        let current_fills: Vec<RangeDecoration> =
            self.hunks.iter().filter_map(|h| rows_range(&current_text, &h.new)).map(|r| fill(r, added)).collect();
        let mut base_fills = vec![];
        let mut base_pads = vec![];
        let mut current_pads = vec![];
        let mut deleted_blocks = vec![];
        for hunk in &self.hunks {
            let (old, new) = (hunk.old.len() as u32, hunk.new.len() as u32);
            if let Some(range) = rows_range(&base_rope, &hunk.old) {
                base_fills.push(fill(range, deleted));
            }
            // Pads go under the change's last row (or the row before it),
            // above the first row for a change at the top.
            let pad = |rows: &std::ops::Range<u32>, n: u32| {
                let lines = vec![""; n as usize];
                match rows.end.checked_sub(1) {
                    Some(row) => Block::new(row, lines),
                    None => Block::new(0, lines).above(),
                }
                .background(padding)
            };
            if new > old {
                base_pads.push(pad(&hunk.old, new - old));
            } else if old > new {
                current_pads.push(pad(&hunk.new, old - new));
            }
            if old > 0 {
                let lines = base_lines[(hunk.old.start as usize).min(base_lines.len())..(hunk.old.end as usize).min(base_lines.len())].to_vec();
                let block = match hunk.new.start.checked_sub(1) {
                    Some(row) => Block::new(row, lines),
                    None => Block::new(0, lines).above(),
                };
                deleted_blocks.push(block.color(deleted_text).background(deleted));
            }
        }
        let side_by_side = self.side_by_side;
        self.current.update(cx, |state, cx| {
            if side_by_side {
                state.clear_blocks(DELETED_KEY, cx);
                state.set_blocks(PAD_KEY, current_pads, cx);
            } else {
                state.clear_blocks(PAD_KEY, cx);
                state.set_blocks(DELETED_KEY, deleted_blocks, cx);
            }
        });
        self.base.update(cx, |state, cx| state.set_blocks(PAD_KEY, base_pads, cx));
        set_marks(&mut self.current_marks, &self.current, current_fills, cx);
        set_marks(&mut self.base_marks, &self.base, base_fills, cx);
        cx.notify();
    }

    /// One side scrolled: the other follows (side by side).
    fn sync_scroll(&mut self, from_current: bool, cx: &mut Context<Self>) {
        if !self.side_by_side {
            return;
        }
        let (source, target) = if from_current { (&self.current, &self.base) } else { (&self.base, &self.current) };
        let y = source.read(cx).scroll_offset().y;
        if y == self.synced_y {
            return;
        }
        self.synced_y = y;
        let x = target.read(cx).scroll_offset().x;
        target.update(cx, |state, cx| state.set_scroll_offset(point(x, y), cx));
    }

    /// The next or previous change from the cursor, wrapping around.
    pub fn jump_to_change(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.current.read(cx).cursor_position().line;
        let starts: Vec<u32> = self.hunks.iter().map(|h| h.new.start).collect();
        let target = if forward {
            starts.iter().find(|&&s| s > row).or(starts.first())
        } else {
            starts.iter().rev().find(|&&s| s < row).or(starts.last())
        };
        let Some(&target) = target else { return };
        self.current.update(cx, |state, cx| {
            let text = state.text();
            let offset = if (target as usize) < text.lines_len() { text.line_start_offset(target as usize) } else { text.len() };
            state.set_selected_range(offset..offset, cx);
            state.focus(window, cx);
        });
    }

    pub fn summary(&self, cx: &App) -> serde_json::Value {
        let rows = |editor: &Entity<EditorState>| {
            editor.read(cx).gutter_geometry().rows.iter().take(80).map(|(row, top, _)| (row + 1, f32::from(*top))).collect::<Vec<_>>()
        };
        serde_json::json!({
            "path": self.path,
            "side_by_side": self.side_by_side,
            "hunks": self.hunks.iter().map(|h| [h.old.start + 1, h.old.end, h.new.start + 1, h.new.end]).collect::<Vec<_>>(),
            "base_text": (self.base_text.len() < 8000).then(|| self.base_text.clone()),
            // 1-based row, lines, and whether they're above the row.
            "current_blocks": self.current.read(cx).blocks(if self.side_by_side { PAD_KEY } else { DELETED_KEY }).iter().map(|b| (b.row + 1, b.lines.len(), b.above)).collect::<Vec<_>>(),
            "base_blocks": self.base.read(cx).blocks(PAD_KEY).iter().map(|b| (b.row + 1, b.lines.len(), b.above)).collect::<Vec<_>>(),
            "current_rows": rows(&self.current),
            "base_rows": rows(&self.base),
            "current_scroll": f32::from(self.current.read(cx).scroll_offset().y),
            "base_scroll": f32::from(self.base.read(cx).scroll_offset().y),
        })
    }
}

/// The byte range of `rows`, whole lines.
fn rows_range(text: &zcode_editor::Rope, rows: &std::ops::Range<u32>) -> Option<std::ops::Range<usize>> {
    if rows.is_empty() {
        return None;
    }
    let lines = text.lines_len();
    let start = text.line_start_offset((rows.start as usize).min(lines.saturating_sub(1)));
    let end = if (rows.end as usize) < lines { text.line_start_offset(rows.end as usize) } else { text.len() };
    (end > start).then_some(start..end)
}

fn fill(range: std::ops::Range<usize>, color: Hsla) -> RangeDecoration {
    RangeDecoration::new(range).with_style(RangeDecorationStyle::Fill).with_color(color)
}

fn set_marks(marks: &mut Option<RangeDecorationCollection>, editor: &Entity<EditorState>, decorations: Vec<RangeDecoration>, cx: &mut App) {
    match marks {
        Some(marks) => marks.set(decorations, cx),
        None => *marks = Some(editor.update(cx, |state, cx| state.create_range_decorations_collection(decorations, cx))),
    }
}

impl DiffView {
    fn editor_element(&self, editor: &Entity<EditorState>, cx: &App) -> Editor {
        Editor::new(editor)
            .bordered(false)
            .p_0()
            .h_full()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(self.font_size))
    }
}

impl Render for DiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = self.hunks.len();
        let caption = |label: &'static str| {
            div().px_2().text_xs().text_color(theme.muted_foreground).border_b_1().border_color(theme.border).child(label)
        };
        let body = if self.side_by_side {
            h_flex()
                .flex_1()
                .min_h_0()
                .child(v_flex().flex_1().min_w_0().h_full().child(caption("HEAD")).child(div().flex_1().min_h_0().child(self.editor_element(&self.base, cx))))
                .child(div().w(px(1.)).h_full().bg(theme.border))
                .child(v_flex().flex_1().min_w_0().h_full().child(caption("Local Changes")).child(div().flex_1().min_h_0().child(self.editor_element(&self.current, cx))))
                .into_any_element()
        } else {
            div().flex_1().min_h_0().child(self.editor_element(&self.current, cx)).into_any_element()
        };
        let side_by_side = self.side_by_side;
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(px(28.))
                    .px_2()
                    .gap_1()
                    .text_xs()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().flex_1().text_color(theme.muted_foreground).child(match count {
                        0 => "No changes against HEAD".to_string(),
                        1 => "1 change against HEAD".to_string(),
                        n => format!("{n} changes against HEAD"),
                    }))
                    .child(Button::new("diff-previous").icon(IconName::ArrowUp).ghost().xsmall().tooltip("Previous Change (⌃⇧\\)").on_click(
                        cx.listener(|this, _, window, cx| this.jump_to_change(false, window, cx)),
                    ))
                    .child(Button::new("diff-next").icon(IconName::ArrowDown).ghost().xsmall().tooltip("Next Change (⌃\\)").on_click(
                        cx.listener(|this, _, window, cx| this.jump_to_change(true, window, cx)),
                    ))
                    .child(
                        Button::new("diff-side-by-side")
                            .icon(IconName::Columns2)
                            .ghost()
                            .xsmall()
                            .selected(side_by_side)
                            .tooltip("Side by Side Comparison")
                            .on_click(cx.listener(|this, _, _, cx| this.set_side_by_side(true, cx))),
                    )
                    .child(
                        Button::new("diff-inline")
                            .icon(IconName::Rows2)
                            .ghost()
                            .xsmall()
                            .selected(!side_by_side)
                            .tooltip("Inline Comparison")
                            .on_click(cx.listener(|this, _, _, cx| this.set_side_by_side(false, cx))),
                    ),
            )
            .child(body)
    }
}

impl super::Workspace {
    /// Editor ▸ Code Review: the focused tab's file compared with `HEAD` in
    /// a tab of its own, or, from that tab, back to the file.
    pub fn toggle_code_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.active_pane;
        let shown = self.pane(pane).and_then(|p| p.active_item()).and_then(|item| match item {
            super::Item::Diff(diff) => Some(diff.path.clone()),
            _ => None,
        });
        match shown {
            Some(path) => {
                if let Some(ix) = self.pane(pane).map(|p| p.active) {
                    self.close_item(pane, ix, window, cx);
                }
                self.open_file(path, None, true, window, cx);
            }
            None => {
                if let Some(path) = self.active_item().map(|item| item.path.clone()) {
                    self.open_diff(path, window, cx);
                }
            }
        }
    }

    /// Opens (or shows) the comparison of `path` with `HEAD` in the focused pane.
    pub fn open_diff(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let path = path.canonicalize().unwrap_or(path);
        let pane = self.active_pane;
        if let Some(ix) = self.pane(pane).and_then(|p| p.items.iter().position(|item| matches!(item, super::Item::Diff(d) if d.path == path))) {
            self.activate_item(pane, ix, window, cx);
            return;
        }
        if self.git_rel(&path).is_none() {
            self.notify_error(format!("“{}” isn’t in a Git repository.", super::pane::file_title(&path)), window, cx);
            return;
        }
        if self.buffer(&path).is_none() && !self.open_buffer(&path, window, cx) {
            return;
        }
        let Some(buffer) = self.buffer(&path) else { return };
        let (shared, read_only) = (buffer.editor.clone(), buffer.read_only);
        let base_text = buffer.git.bases.as_ref().and_then(|b| b.head.clone()).unwrap_or_default();
        let language = Self::language_for(&path);
        let current = cx.new(|cx| Self::new_editor(false, language, window, cx));
        EditorState::link(&shared, &current, window, cx);
        current.update(cx, |state, cx| state.set_read_only(read_only, cx));
        let base = cx.new(|cx| Self::new_editor(false, language, window, cx).default_value(base_text.clone()));
        base.update(cx, |state, cx| state.set_read_only(true, cx));
        let (side_by_side, font_size) = (self.git.side_by_side, self.font_size);
        let view = cx.new(|cx| DiffView::new(path.clone(), current.clone(), base, base_text, side_by_side, font_size, cx));
        let view_id = view.entity_id();
        let focus = current.read(cx).focus_handle(cx);
        let subscriptions = vec![
            cx.observe(&view, |_, _, cx| cx.notify()),
            cx.on_focus_in(&focus, window, move |this, window, cx| this.view_focused(view_id, window, cx)),
        ];
        let Some(p) = self.pane_mut(pane) else { return };
        let ix = (p.active + 1).min(p.items.len());
        p.items.insert(ix, super::Item::Diff(super::pane::DiffItem { path, view, _subscriptions: subscriptions }));
        self.activate_item(pane, ix, window, cx);
    }

    /// Every open comparison of `path`.
    pub(super) fn diff_views_of(&self, path: &std::path::Path) -> Vec<Entity<DiffView>> {
        self.panes
            .iter()
            .flat_map(|pane| pane.items.iter())
            .filter_map(|item| match item {
                super::Item::Diff(diff) if diff.path == path => Some(diff.view.clone()),
                _ => None,
            })
            .collect()
    }

    pub(super) fn diff_views_edited(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        for view in self.diff_views_of(path) {
            view.update(cx, |view, cx| view.edited(cx));
        }
    }

    pub(super) fn diff_bases_changed(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let views = self.diff_views_of(path);
        if views.is_empty() {
            return;
        }
        let text = self.buffer(path).and_then(|b| b.git.bases.as_ref()).and_then(|b| b.head.clone()).unwrap_or_default();
        // Outside the window's update this may run in.
        cx.defer(move |cx| {
            let Some(window) = cx.windows().first().copied() else { return };
            window
                .update(cx, |_, window, cx| {
                    for view in views {
                        view.update(cx, |view, cx| view.set_base(text.clone(), window, cx));
                    }
                })
                .ok();
        });
    }

    /// Editor ▸ Side by Side Comparison / Inline Comparison.
    pub fn set_comparison(&mut self, side_by_side: bool, cx: &mut Context<Self>) {
        self.git.side_by_side = side_by_side;
        let views: Vec<Entity<DiffView>> = self
            .panes
            .iter()
            .flat_map(|pane| pane.items.iter())
            .filter_map(|item| match item {
                super::Item::Diff(diff) => Some(diff.view.clone()),
                _ => None,
            })
            .collect();
        for view in views {
            view.update(cx, |view, cx| view.set_side_by_side(side_by_side, cx));
        }
        cx.notify();
    }
}

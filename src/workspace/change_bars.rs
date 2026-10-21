//! Change bars in the editor gutter: lines added, modified or deleted
//! against `HEAD`, beside the line numbers. A change that is staged (the
//! index has it) is drawn hollow, as in Zed; one partly staged, solid.
//!
//! The editor's text is compared, not the file: each edit schedules a diff
//! in the background (a few milliseconds later, so a burst of typing makes
//! one), and the bars move as you type. Clicking a bar shows the change in
//! a popover with Discard Change and Stage / Unstage Change;
//! Navigate ▸ Jump to Next / Previous Change (⌃\ ⌃⇧\) go from one to the
//! next.

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use zcode_editor::{EditorState, GutterItem, GutterLane, GutterPlacement, RopeExt as _, TransactionId};

use super::Workspace;
use crate::git::diff::{self, Hunk, HunkKind, Staging};
use crate::git::{self, Bases};

/// The gutter lane of change bars.
pub(super) const CHANGES_LANE: &str = "git-changes";
pub(super) const LANE_WIDTH: f32 = 6.;
/// From an edit to the diff that follows it.
const DIFF_DELAY: Duration = Duration::from_millis(30);

#[derive(Clone, Debug, PartialEq)]
pub struct DiffHunk {
    /// Rows of `HEAD` against rows of the editor's text.
    pub hunk: Hunk,
    pub staging: Staging,
}

/// A buffer's comparison with git.
#[derive(Default)]
pub struct BufferGit {
    /// The index and `HEAD` versions; `None` until read, or outside a repository.
    pub bases: Option<Bases>,
    /// Against `HEAD`, in the editor's rows.
    pub hunks: Rc<Vec<DiffHunk>>,
    /// The blame of the editor's text, and whether edits made it stale.
    pub blame: Option<Rc<git::blame::Blame>>,
    pub blame_stale: bool,
    pub(super) diff_task: Option<Task<()>>,
    pub(super) blame_task: Option<Task<()>>,
    /// The Discard Changes that unstaged what they discarded, latest last.
    pub(super) discarded_stages: Vec<DiscardedStage>,
}

/// A Discard Change of a staged change: undoing its edit stages the change
/// again, redoing it unstages it again (`index_follows_undo`).
pub(super) struct DiscardedStage {
    /// The editor's transaction the discard made.
    transaction: TransactionId,
    rel: String,
    mode: Option<String>,
    /// The index's text of the file before and after the discard (`None`:
    /// not in the index).
    staged: Option<String>,
    unstaged: Option<String>,
    /// Whether the edit is undone, so the index has `staged` again.
    undone: bool,
}

/// How many discards a file keeps for undo.
const KEPT_DISCARDS: usize = 16;

impl Workspace {
    /// New versions of a buffer's file from git: compares again if they changed.
    pub(super) fn set_git_bases(&mut self, path: &Path, bases: Bases, force: bool, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else { return };
        if !force && buffer.git.bases.as_ref() == Some(&bases) {
            return;
        }
        buffer.git.bases = Some(bases);
        self.schedule_diff(path, Duration::ZERO, cx);
        self.diff_bases_changed(path, cx);
        // Authors and inline blame follow (a file just read, or just committed).
        self.schedule_blame(path, Duration::ZERO, cx);
    }

    /// Compares the buffer's text with its versions after `delay`, in the
    /// background; a newer request replaces a pending one.
    pub(super) fn schedule_diff(&mut self, path: &Path, delay: Duration, cx: &mut Context<Self>) {
        let untracked = self.is_untracked(path);
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else { return };
        let Some(bases) = buffer.git.bases.clone() else { return };
        // A file git doesn't track is all added; one it ignores, unmarked.
        let (head, index) = match (bases.head, bases.index) {
            (None, None) if untracked => (String::new(), String::new()),
            (None, None) => {
                if !buffer.git.hunks.is_empty() {
                    buffer.git.hunks = Rc::default();
                    cx.notify();
                }
                return;
            }
            (head, index) => (head.unwrap_or_default(), index.unwrap_or_default()),
        };
        let text = buffer.editor.read(cx).text().clone();
        // A file this large isn't compared at every pause in typing.
        if text.len() > super::buffers::MAX_LANGUAGE_FEATURES_SIZE {
            return;
        }
        let path = path.to_path_buf();
        buffer.git.diff_task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let hunks = cx.background_spawn(async move { compare(&head, &index, &text.to_string()) }).await;
            this.update(cx, |this, cx| {
                if let Some(buffer) = this.buffers.iter_mut().find(|b| b.path == path)
                    && *buffer.git.hunks != hunks
                {
                    buffer.git.hunks = Rc::new(hunks);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// After an edit: the bars follow it.
    pub(super) fn buffer_edited_for_git(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.schedule_diff(path, DIFF_DELAY, cx);
        self.buffer_edited_for_blame(path, cx);
        self.diff_views_edited(path, cx);
        self.index_follows_undo(path, cx);
    }

    /// ⌘Z of a Discard Change that unstaged the change: the index gets
    /// back what the discard took from it, and ⇧⌘Z takes it again. Only
    /// while the index has what the step before left there: after a change
    /// staged, unstaged or committed since, the index is left alone, the
    /// user is told, and the discard's undo no longer touches it.
    fn index_follows_undo(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) else { return };
        if buffer.git.discarded_stages.is_empty() {
            return;
        }
        let editor = buffer.editor.read(cx);
        let (last, next_redo) = (editor.last_transaction(), editor.next_redo_transaction());
        // (the discard, rel, what the index should have, what it gets, mode)
        let mut writes: Vec<(TransactionId, String, Option<String>, Option<String>, Option<String>)> = vec![];
        for discard in &mut buffer.git.discarded_stages {
            let (expected, text) = if !discard.undone && next_redo == Some(discard.transaction) {
                (&discard.unstaged, &discard.staged)
            } else if discard.undone && last == Some(discard.transaction) {
                (&discard.staged, &discard.unstaged)
            } else {
                continue;
            };
            writes.push((discard.transaction, discard.rel.clone(), expected.clone(), text.clone(), discard.mode.clone()));
            discard.undone = !discard.undone;
        }
        if writes.is_empty() {
            return;
        }
        // After git's operation in progress, outside the window's update
        // (undo runs in it).
        let (window, path) = (self.window, path.to_path_buf());
        cx.spawn(async move |this, cx| {
            while this.update(cx, |this, _| this.git.busy.is_some()).unwrap_or(false) {
                cx.background_executor().timer(Duration::from_millis(50)).await;
            }
            cx.update_window(window, |_, window, cx| {
                this.update(cx, |this, cx| {
                    let label = if writes.iter().any(|(_, _, _, text, _)| text.is_some()) { "Staging" } else { "Unstaging" };
                    this.run_git(
                        label,
                        move |root| {
                            let failed: Vec<(TransactionId, String)> = writes
                                .iter()
                                .filter_map(|(transaction, rel, expected, text, mode)| {
                                    let written = git::write_index_if(root, rel, expected.as_deref(), text.as_deref(), mode.as_deref());
                                    written.err().map(|err| (*transaction, format!("{err:#}")))
                                })
                                .collect();
                            Ok(failed)
                        },
                        move |this, failed, window, cx| {
                            let Some(buffer) = this.buffers.iter_mut().find(|b| b.path == path) else { return };
                            buffer.git.discarded_stages.retain(|d| !failed.iter().any(|(t, _)| *t == d.transaction));
                            if let Some((_, message)) = failed.into_iter().next_back() {
                                this.git.last_error = Some(message.clone());
                                this.notify_error(message, window, cx);
                            }
                        },
                        window,
                        cx,
                    )
                })
            })
            .ok();
        })
        .detach();
    }

    /// The change bars in `editor`'s gutter, between the numbers and the
    /// fold chevrons (where Xcode draws them).
    pub(super) fn changes_lane(&self, editor: &Entity<EditorState>, cx: &Context<Self>) -> GutterLane {
        let workspace = cx.weak_entity();
        let editor_id = editor.entity_id();
        GutterLane::new(CHANGES_LANE, move |rows, cx| {
            let Some(this) = workspace.upgrade() else { return vec![] };
            let this = this.read(cx);
            let Some(path) = this.buffer_path_of(editor_id) else { return vec![] };
            let Some(buffer) = this.buffer(&path) else { return vec![] };
            if buffer.git.hunks.is_empty() {
                return vec![];
            }
            let last_row = rows.text().lines_len().saturating_sub(1) as u32;
            let colors = Rc::new(BarColors::new(cx));
            let mut items = vec![];
            for (ix, hunk) in buffer.git.hunks.iter().enumerate() {
                let kind = hunk.hunk.kind();
                // A deletion's mark sits on the line after it (the last line
                // when it was at the end).
                let marked = match kind {
                    HunkKind::Deleted => {
                        let row = hunk.hunk.new.start.min(last_row);
                        row..row + 1
                    }
                    _ => hunk.hunk.new.clone(),
                };
                let at_end = kind == HunkKind::Deleted && hunk.hunk.new.start > last_row;
                for row in marked.start.max(rows.rows.start)..marked.end.min(rows.rows.end) {
                    let (colors, staging, workspace, path) = (colors.clone(), hunk.staging, workspace.clone(), path.clone());
                    items.push(
                        GutterItem::new(row, move |paint, window, _| {
                            paint_bar(window, paint.cell, kind, staging, at_end, paint.hovered, &colors);
                        })
                        .cursor(CursorStyle::PointingHand)
                        .on_click(move |event, window, cx| {
                            let position = point(event.bounds.right(), event.bounds.top());
                            // After the editor's mouse handling, which holds it.
                            let (workspace, path) = (workspace.clone(), path.clone());
                            window.defer(cx, move |window, cx| {
                                workspace.update(cx, |this, cx| this.show_hunk_popover(path, ix, position, window, cx)).ok();
                            });
                            true
                        }),
                    );
                }
            }
            items
        })
        .placement(GutterPlacement::AfterNumbers(px(LANE_WIDTH)))
    }

    // ---------------------------------------------------------------------
    // A hunk's actions

    /// The hunk `ix` of `path`, with the `HEAD` and index texts.
    fn hunk_with_bases(&self, path: &Path, ix: usize) -> Option<(DiffHunk, String, Option<String>)> {
        let buffer = self.buffer(path)?;
        let hunk = buffer.git.hunks.get(ix)?.clone();
        let bases = buffer.git.bases.as_ref()?;
        Some((hunk, bases.head.clone().unwrap_or_default(), bases.index.clone()))
    }

    /// Discard Change: the hunk's lines go back to `HEAD`'s, as one edit
    /// ⌘Z undoes. What of it is staged leaves the index too, or the next
    /// commit would have the change the editor no longer shows; ⌘Z stages
    /// it again (`index_follows_undo`).
    pub fn discard_hunk(&mut self, path: &Path, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((hunk, head, index)) = self.hunk_with_bases(path, ix) else { return };
        let Some(buffer) = self.buffer(path).filter(|b| !b.read_only) else { return };
        let editor = buffer.editor.clone();
        let has_head = buffer.git.bases.as_ref().is_some_and(|b| b.head.is_some());
        let mode = buffer.git.bases.as_ref().and_then(|b| b.mode.clone());
        let unstage = match index {
            Some(index) if hunk.staging != Staging::Unstaged => {
                let Some((_, rel)) = self.git_rel(path) else { return };
                if let Some(busy) = &self.git.busy {
                    self.notify_error(format!("Wait for “{busy}” to finish."), window, cx);
                    return;
                }
                let unstaged = diff::unstage_hunk(&hunk.hunk, &index, &head);
                Some((rel, (index, unstaged)))
            }
            _ => None,
        };
        let text = editor.read(cx).text().to_string();
        let (range, replacement) = diff::discard_edit(&head, &text, &hunk.hunk);
        let before = editor.read(cx).last_transaction();
        editor.update(cx, |state, cx| {
            state.transact(window, cx, |state, window, cx| state.edit([(range, replacement.as_str())], window, cx));
        });
        let transaction = editor.read(cx).last_transaction().filter(|t| Some(*t) != before);
        self.schedule_diff(path, Duration::ZERO, cx);
        if let Some((rel, (index, new_index))) = unstage {
            // A file new since HEAD leaves the index when nothing of it is left.
            let unstaged = (has_head || !new_index.is_empty()).then_some(new_index);
            // Its undo stages it again.
            if let Some(transaction) = transaction
                && let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path)
            {
                let stages = &mut buffer.git.discarded_stages;
                stages.push(DiscardedStage { transaction, rel: rel.clone(), mode: mode.clone(), staged: Some(index), unstaged: unstaged.clone(), undone: false });
                if stages.len() > KEPT_DISCARDS {
                    stages.remove(0);
                }
            }
            self.run_git(
                "Unstaging",
                move |root| git::write_index(root, &rel, unstaged.as_deref(), mode.as_deref()),
                |_, _, _, _| {},
                window,
                cx,
            );
        }
    }

    /// Stage Change / Unstage Change: the index takes the editor's lines of
    /// the hunk, or gives them back for `HEAD`'s.
    pub fn stage_hunk(&mut self, path: &Path, ix: usize, stage: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((hunk, head, index)) = self.hunk_with_bases(path, ix) else { return };
        let Some((_, rel)) = self.git_rel(path) else { return };
        let Some(buffer) = self.buffer(path) else { return };
        let current = buffer.editor.read(cx).text().to_string();
        let mode = buffer.git.bases.as_ref().and_then(|b| b.mode.clone());
        let index = index.unwrap_or_default();
        let new_index = if stage { diff::stage_hunk(&hunk.hunk, &index, &current) } else { diff::unstage_hunk(&hunk.hunk, &index, &head) };
        // A file new since HEAD leaves the index when nothing of it is staged.
        let has_head = buffer.git.bases.as_ref().is_some_and(|b| b.head.is_some());
        let remove = !stage && !has_head && new_index.is_empty();
        let label = if stage { "Staging" } else { "Unstaging" };
        self.run_git(
            label,
            move |root| git::write_index(root, &rel, (!remove).then_some(new_index.as_str()), mode.as_deref()),
            |_, _, _, _| {},
            window,
            cx,
        );
    }

    /// Navigate ▸ Jump to Next / Previous Change, in a file's tab or its
    /// comparison.
    pub fn jump_to_change_anywhere(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_diff().cloned() {
            Some(diff) => diff.update(cx, |diff, cx| diff.jump_to_change(forward, window, cx)),
            None => self.jump_to_change(forward, window, cx),
        }
    }

    /// Jump to Next / Previous Change: the next hunk below (or above) the
    /// cursor, wrapping around.
    pub fn jump_to_change(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.active_item() else { return };
        let (path, editor) = (item.path.clone(), item.editor.clone());
        let Some(buffer) = self.buffer(&path) else { return };
        let starts: Vec<u32> = buffer.git.hunks.iter().map(|h| h.hunk.new.start).collect();
        let row = editor.read(cx).cursor_position().line;
        let target = if forward {
            starts.iter().find(|&&s| s > row).or(starts.first())
        } else {
            starts.iter().rev().find(|&&s| s < row).or(starts.last())
        };
        let Some(&target) = target else { return };
        self.open_file(path, Some(super::Target::Line { line: target, column: 0 }), true, window, cx);
    }

    /// The control channel's view of the active file's hunks and their bars.
    pub(super) fn hunks_summary(&self, cx: &App) -> serde_json::Value {
        let Some(buffer) = self.active_buffer() else { return serde_json::Value::Null };
        let bars = self.painted_gutter(CHANGES_LANE, cx);
        serde_json::json!({
            "path": buffer.path,
            "tracked": buffer.git.bases.is_some(),
            "list": buffer.git.hunks.iter().map(|h| serde_json::json!({
                "kind": format!("{:?}", h.hunk.kind()),
                "staging": format!("{:?}", h.staging),
                // 1-based lines of the editor and of HEAD.
                "lines": [h.hunk.new.start + 1, h.hunk.new.end],
                "head_lines": [h.hunk.old.start + 1, h.hunk.old.end],
            })).collect::<Vec<_>>(),
            "bars": bars.iter().map(|(line, b)| serde_json::json!({
                "line": line,
                "x": f32::from(b.center().x),
                "y": f32::from(b.center().y),
            })).collect::<Vec<_>>(),
        })
    }
}

/// The hunks of `current` against `head`, with how much of each is staged.
pub fn compare(head: &str, index: &str, current: &str) -> Vec<DiffHunk> {
    let head_hunks = diff::line_hunks(head, current);
    if head_hunks.is_empty() {
        return vec![];
    }
    let index_hunks = if index == head { head_hunks.clone() } else { diff::line_hunks(index, current) };
    let (head_lines, index_lines) = (diff::lines(head), diff::lines(index));
    head_hunks
        .into_iter()
        .map(|hunk| {
            let staging = diff::staging(&hunk, &index_hunks, &head_lines, &index_lines);
            DiffHunk { hunk, staging }
        })
        .collect()
}

pub(super) struct BarColors {
    added: Hsla,
    modified: Hsla,
    deleted: Hsla,
}

impl BarColors {
    pub(super) fn new(cx: &App) -> Self {
        let theme = cx.theme();
        Self { added: theme.success, modified: theme.info, deleted: theme.danger }
    }
}

/// A bar down the cell (a wedge at its top edge for a deletion); hollow
/// when staged, wider while hovered.
pub(super) fn paint_bar(window: &mut Window, cell: Bounds<Pixels>, kind: HunkKind, staging: Staging, at_end: bool, hovered: bool, colors: &BarColors) {
    let color = match kind {
        HunkKind::Added => colors.added,
        HunkKind::Modified => colors.modified,
        HunkKind::Deleted => colors.deleted,
    };
    let width = if hovered { px(4.) } else { px(3.) };
    let left = cell.left() + px(1.);
    if kind == HunkKind::Deleted {
        let y = if at_end { cell.bottom() } else { cell.top() };
        let h = px(4.);
        let mut path = PathBuilder::fill();
        path.add_polygon(&[point(left, y - h), point(left + px(5.), y), point(left, y + h)], true);
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
        return;
    }
    let bar = Bounds::new(point(left, cell.top()), size(width, cell.size.height));
    if staging == Staging::Staged {
        window.paint_quad(outline(bar, color, BorderStyle::Solid));
    } else {
        window.paint_quad(fill(bar, color));
    }
}

#[cfg(test)]
mod tests {
    use super::{HunkKind, Staging, compare};

    #[test]
    fn hunks_with_their_staging() {
        let head = "a\nb\nc\nd\n";
        let index = "A\nb\nc\nd\n";
        let current = "A\nb\nc\nD\ne\n";
        let hunks = compare(head, index, current);
        let summary: Vec<(HunkKind, Staging, std::ops::Range<u32>)> =
            hunks.iter().map(|h| (h.hunk.kind(), h.staging, h.hunk.new.clone())).collect();
        assert_eq!(summary, vec![(HunkKind::Modified, Staging::Staged, 0..1), (HunkKind::Modified, Staging::Unstaged, 3..5)]);
        assert!(compare(head, head, head).is_empty());
        // Untracked: everything added.
        let all = compare("", "", "x\ny\n");
        assert_eq!((all.len(), all[0].hunk.kind(), all[0].hunk.new.clone()), (1, HunkKind::Added, 0..2));
    }
}

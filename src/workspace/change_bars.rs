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
use zcode_editor::{EditorState, GutterItem, GutterLane, GutterPlacement, RopeExt as _};

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
}

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
    /// ⌘Z undoes.
    pub fn discard_hunk(&mut self, path: &Path, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((hunk, head, _)) = self.hunk_with_bases(path, ix) else { return };
        let Some(buffer) = self.buffer(path).filter(|b| !b.read_only) else { return };
        let editor = buffer.editor.clone();
        let text = editor.read(cx).text().clone();
        let head_lines = diff::lines(&head);
        let mut replacement: String = diff::slice(&head_lines, &hunk.hunk.old).concat();
        let row_offset = |row: u32| {
            if (row as usize) < text.lines_len() { text.line_start_offset(row as usize) } else { text.len() }
        };
        let (start, end) = (row_offset(hunk.hunk.new.start), row_offset(hunk.hunk.new.end));
        let ends_unterminated = !text.to_string().ends_with('\n');
        if end == text.len() && ends_unterminated && replacement.ends_with('\n') {
            // The file's last line has no newline; neither does what replaces it.
            replacement.pop();
            if start == text.len() && !replacement.is_empty() {
                replacement.insert(0, '\n');
            }
        }
        editor.update(cx, |state, cx| {
            state.transact(window, cx, |state, window, cx| state.edit([(start..end, replacement.as_str())], window, cx));
        });
        self.schedule_diff(path, Duration::ZERO, cx);
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

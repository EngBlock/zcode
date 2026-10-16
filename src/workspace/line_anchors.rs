//! Lines anchored in an open buffer, for breakpoints and bookmarks, on the
//! editor's line anchors (`zcode-editor`'s `anchors.rs`): the editor moves
//! them through edits, undo and redo. After an edit the owner reads what
//! happened to its lines: moved, deleted with their line, or brought back by
//! an undo. A deleted line's owner keeps what it removed here, with the
//! anchor, so the undo that restores the line restores it too.

use gpui_kit::*;
use zcode_editor::{Anchor, EditorState};

/// What an edit did to an anchored line (1-based lines).
pub(super) enum LineChange<T> {
    Moved(u64, u32),
    Deleted(u64),
    /// Undo brought the line back, with what the owner kept for it.
    Restored(u32, T),
}

pub(super) struct LineAnchors<T> {
    /// (id, 1-based line when last seen, anchor)
    live: Vec<(u64, u32, Anchor)>,
    /// Anchors whose line is deleted, with what their owner removed.
    deleted: Vec<(u64, Anchor, Option<T>)>,
}

impl<T> Default for LineAnchors<T> {
    fn default() -> Self {
        Self { live: vec![], deleted: vec![] }
    }
}

impl<T> LineAnchors<T> {
    /// Anchors `wanted` (id, 1-based line) in `editor`. Skipped when the
    /// anchors already match, unless `force` (the text was replaced, which
    /// also forgets the deleted lines: there is no undo back to them).
    pub fn set(&mut self, editor: &Entity<EditorState>, wanted: Vec<(u64, u32)>, force: bool, cx: &mut App) {
        let current: Vec<(u64, u32)> = self.live.iter().map(|&(id, line, _)| (id, line)).collect();
        if current == wanted && !force {
            return;
        }
        editor.update(cx, |state, _| {
            let mut old = std::mem::take(&mut self.live);
            for (id, line) in wanted {
                let kept = (!force).then(|| old.iter().position(|&(i, l, _)| i == id && l == line)).flatten();
                match kept {
                    Some(ix) => self.live.push(old.remove(ix)),
                    None => self.live.push((id, line, state.line_anchor(line.saturating_sub(1)))),
                }
            }
            for (_, _, anchor) in old {
                state.remove_anchor(anchor);
            }
            let live = &self.live;
            self.deleted.retain(|(id, anchor, _)| {
                let keep = !force && !live.iter().any(|(i, _, _)| i == id);
                if !keep {
                    state.remove_anchor(*anchor);
                }
                keep
            });
        });
    }

    /// What happened to the anchored lines since the last call.
    pub fn changes(&mut self, editor: &Entity<EditorState>, cx: &App) -> Vec<LineChange<T>> {
        let state = editor.read(cx);
        let mut changes = vec![];
        let mut ix = 0;
        while ix < self.live.len() {
            let (id, line, anchor) = self.live[ix];
            match state.anchor_row(anchor) {
                None => {
                    self.live.remove(ix);
                    self.deleted.push((id, anchor, None));
                    changes.push(LineChange::Deleted(id));
                    continue;
                }
                Some(row) if row + 1 != line => {
                    self.live[ix].1 = row + 1;
                    changes.push(LineChange::Moved(id, row + 1));
                }
                Some(_) => {}
            }
            ix += 1;
        }
        let mut ix = 0;
        while ix < self.deleted.len() {
            let anchor = self.deleted[ix].1;
            match state.anchor_row(anchor) {
                Some(row) => {
                    let (id, anchor, kept) = self.deleted.remove(ix);
                    self.live.push((id, row + 1, anchor));
                    if let Some(kept) = kept {
                        changes.push(LineChange::Restored(row + 1, kept));
                    }
                }
                None => ix += 1,
            }
        }
        changes
    }

    /// Keeps what the owner removed with `id`'s deleted line, for the undo
    /// that brings the line back.
    pub fn keep_deleted(&mut self, id: u64, removed: T) {
        if let Some(entry) = self.deleted.iter_mut().find(|(i, _, _)| *i == id) {
            entry.2 = Some(removed);
        }
    }
}

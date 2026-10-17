//! What an agent does to files, through the workspace's buffers: it reads
//! a file's open buffer (unsaved edits included) rather than the disk, and
//! its writes land in the buffer as one undoable transaction each, then
//! are saved, so ⌘Z in any pane showing the file takes them back. A file no
//! pane has open gets a headless buffer (as a multibuffer's do), kept for
//! the thread so its undo history and Reject still work, and let go when
//! the thread is.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::*;
use similar::{Algorithm, DiffTag, TextDiff};

use super::super::buffers::MAX_OPEN_FILE_SIZE;
use super::super::{Target, Workspace};
use crate::acp::RpcError;

impl Workspace {
    /// `fs/read_text_file`: the open buffer's text, or the file's; from
    /// 1-based `line`, at most `limit` lines.
    pub(super) fn agent_read_file(&self, path: &Path, line: Option<u32>, limit: Option<u32>, cx: &App) -> Result<String, RpcError> {
        if !path.is_absolute() {
            return Err(RpcError::new(RpcError::INVALID_PARAMS, format!("{} isn’t an absolute path", path.display())));
        }
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let text = match self.buffer(&path) {
            Some(buffer) => buffer.editor.read(cx).text().to_string(),
            None => read_disk(&path)?,
        };
        Ok(line_window(&text, line, limit))
    }

    /// `fs/write_text_file`: the file's buffer edited to `content` in one
    /// transaction and saved. Returns what the file was before (`None` for a
    /// file the agent created) and the first line it changed (0-based).
    pub(super) fn agent_write_file(&mut self, path: &Path, content: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(Option<String>, u32), RpcError> {
        let path = self.agent_writable_path(path)?;
        let mut created = false;
        if self.buffer(&path).is_none() {
            if !path.exists() {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|err| RpcError::internal(format!("couldn’t create {}: {err}", dir.display())))?;
                }
                std::fs::write(&path, "").map_err(|err| RpcError::internal(format!("couldn’t create {}: {err}", path.display())))?;
                created = true;
                self.rescan(cx);
            } else if path.is_dir() {
                return Err(RpcError::new(RpcError::INVALID_PARAMS, format!("{} is a folder", path.display())));
            }
            if !self.open_buffer_with(&path, true, window, cx) {
                return Err(RpcError::internal(format!("couldn’t open {}", path.display())));
            }
        }
        let buffer = self.buffer(&path).ok_or_else(|| RpcError::internal("no buffer"))?;
        if buffer.read_only {
            return Err(RpcError::new(RpcError::INVALID_PARAMS, format!("{} can’t be edited", path.display())));
        }
        let editor = buffer.editor.clone();
        let before = editor.read(cx).text().to_string();
        let edits = text_edits(&before, content);
        let first_line = edits.first().map(|(range, _)| before[..range.start].matches('\n').count() as u32).unwrap_or(0);
        if !edits.is_empty() {
            editor.update(cx, |state, cx| {
                state.transact(window, cx, |state, window, cx| state.edit(edits, window, cx));
            });
        }
        self.save_path(&path, window, cx);
        Ok(((!created).then_some(before), first_line))
    }

    /// Where an agent may write: an absolute path inside the project.
    fn agent_writable_path(&self, path: &Path) -> Result<PathBuf, RpcError> {
        if !path.is_absolute() {
            return Err(RpcError::new(RpcError::INVALID_PARAMS, format!("{} isn’t an absolute path", path.display())));
        }
        let path = path.canonicalize().unwrap_or_else(|_| crate::xcodeproj::files::normalize(path));
        // The parent may be a symlink the project root resolved through.
        let path = match (path.parent().and_then(|p| p.canonicalize().ok()), path.file_name()) {
            (Some(dir), Some(name)) => dir.join(name),
            _ => path,
        };
        if !path.starts_with(&self.project.root) {
            return Err(RpcError::new(RpcError::INVALID_PARAMS, format!("{} is outside the project; zcode only lets agents write in {}", path.display(), self.project.root.display())));
        }
        Ok(path)
    }

    /// The agent changed `path` on disk itself (a tool call reported its
    /// `diff`, old and new text, perhaps only the changed part): the change
    /// lands in the file's buffer as an undoable edit, at once rather than
    /// at the next poll. A file no buffer has gets a headless one holding
    /// what it was before (worked out from the diff), so ⌘Z and Reject work
    /// for it too. Returns what it was before (`None`: the agent created
    /// it), what it is now, and whether a buffer was opened; `None` when
    /// there's nothing to take in (a buffer with unsaved edits is left alone).
    pub(super) fn agent_adopt_disk_change(&mut self, path: &Path, diff: (Option<String>, String), window: &mut Window, cx: &mut Context<Self>) -> Option<(Option<String>, String, bool)> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let now = std::fs::read_to_string(&path).ok()?;
        if let Some(buffer) = self.buffer(&path) {
            if buffer.dirty || buffer.read_only || buffer.plist_format.is_some() {
                return None;
            }
            let editor = buffer.editor.clone();
            let before = editor.read(cx).text().to_string();
            if before == now {
                return None;
            }
            self.replace_saved_text(&path, &editor, &now, window, cx);
            return Some((Some(before), now, false));
        }
        let before = match diff {
            (None, _) => return Some((None, now, false)),
            (Some(old), new) if new == now => old,
            (Some(old), new) if !new.is_empty() && now.matches(new.as_str()).count() == 1 => now.replacen(new.as_str(), &old, 1),
            _ => return None,
        };
        if before == now || !self.open_buffer_with(&path, true, window, cx) {
            return None;
        }
        let editor = self.buffer(&path)?.editor.clone();
        editor.update(cx, |state, cx| state.set_value(before.clone(), window, cx));
        self.replace_saved_text(&path, &editor, &now, window, cx);
        Some((Some(before), now, true))
    }

    /// The buffer edited to `text`, which is what the file on disk has.
    fn replace_saved_text(&mut self, path: &Path, editor: &Entity<zcode_editor::EditorState>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let edits = text_edits(&editor.read(cx).text().to_string(), text);
        if let Some(buffer) = self.buffers.iter_mut().find(|b| b.path == path) {
            buffer.saved_text = zcode_editor::Rope::from(text);
            buffer.mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        }
        editor.update(cx, |state, cx| {
            state.transact(window, cx, |state, window, cx| state.edit(edits, window, cx));
        });
        self.file_changed_on_disk(cx);
        cx.notify();
    }

    /// Reject: the file back to what it was before the agent (an undoable
    /// edit, saved), or a file the agent created moved to the Trash.
    pub(super) fn agent_restore(&mut self, path: &Path, before: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(before) = before else {
            if path.exists() {
                self.move_to_trash(path.to_path_buf(), window, cx);
            }
            return;
        };
        if self.buffer(path).is_none() && !self.open_buffer_with(path, true, window, cx) {
            return;
        }
        let Some(editor) = self.buffer(path).filter(|b| !b.read_only).map(|b| b.editor.clone()) else { return };
        let edits = text_edits(&editor.read(cx).text().to_string(), before);
        if !edits.is_empty() {
            editor.update(cx, |state, cx| {
                state.transact(window, cx, |state, window, cx| state.edit(edits, window, cx));
            });
        }
        self.save_path(path, window, cx);
    }

    /// Following the agent: shows the file it's reading or changing at the
    /// line; the keyboard stays in `keep_focus` (the agent panel's message
    /// field, when it was there).
    pub(super) fn agent_follow(&mut self, path: &Path, line: Option<u32>, keep_focus: Option<FocusHandle>, window: &mut Window, cx: &mut Context<Self>) {
        if !path.is_file() {
            return;
        }
        let target = line.map(|line| Target::Line { line, column: 0 });
        self.open_file(path.to_path_buf(), target, true, window, cx);
        if let Some(focus) = keep_focus {
            focus.focus(window, cx);
            // Again after the editor's first layout has scrolled to the line.
            cx.spawn_in(window, async move |_, cx| {
                cx.background_executor().timer(Duration::from_millis(40)).await;
                cx.update(|window, cx| focus.focus(window, cx)).ok();
            })
            .detach();
        }
    }

    /// Lets go of the headless buffers an agent's writes opened, unless
    /// something shows them now or they have unsaved edits.
    pub(super) fn release_agent_buffers(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        for path in paths {
            let Some(ix) = self.buffers.iter().position(|b| &b.path == path && b.headless && !b.dirty) else { continue };
            if !self.views_of(path).is_empty() || !self.diff_views_of(path).is_empty() || !self.multibuffer_views_of(path, cx).is_empty() {
                continue;
            }
            let buffer = self.buffers.remove(ix);
            self.lsp.update(cx, |lsp, cx| lsp.close_document(&buffer.path, cx));
        }
    }

    /// The text an @-mention of a file sends: its open buffer's, or the file's.
    pub(super) fn mention_text(&self, path: &Path, lines: Option<Range<u32>>, cx: &App) -> Option<String> {
        let text = match self.buffer(path) {
            Some(buffer) => buffer.editor.read(cx).text().to_string(),
            None => read_disk(path).ok()?,
        };
        Some(match lines {
            Some(lines) => text.split_inclusive('\n').skip(lines.start as usize).take((lines.end - lines.start + 1) as usize).collect(),
            None => text,
        })
    }
}

fn read_disk(path: &Path) -> Result<String, RpcError> {
    let meta = std::fs::metadata(path).map_err(|_| RpcError::new(RpcError::NOT_FOUND, format!("{} doesn’t exist", path.display())))?;
    if meta.len() > MAX_OPEN_FILE_SIZE {
        return Err(RpcError::new(RpcError::INVALID_PARAMS, format!("{} is too large", path.display())));
    }
    std::fs::read_to_string(path).map_err(|err| RpcError::internal(format!("couldn’t read {}: {err}", path.display())))
}

/// `text` from 1-based `line`, at most `limit` lines.
fn line_window(text: &str, line: Option<u32>, limit: Option<u32>) -> String {
    if line.is_none() && limit.is_none() {
        return text.to_string();
    }
    let skip = line.unwrap_or(1).saturating_sub(1) as usize;
    let take = limit.map_or(usize::MAX, |l| l as usize);
    text.split_inclusive('\n').skip(skip).take(take).collect()
}

/// The edits that turn `old` into `new`, by line, in `old`'s byte offsets:
/// lines the agent didn't change keep their breakpoints, bookmarks and
/// cursors.
pub(super) fn text_edits(old: &str, new: &str) -> Vec<(Range<usize>, String)> {
    if old == new {
        return vec![];
    }
    let diff = TextDiff::configure().algorithm(Algorithm::Myers).timeout(Duration::from_millis(500)).diff_lines(old, new);
    let offsets = |text: &str| {
        let mut starts = vec![0];
        starts.extend(text.split_inclusive('\n').scan(0, |at, line| {
            *at += line.len();
            Some(*at)
        }));
        starts
    };
    let (old_starts, new_starts) = (offsets(old), offsets(new));
    let mut edits: Vec<(Range<usize>, String)> = vec![];
    let mut last_end: Option<(usize, usize)> = None;
    for op in diff.ops() {
        let (tag, old_rows, new_rows) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        let range = old_starts[old_rows.start]..old_starts[old_rows.end];
        let text = &new[new_starts[new_rows.start]..new_starts[new_rows.end]];
        match (edits.last_mut(), last_end) {
            // A deletion next to an insertion is one replacement.
            (Some(last), Some((old_end, new_end))) if old_end == old_rows.start && new_end == new_rows.start => {
                last.0.end = range.end;
                last.1.push_str(text);
            }
            _ => edits.push((range, text.to_string())),
        }
        last_end = Some((old_rows.end, new_rows.end));
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::{line_window, text_edits};
    use std::ops::Range;

    fn apply(old: &str, edits: &[(Range<usize>, String)]) -> String {
        let mut out = old.to_string();
        for (range, text) in edits.iter().rev() {
            out.replace_range(range.clone(), text);
        }
        out
    }

    #[test]
    fn edits_turn_the_old_text_into_the_new_one_exactly() {
        let cases = [
            ("a\nb\nc\n", "a\nB\nc\n"),
            ("a\nb\nc\n", "a\nb\nc"),
            ("a\nb", "a\nb\n"),
            ("", "new\nfile\n"),
            ("gone\n", ""),
            ("one\ntwo\nthree\nfour\n", "zero\none\nthree\nfour\nfive\n"),
            ("same\n", "same\n"),
        ];
        for (old, new) in cases {
            assert_eq!(apply(old, &text_edits(old, new)), new, "{old:?} → {new:?}");
        }
    }

    #[test]
    fn unchanged_lines_are_left_alone() {
        let old = "1\n2\n3\n4\n5\n";
        let edits = text_edits(old, "1\nTWO\n3\n4\nFIVE\n");
        assert_eq!(edits, [(2..4, "TWO\n".to_string()), (8..10, "FIVE\n".to_string())]);
    }

    #[test]
    fn reads_from_a_line_for_a_number_of_lines() {
        let text = "1\n2\n3\n4\n";
        assert_eq!(line_window(text, None, None), text);
        assert_eq!(line_window(text, Some(2), Some(2)), "2\n3\n");
        assert_eq!(line_window(text, Some(3), None), "3\n4\n");
        assert_eq!(line_window(text, None, Some(1)), "1\n");
        assert_eq!(line_window(text, Some(9), None), "");
    }
}

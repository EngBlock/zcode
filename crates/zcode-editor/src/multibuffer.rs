//! The excerpt model a multibuffer is built on (roadmap 2.5): one editor
//! showing editable excerpts of many files, as project search results and
//! diagnostics do in Zed.
//!
//! This is the model: which ranges of which source files are shown, the
//! text an editor shows for them (each excerpt's text, separated by a
//! newline), and the translation of positions and edits between that text
//! and the sources, both ways. [`crate::MultiBufferEditor`] hosts it in an
//! editor with the engine's hooks: `BufferEdit` events to forward edits,
//! read-only ranges over the separators, blocks above rows for the excerpt
//! headers, other line numbers in the gutter, and an external history.
//!
//! Sources are identified by the caller (`SourceId`, for example an editor
//! entity's id); offsets are UTF-8 bytes.

use std::ops::Range;

use gpui::SharedString;
use ropey::Rope;

/// A source file (buffer), as the caller identifies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExcerptId(u64);

#[derive(Clone, Debug, PartialEq)]
pub struct Excerpt {
    pub id: ExcerptId,
    pub source: SourceId,
    /// The excerpt's range in its source, kept up to date with edits.
    pub range: Range<usize>,
    /// What the editor shows above it (a file name, a line).
    pub header: SharedString,
}

/// An edit in a source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEdit {
    pub source: SourceId,
    pub range: Range<usize>,
    pub new_text: String,
}

/// An edit of the multibuffer's own text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposedEdit {
    pub range: Range<usize>,
    pub new_text: String,
}

/// Why a multibuffer edit can't reach a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditError {
    /// It touches the newline between two excerpts (or spans several).
    CrossesExcerpts,
}

#[derive(Default)]
pub struct MultiBuffer {
    excerpts: Vec<Excerpt>,
    /// Each excerpt's start in the composed text.
    starts: Vec<usize>,
    next_id: u64,
}

impl MultiBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an excerpt of `source` at the end.
    pub fn push_excerpt(&mut self, source: SourceId, range: Range<usize>, header: impl Into<SharedString>) -> ExcerptId {
        let id = ExcerptId(self.next_id);
        self.next_id += 1;
        let start = if self.excerpts.is_empty() { 0 } else { self.len() + 1 };
        self.starts.push(start);
        self.excerpts.push(Excerpt { id, source, range, header: header.into() });
        id
    }

    pub fn excerpts(&self) -> &[Excerpt] {
        &self.excerpts
    }

    pub fn excerpt(&self, id: ExcerptId) -> Option<&Excerpt> {
        self.excerpts.get(self.index_of(id)?)
    }

    pub fn index_of(&self, id: ExcerptId) -> Option<usize> {
        self.excerpts.iter().position(|excerpt| excerpt.id == id)
    }

    /// The composed text's length.
    pub fn len(&self) -> usize {
        match (self.starts.last(), self.excerpts.last()) {
            (Some(start), Some(last)) => start + last.range.len(),
            _ => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.excerpts.is_empty()
    }

    /// Recomputes the composed starts from excerpt `ix` on.
    fn restart_from(&mut self, ix: usize) {
        self.starts.truncate(ix);
        let mut start = match ix.checked_sub(1) {
            Some(prev) => self.starts[prev] + self.excerpts[prev].range.len() + 1,
            None => 0,
        };
        for excerpt in &self.excerpts[ix..] {
            self.starts.push(start);
            start += excerpt.range.len() + 1;
        }
    }

    /// The text an editor shows: each excerpt's text from `text_of`, joined
    /// with newlines.
    pub fn compose(&self, mut text_of: impl FnMut(SourceId, Range<usize>) -> String) -> Rope {
        let mut composed = String::new();
        for (ix, excerpt) in self.excerpts.iter().enumerate() {
            if ix > 0 {
                composed.push('\n');
            }
            composed.push_str(&text_of(excerpt.source, excerpt.range.clone()));
        }
        Rope::from(composed.as_str())
    }

    /// Each excerpt's range in the composed text.
    pub fn composed_ranges(&self) -> Vec<(ExcerptId, Range<usize>)> {
        self.excerpts.iter().zip(&self.starts).map(|(excerpt, &start)| (excerpt.id, start..start + excerpt.range.len())).collect()
    }

    /// Excerpt `id`'s range in the composed text.
    pub fn composed_range(&self, id: ExcerptId) -> Option<Range<usize>> {
        let ix = self.index_of(id)?;
        Some(self.starts[ix]..self.starts[ix] + self.excerpts[ix].range.len())
    }

    /// The newlines between excerpts, for the editor's read-only ranges.
    pub fn separators(&self) -> Vec<Range<usize>> {
        let ranges = self.composed_ranges();
        ranges.iter().take(ranges.len().saturating_sub(1)).map(|(_, r)| r.end..r.end + 1).collect()
    }

    /// The index of the excerpt showing composed offset `offset` (its end
    /// included); `None` on a separator.
    fn index_at(&self, offset: usize) -> Option<usize> {
        let ix = self.starts.partition_point(|&start| start <= offset).checked_sub(1)?;
        (offset <= self.starts[ix] + self.excerpts[ix].range.len()).then_some(ix)
    }

    /// The excerpt showing composed offset `offset`; `None` on a separator.
    pub fn excerpt_at(&self, offset: usize) -> Option<&Excerpt> {
        self.excerpts.get(self.index_at(offset)?)
    }

    /// The source position of composed offset `offset`; `None` on a separator.
    pub fn to_source(&self, offset: usize) -> Option<(ExcerptId, SourceId, usize)> {
        let ix = self.index_at(offset)?;
        let excerpt = &self.excerpts[ix];
        Some((excerpt.id, excerpt.source, excerpt.range.start + offset - self.starts[ix]))
    }

    /// Where a source position shows, in the first excerpt that contains it.
    pub fn to_composed(&self, source: SourceId, offset: usize) -> Option<usize> {
        self.excerpts
            .iter()
            .zip(&self.starts)
            .find(|(excerpt, _)| excerpt.source == source && excerpt.range.start <= offset && offset <= excerpt.range.end)
            .map(|(excerpt, start)| start + offset - excerpt.range.start)
    }

    /// Takes an edit made in the multibuffer's editor: returns the source
    /// edit to apply, grows or shrinks the excerpt and moves the source's
    /// excerpts after it. An edit touching a separator is refused, and
    /// changes nothing (the editor's read-only ranges already keep the user
    /// from making one).
    pub fn edit_composed(&mut self, edit: &ComposedEdit) -> Result<SourceEdit, EditError> {
        let ix = self.index_at(edit.range.start).ok_or(EditError::CrossesExcerpts)?;
        let composed_start = self.starts[ix];
        if edit.range.end > composed_start + self.excerpts[ix].range.len() {
            return Err(EditError::CrossesExcerpts);
        }
        let excerpt = &mut self.excerpts[ix];
        let start = excerpt.range.start + edit.range.start - composed_start;
        let end = excerpt.range.start + edit.range.end - composed_start;
        excerpt.range.end = excerpt.range.end - (end - start) + edit.new_text.len();
        let source = excerpt.source;
        let delta = edit.new_text.len() as isize - (end - start) as isize;
        for (other, excerpt) in self.excerpts.iter_mut().enumerate() {
            if other != ix && excerpt.source == source && excerpt.range.start >= end {
                excerpt.range = shift(excerpt.range.start, delta)..shift(excerpt.range.end, delta);
            }
        }
        self.restart_from(ix + 1);
        Ok(SourceEdit { source, range: start..end, new_text: edit.new_text.clone() })
    }

    /// Takes an edit made to a source (in its own editor, or on disk):
    /// moves the excerpts after it in that source, and returns the edits
    /// the multibuffer's editor needs for the excerpts that show it, in
    /// the order to apply them. Text inserted at an excerpt's edge goes
    /// into it (an excerpt is whole lines, and text typed at a line's start
    /// or end belongs to that line); an edit that spans an excerpt's edge
    /// trims the excerpt to what's left of it.
    pub fn edit_source(&mut self, edit: &SourceEdit) -> Vec<ComposedEdit> {
        let mut composed_edits = vec![];
        let delta = edit.new_text.len() as isize - edit.range.len() as isize;
        let replaces = !edit.range.is_empty();
        let mut first_changed = None;
        // From the last excerpt, so earlier composed offsets stay valid.
        for ix in (0..self.excerpts.len()).rev() {
            let excerpt = &mut self.excerpts[ix];
            if excerpt.source != edit.source {
                continue;
            }
            let range = excerpt.range.clone();
            if edit.range.end < range.start || (replaces && edit.range.end == range.start) {
                // Before it: it moves.
                excerpt.range = shift(range.start, delta)..shift(range.end, delta);
                first_changed = Some(ix);
                continue;
            }
            if edit.range.start > range.end || (replaces && edit.range.start == range.end) {
                // After it.
                continue;
            }
            // The part of the edit inside the excerpt; the new text only when
            // the whole edit is inside it.
            let inner = edit.range.start.max(range.start)..edit.range.end.min(range.end);
            let inside = range.start <= edit.range.start && edit.range.end <= range.end;
            let inserted = if inside { edit.new_text.clone() } else { String::new() };
            let composed_start = self.starts[ix];
            composed_edits.push(ComposedEdit {
                range: composed_start + inner.start - range.start..composed_start + inner.end - range.start,
                new_text: inserted.clone(),
            });
            let start = if edit.range.start < range.start { edit.range.start + edit.new_text.len() } else { range.start };
            excerpt.range = start..start + range.len() - inner.len() + inserted.len();
            first_changed = Some(ix);
        }
        if let Some(ix) = first_changed {
            self.restart_from(ix);
        }
        composed_edits
    }

    /// Removes excerpt `id`; returns the edit taking its text (and a
    /// separator) out of the composed text.
    pub fn remove_excerpt(&mut self, id: ExcerptId) -> Option<ComposedEdit> {
        let ix = self.index_of(id)?;
        let range = self.starts[ix]..self.starts[ix] + self.excerpts[ix].range.len();
        let range = if ix + 1 < self.excerpts.len() {
            range.start..range.end + 1
        } else if ix > 0 {
            range.start - 1..range.end
        } else {
            range
        };
        self.excerpts.remove(ix);
        self.restart_from(ix.min(self.excerpts.len()));
        Some(ComposedEdit { range, new_text: String::new() })
    }

    /// Shows `range` of excerpt `id`'s source instead, `text` being the
    /// source's text there; returns the edits of the composed text, in the
    /// order to apply them. Growing an excerpt (more context) only inserts
    /// the new text above and below it, so what's in it stays put.
    pub fn set_excerpt_range(&mut self, id: ExcerptId, range: Range<usize>, text: &str) -> Vec<ComposedEdit> {
        let Some(ix) = self.index_of(id) else { return vec![] };
        let old = self.excerpts[ix].range.clone();
        let start = self.starts[ix];
        let edits = if range.start <= old.start && old.end <= range.end && text.len() == range.len() {
            let before = &text[..old.start - range.start];
            let after = &text[old.end - range.start..];
            [
                ComposedEdit { range: start + old.len()..start + old.len(), new_text: after.to_string() },
                ComposedEdit { range: start..start, new_text: before.to_string() },
            ]
            .into_iter()
            .filter(|edit| !edit.new_text.is_empty())
            .collect()
        } else {
            vec![ComposedEdit { range: start..start + old.len(), new_text: text.to_string() }]
        };
        self.excerpts[ix].range = range;
        self.restart_from(ix + 1);
        edits
    }

    /// Shows `range` of excerpt `id`'s source, `text` being the source's
    /// text there, replacing what the excerpt showed (a reloaded file).
    pub fn reset_excerpt(&mut self, id: ExcerptId, range: Range<usize>, text: &str) -> Option<ComposedEdit> {
        let ix = self.index_of(id)?;
        let start = self.starts[ix];
        let old_len = self.excerpts[ix].range.len();
        self.excerpts[ix].range = range;
        self.restart_from(ix + 1);
        Some(ComposedEdit { range: start..start + old_len, new_text: text.to_string() })
    }
}

fn shift(offset: usize, delta: isize) -> usize {
    (offset as isize + delta) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SourceId = SourceId(1);
    const B: SourceId = SourceId(2);

    fn sources(source: SourceId) -> String {
        match source {
            A => "fn a() {}\nlet x = 1;\nlet y = 2;\n".into(),
            _ => "struct B;\nimpl B {}\n".into(),
        }
    }

    fn two_excerpts() -> MultiBuffer {
        let mut mb = MultiBuffer::new();
        mb.push_excerpt(A, 10..20, "a.rs");
        mb.push_excerpt(B, 0..9, "b.rs");
        mb
    }

    #[test]
    fn composes_excerpts_with_separators() {
        let mb = two_excerpts();
        let text = mb.compose(|source, range| sources(source)[range].to_string());
        assert_eq!(text.to_string(), "let x = 1;\nstruct B;");
        assert_eq!(mb.composed_ranges().iter().map(|(_, r)| r.clone()).collect::<Vec<_>>(), [0..10, 11..20]);
        assert_eq!(mb.separators(), [10..11]);
    }

    #[test]
    fn maps_positions_both_ways() {
        let mb = two_excerpts();
        assert_eq!(mb.to_source(4).map(|(_, s, o)| (s, o)), Some((A, 14)));
        assert_eq!(mb.to_source(10).map(|(_, s, o)| (s, o)), Some((A, 20)), "an excerpt's end");
        assert_eq!(mb.to_source(11).map(|(_, s, o)| (s, o)), Some((B, 0)));
        assert_eq!(mb.to_composed(B, 7), Some(18));
        assert_eq!(mb.to_composed(A, 2), None, "not shown");
    }

    #[test]
    fn composed_edits_reach_their_source_and_resize_the_excerpt() {
        let mut mb = two_excerpts();
        let edit = mb.edit_composed(&ComposedEdit { range: 8..9, new_text: "42".into() }).unwrap();
        assert_eq!(edit, SourceEdit { source: A, range: 18..19, new_text: "42".into() });
        assert_eq!(mb.excerpts()[0].range, 10..21);
        assert_eq!(mb.separators(), [11..12]);
        assert_eq!(mb.edit_composed(&ComposedEdit { range: 10..13, new_text: String::new() }), Err(EditError::CrossesExcerpts));
    }

    #[test]
    fn source_edits_move_and_update_excerpts() {
        let mut mb = two_excerpts();
        // Above the excerpt: it moves, the multibuffer's text doesn't change.
        assert!(mb.edit_source(&SourceEdit { source: A, range: 0..0, new_text: "// x\n".into() }).is_empty());
        assert_eq!(mb.excerpts()[0].range, 15..25);
        // Inside it: the multibuffer's text changes there.
        let edits = mb.edit_source(&SourceEdit { source: A, range: 19..20, new_text: "z".into() });
        assert_eq!(edits, [ComposedEdit { range: 4..5, new_text: "z".into() }]);
        // Across its start: trimmed to what's left.
        let edits = mb.edit_source(&SourceEdit { source: A, range: 10..17, new_text: "Q".into() });
        assert_eq!(edits, [ComposedEdit { range: 0..2, new_text: String::new() }]);
        assert_eq!(mb.excerpts()[0].range, 11..19);
        // Another source's excerpt is untouched.
        assert_eq!(mb.excerpts()[1].range, 0..9);
    }

    #[test]
    fn text_inserted_at_an_edge_joins_the_excerpt() {
        let mut mb = two_excerpts();
        // At its start (the line's start) and at its end (the line's end).
        assert_eq!(mb.edit_source(&SourceEdit { source: A, range: 10..10, new_text: "// ".into() }), [ComposedEdit { range: 0..0, new_text: "// ".into() }]);
        assert_eq!(mb.edit_source(&SourceEdit { source: A, range: 23..23, new_text: " ok".into() }), [ComposedEdit { range: 13..13, new_text: " ok".into() }]);
        assert_eq!(mb.excerpts()[0].range, 10..26);
        // Deleting the line before it moves it instead.
        assert!(mb.edit_source(&SourceEdit { source: A, range: 0..10, new_text: String::new() }).is_empty());
        assert_eq!(mb.excerpts()[0].range, 0..16);
        assert_eq!(mb.composed_ranges()[1].1, 17..26);
    }

    #[test]
    fn an_edit_moves_the_sources_later_excerpts() {
        let mut mb = MultiBuffer::new();
        let first = mb.push_excerpt(A, 0..9, "a.rs");
        let second = mb.push_excerpt(A, 21..31, "a.rs");
        mb.edit_composed(&ComposedEdit { range: 3..3, new_text: "bc".into() }).unwrap();
        assert_eq!(mb.excerpt(first).unwrap().range, 0..11);
        assert_eq!(mb.excerpt(second).unwrap().range, 23..33);
        assert_eq!(mb.composed_range(second), Some(12..22));
        assert_eq!(mb.to_source(12), Some((second, A, 23)));
        assert_eq!(mb.excerpt_at(11), mb.excerpt(first), "an excerpt's end is in it");
        assert_eq!(mb.excerpt_at(11 + 1), mb.excerpt(second));
    }

    #[test]
    fn excerpts_grow_and_go() {
        let text = sources(A);
        let mut mb = two_excerpts();
        let id = mb.excerpts()[0].id;
        // Grown by a line each way: only the new lines are inserted, the end first.
        let edits = mb.set_excerpt_range(id, 0..31, &text[0..31]);
        assert_eq!(
            edits,
            [
                ComposedEdit { range: 10..10, new_text: "\nlet y = 2;".into() },
                ComposedEdit { range: 0..0, new_text: "fn a() {}\n".into() },
            ]
        );
        assert_eq!(mb.composed_ranges()[1].1, 32..41);
        let gone = mb.remove_excerpt(id).unwrap();
        assert_eq!(gone.range, 0..32, "with the separator after it");
        assert_eq!(mb.composed_ranges()[0].1, 0..9);
        let last = mb.excerpts()[0].id;
        assert_eq!(mb.remove_excerpt(last).unwrap().range, 0..9);
        assert!(mb.is_empty() && mb.len() == 0);
    }

    /// An editor showing an excerpt of another editor's text, kept in sync
    /// both ways with the engine's public hooks only: what 2.5 builds on.
    #[gpui::test]
    fn an_editor_hosts_excerpts_and_edits_reach_the_source(cx: &mut gpui::TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;

        use gpui::AppContext as _;

        use crate::{BufferEdit, EditorState};

        let (source, mut cx) = crate::test_support::open_editor(cx, &sources(A));
        let source_id = SourceId(source.entity_id().as_u64());
        let model = Rc::new(RefCell::new(MultiBuffer::new()));
        model.borrow_mut().push_excerpt(source_id, 10..20, "a.rs");
        let host = cx.update(|window, cx| {
            let text = model.borrow().compose(|_, range| source.read(cx).text().slice(range).to_string());
            cx.new(|cx| EditorState::new(window, cx).default_value(text.to_string()))
        });
        // Events arrive after the edit that caused them, so each side keeps
        // the edits it forwarded and ignores them when they come back.
        let to_source: Rc<RefCell<Vec<BufferEdit>>> = Default::default();
        let to_host: Rc<RefCell<Vec<BufferEdit>>> = Default::default();
        let _subscriptions = cx.update(|window, cx| {
            let handle = window.window_handle();
            let host_to_source = {
                let (model, source, to_source, to_host) = (model.clone(), source.clone(), to_source.clone(), to_host.clone());
                cx.subscribe(&host, move |_, edit: &BufferEdit, cx| {
                    if to_host.borrow().first() == Some(edit) {
                        to_host.borrow_mut().remove(0);
                        return;
                    }
                    let edit = ComposedEdit { range: edit.range.clone(), new_text: edit.new_text.clone() };
                    let Ok(edit) = model.borrow_mut().edit_composed(&edit) else { return };
                    to_source.borrow_mut().push(BufferEdit { range: edit.range.clone(), new_text: edit.new_text.clone() });
                    let source = source.clone();
                    handle
                        .update(cx, |_, window, cx| {
                            source.update(cx, |source, cx| source.edit([(edit.range, edit.new_text)], window, cx))
                        })
                        .ok();
                })
            };
            let source_to_host = {
                let (model, host, to_source, to_host) = (model.clone(), host.clone(), to_source.clone(), to_host.clone());
                cx.subscribe(&source, move |_, edit: &BufferEdit, cx| {
                    if to_source.borrow().first() == Some(edit) {
                        to_source.borrow_mut().remove(0);
                        return;
                    }
                    let edit = SourceEdit { source: source_id, range: edit.range.clone(), new_text: edit.new_text.clone() };
                    let edits = model.borrow_mut().edit_source(&edit);
                    to_host.borrow_mut().extend(edits.iter().map(|e| BufferEdit { range: e.range.clone(), new_text: e.new_text.clone() }));
                    let host = host.clone();
                    handle
                        .update(cx, |_, window, cx| {
                            host.update(cx, |host, cx| host.edit(edits.into_iter().map(|e| (e.range, e.new_text)), window, cx))
                        })
                        .ok();
                })
            };
            (host_to_source, source_to_host)
        });
        assert_eq!(host.read_with(&cx, |h, _| h.text().to_string()), "let x = 1;");

        // Typing in the excerpt edits the source.
        cx.update(|window, cx| host.update(cx, |h, cx| h.edit([(8..9, "42")], window, cx)));
        cx.run_until_parked();
        assert_eq!(source.read_with(&cx, |s, _| s.text().to_string()), "fn a() {}\nlet x = 42;\nlet y = 2;\n");
        // Editing the source above the excerpt moves it; inside it updates it.
        cx.update(|window, cx| source.update(cx, |s, cx| s.edit([(0..0, "// top\n")], window, cx)));
        cx.run_until_parked();
        assert_eq!(host.read_with(&cx, |h, _| h.text().to_string()), "let x = 42;");
        cx.update(|window, cx| source.update(cx, |s, cx| s.edit([(21..22, "z")], window, cx)));
        cx.run_until_parked();
        assert_eq!(host.read_with(&cx, |h, _| h.text().to_string()), "let z = 42;");
        assert_eq!(model.borrow().excerpts()[0].range, 17..28);
    }
}

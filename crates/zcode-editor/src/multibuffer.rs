//! The excerpt model a multibuffer is built on (roadmap 2.5): one editor
//! showing editable excerpts of many files, as project search results and
//! diagnostics do in Zed.
//!
//! This is the model only: which ranges of which source files are shown,
//! the text an editor shows for them (each excerpt's text, separated by a
//! newline), and the translation of positions and edits between that text
//! and the sources, both ways. `docs/editor-engine.md` describes how an
//! `EditorState` hosts it with the engine's hooks: `BufferEdit` events to
//! forward edits, read-only ranges over the separators, blocks for the
//! excerpt headers, a gutter lane for the source line numbers.
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
        self.excerpts.push(Excerpt { id, source, range, header: header.into() });
        id
    }

    pub fn excerpts(&self) -> &[Excerpt] {
        &self.excerpts
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
        let mut start = 0;
        self.excerpts
            .iter()
            .map(|excerpt| {
                let range = start..start + excerpt.range.len();
                start = range.end + 1;
                (excerpt.id, range)
            })
            .collect()
    }

    /// The newlines between excerpts, for the editor's read-only ranges.
    pub fn separators(&self) -> Vec<Range<usize>> {
        let ranges = self.composed_ranges();
        ranges.iter().take(ranges.len().saturating_sub(1)).map(|(_, r)| r.end..r.end + 1).collect()
    }

    /// The source position of composed offset `offset`; `None` on a separator.
    pub fn to_source(&self, offset: usize) -> Option<(ExcerptId, SourceId, usize)> {
        let ranges = self.composed_ranges();
        let ix = ranges.iter().position(|(_, r)| offset <= r.end)?;
        let range = &ranges[ix].1;
        if offset < range.start {
            return None;
        }
        let excerpt = &self.excerpts[ix];
        Some((excerpt.id, excerpt.source, excerpt.range.start + offset - range.start))
    }

    /// Where a source position shows, in the first excerpt that contains it.
    pub fn to_composed(&self, source: SourceId, offset: usize) -> Option<usize> {
        self.composed_ranges()
            .into_iter()
            .zip(&self.excerpts)
            .find(|(_, excerpt)| excerpt.source == source && excerpt.range.start <= offset && offset <= excerpt.range.end)
            .map(|((_, range), excerpt)| range.start + offset - excerpt.range.start)
    }

    /// Takes an edit made in the multibuffer's editor: returns the source
    /// edit to apply, and grows or shrinks the excerpt. An edit touching a
    /// separator is refused (the editor's read-only ranges already keep the
    /// user from making one).
    pub fn edit_composed(&mut self, edit: &ComposedEdit) -> Result<SourceEdit, EditError> {
        let ranges = self.composed_ranges();
        let ix = ranges
            .iter()
            .position(|(_, r)| r.start <= edit.range.start && edit.range.end <= r.end)
            .ok_or(EditError::CrossesExcerpts)?;
        let composed = &ranges[ix].1;
        let excerpt = &mut self.excerpts[ix];
        let start = excerpt.range.start + edit.range.start - composed.start;
        let end = excerpt.range.start + edit.range.end - composed.start;
        excerpt.range.end = excerpt.range.end - (end - start) + edit.new_text.len();
        Ok(SourceEdit { source: excerpt.source, range: start..end, new_text: edit.new_text.clone() })
    }

    /// Takes an edit made to a source (in its own editor, or on disk):
    /// moves the excerpts after it in that source, and returns the edits
    /// the multibuffer's editor needs for the excerpts that show it. Text
    /// inserted at an excerpt's edge stays outside it; an edit that spans an
    /// excerpt's edge trims the excerpt to what's left of it.
    pub fn edit_source(&mut self, edit: &SourceEdit) -> Vec<ComposedEdit> {
        let ranges = self.composed_ranges();
        let mut composed_edits = vec![];
        let delta = edit.new_text.len() as isize - edit.range.len() as isize;
        let shift = |offset: usize| (offset as isize + delta) as usize;
        // From the last excerpt, so earlier composed offsets stay valid.
        for (ix, excerpt) in self.excerpts.iter_mut().enumerate().rev() {
            if excerpt.source != edit.source {
                continue;
            }
            let range = excerpt.range.clone();
            if edit.range.end <= range.start {
                // Before it, or inserted at its start: it moves.
                excerpt.range = shift(range.start)..shift(range.end);
                continue;
            }
            if edit.range.start >= range.end {
                // After it, or inserted at its end.
                continue;
            }
            // The part of the edit inside the excerpt; the new text only when
            // the whole edit is inside it.
            let inner = edit.range.start.max(range.start)..edit.range.end.min(range.end);
            let inside = range.start <= edit.range.start && edit.range.end <= range.end;
            let inserted = if inside { edit.new_text.clone() } else { String::new() };
            let composed_start = ranges[ix].1.start;
            composed_edits.push(ComposedEdit {
                range: composed_start + inner.start - range.start..composed_start + inner.end - range.start,
                new_text: inserted.clone(),
            });
            let start = if edit.range.start < range.start { edit.range.start + edit.new_text.len() } else { range.start };
            excerpt.range = start..start + range.len() - inner.len() + inserted.len();
        }
        composed_edits
    }
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

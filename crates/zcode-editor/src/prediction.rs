//! Edit predictions (3.3): text a provider predicts at the cursor, shown as
//! ghost text until Tab accepts it or Escape dismisses it.
//!
//! The editor doesn't ask anyone for predictions: its host sets one
//! (`set_edit_prediction`), streams more of it in (`update_edit_prediction`)
//! and the editor shows it with its own annotations, an inlay for the first
//! line at the insertion point and a block for the lines after it, so the
//! text after the caret moves aside instead of being painted over. A
//! prediction that replaces text strikes that text through.
//!
//! A prediction sits on two anchors, so edits elsewhere move it. Typing what
//! it predicts keeps it, and what's left of it stays on screen; anything
//! else (other text, a caret moved away, a selection, a second cursor, Vim
//! leaving insert mode, a read-only range) drops it. While the completion
//! menu is open it hides and comes back when the menu closes. All of this is
//! checked after each change by an observer that exists only while there is
//! a prediction, so an editor without one does nothing new per keystroke.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{Context, HighlightStyle, SharedString, StrikethroughStyle, Subscription, Window, px};

use crate::{Anchor, AnchorBias, Block, EditorState, HighlightPriority, Inlay, RopeExt as _};

/// The inlay, block and highlight groups a prediction draws with.
const KEY: &str = "edit-prediction";
/// Over search matches and references: the struck-through text is what
/// Tab would replace.
const PRIORITY: i32 = HighlightPriority::SEARCH + 100;
/// Longer than this and a prediction is dropped rather than compared.
const MAX_LEN: usize = 64 * 1024;

/// Text to put in place of `range` (empty for an insertion).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditPrediction {
    pub range: Range<usize>,
    pub text: String,
}

impl EditPrediction {
    pub fn insertion(offset: usize, text: impl Into<String>) -> Self {
        Self { range: offset..offset, text: text.into() }
    }

    pub fn replacement(range: Range<usize>, text: impl Into<String>) -> Self {
        Self { range, text: text.into() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditPredictionId(u64);

/// What an editor shows of its prediction now: `text` in place of `range`,
/// both trimmed to what differs (an insertion when `range` is empty), and
/// what typing it has used up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShownEditPrediction {
    pub id: EditPredictionId,
    pub range: Range<usize>,
    pub text: String,
}

/// A prediction left the editor: Tab took it, or it was dropped (dismissed,
/// replaced, typed over, the caret moved away).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditPredictionEvent {
    Accepted { id: EditPredictionId, text: String },
    Discarded { id: EditPredictionId },
}

impl gpui::EventEmitter<EditPredictionEvent> for EditorState {}

pub(crate) struct PredictionState {
    id: EditPredictionId,
    /// The start stays before text typed at it, the end moves past it, so
    /// for an insertion `start..end` is what has been typed since.
    start: Anchor,
    end: Anchor,
    old: String,
    new: String,
    /// What's on screen (`None` while hidden), to tell whether to redraw.
    shown: Option<ShownEditPrediction>,
    _observer: Subscription,
}

enum Check {
    Show(ShownEditPrediction),
    Hide,
    Drop,
}

impl EditorState {
    /// Shows `prediction`, replacing any other; `None` (nothing shown) when
    /// the editor can't take it now: read-only, a read-only range, several
    /// cursors or a selection, the caret away from it, Vim outside insert
    /// mode.
    pub fn set_edit_prediction(&mut self, prediction: EditPrediction, cx: &mut Context<Self>) -> Option<EditPredictionId> {
        self.dismiss_edit_prediction(cx);
        let len = self.text.len();
        let range = prediction.range.start.min(len)..prediction.range.end.min(len);
        if range.end < range.start
            || range.len() + prediction.text.len() > MAX_LEN
            || !self.text.is_char_boundary(range.start)
            || !self.text.is_char_boundary(range.end)
        {
            return None;
        }
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = EditPredictionId(NEXT.fetch_add(1, Ordering::Relaxed));
        let start = self.anchor(range.start, AnchorBias::Left);
        let end = self.anchor(range.end, AnchorBias::Right);
        let old = self.text.slice(range).to_string();
        let observer = cx.observe_self(|state, cx| state.sync_edit_prediction(cx));
        self.extras.prediction = Some(PredictionState { id, start, end, old, new: prediction.text, shown: None, _observer: observer });
        self.sync_edit_prediction(cx);
        // The observer starts with the next effects; this covers changes
        // made in the rest of this update.
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            this.update(cx, |state, cx| state.sync_edit_prediction(cx)).ok();
        });
        self.extras.prediction.as_ref().map(|p| p.id)
    }

    /// Replaces prediction `id`'s text, as more of it streams in; false when
    /// it's gone (typed over, dismissed), so the stream can stop.
    pub fn update_edit_prediction(&mut self, id: EditPredictionId, text: String, cx: &mut Context<Self>) -> bool {
        match &mut self.extras.prediction {
            Some(state) if state.id == id => {
                if state.old.len() + text.len() > MAX_LEN {
                    self.dismiss_edit_prediction(cx);
                    return false;
                }
                state.new = text;
                self.sync_edit_prediction(cx);
                self.extras.prediction.as_ref().is_some_and(|p| p.id == id)
            }
            _ => false,
        }
    }

    /// The prediction on screen, if any.
    pub fn edit_prediction(&self) -> Option<&ShownEditPrediction> {
        self.extras.prediction.as_ref().and_then(|p| p.shown.as_ref())
    }

    /// The prediction this editor holds, shown or hidden behind the
    /// completion menu.
    pub fn edit_prediction_id(&self) -> Option<EditPredictionId> {
        self.extras.prediction.as_ref().map(|p| p.id)
    }

    /// Whether a prediction is on screen.
    pub fn has_edit_prediction(&self) -> bool {
        self.edit_prediction().is_some()
    }

    /// Types the prediction on screen (Tab), as one undo step that puts the
    /// caret back before it; false when there's none.
    pub fn accept_edit_prediction(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(shown) = self.edit_prediction().cloned() else { return false };
        self.clear_edit_prediction(cx);
        let end = shown.range.start + shown.text.len();
        let applied = self.transact(window, cx, |state, window, cx| {
            state.edit([(shown.range.clone(), shown.text.clone())], window, cx);
            state.set_selected_range(end..end, cx);
        });
        if applied.is_none() {
            return false;
        }
        cx.emit(EditPredictionEvent::Accepted { id: shown.id, text: shown.text });
        true
    }

    /// Drops the prediction (Escape, ⌥]); false when there was none.
    pub fn dismiss_edit_prediction(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.clear_edit_prediction(cx) else { return false };
        cx.emit(EditPredictionEvent::Discarded { id });
        true
    }

    fn clear_edit_prediction(&mut self, cx: &mut Context<Self>) -> Option<EditPredictionId> {
        let state = self.extras.prediction.take()?;
        self.remove_anchor(state.start);
        self.remove_anchor(state.end);
        if state.shown.is_some() {
            self.erase_edit_prediction(cx);
        }
        Some(state.id)
    }

    /// After any change: what's left of the prediction shown, or nothing.
    fn sync_edit_prediction(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.extras.prediction.as_ref() else { return };
        match self.check_edit_prediction(state) {
            Check::Drop => {
                self.dismiss_edit_prediction(cx);
            }
            Check::Hide => {
                if let Some(state) = self.extras.prediction.as_mut()
                    && state.shown.take().is_some()
                {
                    self.erase_edit_prediction(cx);
                }
            }
            Check::Show(shown) => {
                if state.shown.as_ref() == Some(&shown) {
                    return;
                }
                self.draw_edit_prediction(&shown, cx);
                if let Some(state) = self.extras.prediction.as_mut() {
                    state.shown = Some(shown);
                }
            }
        }
    }

    fn check_edit_prediction(&self, state: &PredictionState) -> Check {
        let vim_mode = self.extras.vim.as_ref().map(|vim| vim.mode());
        if !self.is_editable() || vim_mode.is_some_and(|mode| mode != crate::VimMode::Insert) {
            return Check::Drop;
        }
        if !self.selections.is_single() || !self.selected_range().is_empty() {
            return Check::Drop;
        }
        let (Some(start), Some(end)) = (self.anchor_offset(state.start), self.anchor_offset(state.end)) else {
            return Check::Drop;
        };
        if end < start || end - start > state.old.len() + state.new.len() {
            return Check::Drop;
        }
        let current = self.text.slice(start..end).to_string();
        let cursor = self.cursor();
        let (range, text) = if current == state.old {
            if cursor < start || cursor > end {
                return Check::Drop;
            }
            (start..end, state.new.as_str())
        } else if state.old.is_empty() && cursor == end && state.new.len() > current.len() && state.new.starts_with(&current) {
            // Typed what it predicts: the rest is still predicted.
            (end..end, &state.new[current.len()..])
        } else {
            return Check::Drop;
        };
        let Some((range, text)) = trim(&self.text.slice(range.clone()).to_string(), text, range.start) else {
            return Check::Hide;
        };
        if self.refuses_edit(&range) {
            return Check::Drop;
        }
        if self.extras.context_menu_content.completion.open || self.extras.context_menu_content.code_action.open {
            return Check::Hide;
        }
        Check::Show(ShownEditPrediction { id: state.id, range, text })
    }

    /// The first line as an inlay where the text goes, the rest as a block
    /// below that line, the replaced text struck through.
    fn draw_edit_prediction(&mut self, shown: &ShownEditPrediction, cx: &mut Context<Self>) {
        let color = self.editor_style.muted_foreground.opacity(0.75);
        let mut lines = shown.text.split('\n');
        let first = lines.next().unwrap_or_default();
        let rest: Vec<SharedString> = lines.map(|line| SharedString::from(line.replace('\t', "    "))).collect();
        let at = shown.range.end;
        let row = self.text.offset_to_point(at).row as u32;
        let inlays = if first.is_empty() { vec![] } else { vec![Inlay::new(at, first.replace('\t', "    ")).color(color)] };
        self.set_inlays(KEY, inlays, cx);
        let blocks = if rest.is_empty() { vec![] } else { vec![Block::new(row, rest).color(color)] };
        self.set_blocks(KEY, blocks, cx);
        if shown.range.is_empty() {
            self.clear_highlight_layer(KEY, cx);
        } else {
            let style = HighlightStyle {
                strikethrough: Some(StrikethroughStyle { thickness: px(1.), color: Some(color) }),
                background_color: Some(color.opacity(0.12)),
                ..Default::default()
            };
            self.set_highlight_layer(KEY, PRIORITY, vec![(shown.range.clone(), style)], cx);
        }
    }

    fn erase_edit_prediction(&mut self, cx: &mut Context<Self>) {
        self.clear_inlays(KEY, cx);
        self.clear_blocks(KEY, cx);
        self.clear_highlight_layer(KEY, cx);
    }
}

/// `new` in place of `old` (which starts at `offset`), without what they
/// share at either end; `None` when they're the same.
fn trim(old: &str, new: &str, offset: usize) -> Option<(Range<usize>, String)> {
    let prefix = common_prefix(old, new);
    let (old_rest, new_rest) = (&old[prefix..], &new[prefix..]);
    let suffix = common_suffix(old_rest, new_rest);
    let removed = &old_rest[..old_rest.len() - suffix];
    let inserted = &new_rest[..new_rest.len() - suffix];
    if removed.is_empty() && inserted.is_empty() {
        return None;
    }
    let start = offset + prefix;
    Some((start..start + removed.len(), inserted.to_string()))
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).map(|(x, _)| x.len_utf8()).sum()
}

fn common_suffix(a: &str, b: &str) -> usize {
    a.chars().rev().zip(b.chars().rev()).take_while(|(x, y)| x == y).map(|(x, _)| x.len_utf8()).sum()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::test_support::open_editor;

    #[test]
    fn trimming_keeps_what_differs() {
        assert_eq!(trim("", "abc", 5), Some((5..5, "abc".into())));
        assert_eq!(trim("foo", "foobar", 0), Some((3..3, "bar".into())));
        assert_eq!(trim("(x)", "(x, y)", 10), Some((12..12, ", y".into())));
        assert_eq!(trim("let a = 1", "let b = 1", 0), Some((4..5, "b".into())));
        assert_eq!(trim("same", "same", 0), None);
        assert_eq!(trim("é", "éa", 0), Some((2..2, "a".into())));
        assert_eq!(trim("aé", "bé", 0), Some((0..1, "b".into())));
    }

    fn shown(editor: &gpui::Entity<EditorState>, cx: &gpui::VisualTestContext) -> Option<(Range<usize>, String)> {
        editor.read_with(cx, |state, _| state.edit_prediction().map(|p| (p.range.clone(), p.text.clone())))
    }

    #[gpui::test]
    fn a_prediction_draws_an_inlay_and_a_block_and_tab_types_it(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "func f() {\n    \n}\n");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.focus(window, cx);
                state.set_selected_range(15..15, cx);
                state.set_edit_prediction(EditPrediction::insertion(15, "let x = 1\n    print(x)"), cx).unwrap();
            })
        });
        editor.read_with(&cx, |state, _| {
            assert_eq!(state.inlays(KEY).iter().map(|i| (i.offset, i.text.to_string())).collect::<Vec<_>>(), [(15, "let x = 1".into())]);
            let blocks = state.blocks(KEY);
            assert_eq!((blocks[0].row, blocks[0].lines.len()), (1, 1));
            assert_eq!(blocks[0].lines[0].as_ref(), "    print(x)");
            assert_eq!(state.text().to_string(), "func f() {\n    \n}\n", "not in the buffer");
        });
        cx.simulate_keystrokes("tab");
        editor.read_with(&cx, |state, _| {
            assert_eq!(state.text().to_string(), "func f() {\n    let x = 1\n    print(x)\n}\n");
            assert_eq!(state.cursor(), 15 + "let x = 1\n    print(x)".len());
            assert!(state.inlays(KEY).is_empty() && state.blocks(KEY).is_empty());
            assert!(!state.has_edit_prediction());
        });
        // One undo step, the caret back where it was.
        cx.update(|window, cx| editor.update(cx, |state, cx| state.undo(window, cx)));
        editor.read_with(&cx, |state, _| {
            assert_eq!(state.text().to_string(), "func f() {\n    \n}\n");
            assert_eq!(state.cursor(), 15);
        });
    }

    gpui::actions!(prediction_test, [Accept]);

    #[gpui::test]
    fn bindings_for_edit_prediction_apply_while_one_shows(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "ab\n");
        let accepted = std::rc::Rc::new(std::cell::Cell::new(0));
        cx.update(|window, cx| {
            cx.bind_keys([gpui::KeyBinding::new("ctrl-y", Accept, Some("Editor && edit_prediction"))]);
            let accepted = accepted.clone();
            cx.on_action(move |_: &Accept, _| accepted.set(accepted.get() + 1));
            editor.update(cx, |state, cx| state.focus(window, cx));
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-y");
        assert_eq!(accepted.get(), 0, "no prediction, no binding");
        cx.update(|_, cx| {
            editor.update(cx, |state, cx| {
                state.set_selected_range(2..2, cx);
                state.set_edit_prediction(EditPrediction::insertion(2, "c"), cx);
            })
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("ctrl-y");
        assert_eq!(accepted.get(), 1);
    }

    #[gpui::test]
    fn typing_what_it_predicts_keeps_the_rest(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "let a = \n");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.focus(window, cx);
                state.set_selected_range(8..8, cx);
                state.set_edit_prediction(EditPrediction::insertion(8, "foo(bar)"), cx);
            })
        });
        cx.simulate_input("fo");
        assert_eq!(shown(&editor, &cx), Some((10..10, "o(bar)".into())));
        // Backspace gives a character of it back.
        cx.simulate_keystrokes("backspace");
        assert_eq!(shown(&editor, &cx), Some((9..9, "oo(bar)".into())));
        cx.simulate_input("x");
        assert_eq!(shown(&editor, &cx), None, "typed something else");
        assert_eq!(editor.read_with(&cx, |state, _| state.edit_prediction_id()), None);
    }

    #[gpui::test]
    fn moving_away_selecting_or_escape_drops_it(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "abc\ndef\n");
        let set = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                editor.update(cx, |state, cx| {
                    state.focus(window, cx);
                    state.set_selected_range(3..3, cx);
                    state.set_edit_prediction(EditPrediction::insertion(3, "ghi"), cx);
                })
            });
            assert_eq!(shown(&editor, cx), Some((3..3, "ghi".into())));
        };
        set(&mut cx);
        cx.simulate_keystrokes("left");
        assert_eq!(shown(&editor, &cx), None);
        set(&mut cx);
        cx.simulate_keystrokes("shift-left");
        assert_eq!(shown(&editor, &cx), None);
        set(&mut cx);
        cx.simulate_keystrokes("escape");
        assert_eq!(shown(&editor, &cx), None);
        assert_eq!(editor.read_with(&cx, |state, _| state.text().to_string()), "abc\ndef\n");
    }

    #[gpui::test]
    fn edits_elsewhere_move_it(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "one\ntwo\n");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_selected_range(7..7, cx);
                state.set_edit_prediction(EditPrediction::insertion(7, "!"), cx);
                // As another pane's edit of the same buffer would: the
                // caret moves past it too.
                state.edit([(0..0, "zero\n")], window, cx);
                state.set_selected_range(12..12, cx);
            })
        });
        assert_eq!(shown(&editor, &cx), Some((12..12, "!".into())));
    }

    #[gpui::test]
    fn a_replacement_strikes_the_old_text_through(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "let total = a + b\n");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.focus(window, cx);
                state.set_selected_range(17..17, cx);
                // The whole line again, with one name changed: only that shows.
                state.set_edit_prediction(EditPrediction::replacement(0..17, "let total = a + c"), cx);
            })
        });
        assert_eq!(shown(&editor, &cx), Some((16..17, "c".into())));
        editor.read_with(&cx, |state, _| assert_eq!(state.highlight_layer_ranges(KEY), [16..17]));
        cx.simulate_keystrokes("tab");
        editor.read_with(&cx, |state, _| {
            assert_eq!(state.text().to_string(), "let total = a + c\n");
            assert!(state.highlight_layer_ranges(KEY).is_empty());
        });
    }

    #[gpui::test]
    fn streaming_extends_it(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "x\n");
        let id = cx.update(|_, cx| {
            editor.update(cx, |state, cx| {
                state.set_selected_range(1..1, cx);
                state.set_edit_prediction(EditPrediction::insertion(1, " ="), cx).unwrap()
            })
        });
        let kept = cx.update(|_, cx| editor.update(cx, |state, cx| state.update_edit_prediction(id, " = 42".into(), cx)));
        assert!(kept);
        assert_eq!(shown(&editor, &cx), Some((1..1, " = 42".into())));
        cx.update(|_, cx| editor.update(cx, |state, cx| state.dismiss_edit_prediction(cx)));
        let kept = cx.update(|_, cx| editor.update(cx, |state, cx| state.update_edit_prediction(id, " = 43".into(), cx)));
        assert!(!kept, "a dismissed prediction stops the stream");
    }

    #[gpui::test]
    fn read_only_text_takes_none(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "header\nbody\n");
        cx.update(|_, cx| {
            editor.update(cx, |state, cx| {
                state.set_read_only_ranges(vec![0..7], cx);
                state.set_selected_range(3..3, cx);
                assert_eq!(state.set_edit_prediction(EditPrediction::insertion(3, "x"), cx), None);
                state.set_selected_range(11..11, cx);
                assert!(state.set_edit_prediction(EditPrediction::insertion(11, "y"), cx).is_some());
                state.set_read_only(true, cx);
                assert!(!state.has_edit_prediction());
                state.set_selected_range(11..11, cx);
                assert_eq!(state.set_edit_prediction(EditPrediction::insertion(11, "y"), cx), None);
            })
        });
    }

    #[gpui::test]
    fn vim_takes_it_in_insert_mode_only(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "abc\n");
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.focus(window, cx);
                state.set_vim_enabled(true, cx);
                state.set_selected_range(1..1, cx);
                assert_eq!(state.set_edit_prediction(EditPrediction::insertion(1, "x"), cx), None, "normal mode");
            })
        });
        cx.simulate_keystrokes("A");
        cx.update(|_, cx| {
            editor.update(cx, |state, cx| {
                let at = state.cursor();
                assert!(state.set_edit_prediction(EditPrediction::insertion(at, "def"), cx).is_some(), "insert mode");
            })
        });
        cx.simulate_keystrokes("escape");
        assert_eq!(shown(&editor, &cx), None);
    }
}

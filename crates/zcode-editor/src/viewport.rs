//! Which rows an editor shows, and an event when that changes.
//!
//! Language features that are only worth fetching for what's on screen
//! (inlay hints) read [`EditorState::visible_rows`] and subscribe to
//! [`ViewportChanged`], which the element emits after a frame that shows
//! different rows than the one before: a scroll, a resize, a fold, or text
//! inserted above. A frame showing the same rows emits nothing, so typing
//! on screen doesn't.

use std::ops::Range;

use gpui::EventEmitter;

use crate::{EditorState, InputBaseState, InputModeKind};

/// The rows shown changed to `rows` (0-based buffer rows, end exclusive).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewportChanged {
    pub rows: Range<u32>,
}

impl<M: InputModeKind> EventEmitter<ViewportChanged> for InputBaseState<M> {}

/// The buffer rows a layout shows, end exclusive (folded rows inside it
/// included).
pub(crate) fn rows_of(visible_buffer_lines: &[usize]) -> Option<Range<u32>> {
    let first = *visible_buffer_lines.first()?;
    let last = *visible_buffer_lines.last()?;
    Some(first as u32..last as u32 + 1)
}

impl EditorState {
    /// The buffer rows shown in the last frame, end exclusive; `None` before
    /// the first.
    pub fn visible_rows(&self) -> Option<Range<u32>> {
        rows_of(&self.last_layout.as_ref()?.visible_buffer_lines)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::TestAppContext;

    use super::*;
    use crate::test_support::open_editor;

    #[test]
    fn rows_span_the_first_to_the_last_line_shown() {
        assert_eq!(rows_of(&[]), None);
        assert_eq!(rows_of(&[4]), Some(4..5));
        // A fold between them: the folded rows count as shown.
        assert_eq!(rows_of(&[4, 5, 20, 21]), Some(4..22));
    }

    #[gpui::test]
    fn scrolling_reports_the_new_rows_once(cx: &mut TestAppContext) {
        let text = (0..200).map(|i| format!("line {i}\n")).collect::<String>();
        let (editor, mut cx) = open_editor(cx, &text);
        let seen: Rc<RefCell<Vec<Range<u32>>>> = Default::default();
        cx.update(|_, cx| {
            let seen = seen.clone();
            cx.subscribe(&editor, move |_, event: &ViewportChanged, _| seen.borrow_mut().push(event.rows.clone())).detach();
        });
        let first = editor.read_with(&cx, |state, _| state.visible_rows()).expect("rows after a frame");
        assert_eq!(first.start, 0);
        assert!(first.end > 5 && first.end < 60, "{first:?}");

        // A frame with nothing new emits nothing.
        editor.update(&mut cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert!(seen.borrow().is_empty());

        // Revealing a row far down scrolls; the event carries the new rows.
        let offset = text.find("line 150").unwrap();
        editor.update(&mut cx, |state, cx| {
            state.set_selected_range(offset..offset, cx);
            state.scroll_to(offset, None, cx);
        });
        cx.run_until_parked();
        let rows = editor.read_with(&cx, |state, _| state.visible_rows()).unwrap();
        assert!(rows.contains(&150), "{rows:?}");
        assert_eq!(seen.borrow().last(), Some(&rows));
    }
}

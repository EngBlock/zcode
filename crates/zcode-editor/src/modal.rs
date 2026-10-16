//! What a modal editing layer (Vim) needs from the editor: the keystrokes
//! before the keymap sees them, the caret's shape, and a key context of its
//! own for its bindings.
//!
//! The interceptor runs before key bindings and text input (GPUI's
//! keystroke interceptors run first): returning `true` consumes the
//! keystroke, so `j` in normal mode moves instead of typing, and `escape`
//! leaves insert mode instead of closing a popover. It only sees keystrokes
//! while this editor has focus. Pending multi-key commands (`d` `w`) are the
//! layer's own state; the selection and editing APIs (`set_selected_range`,
//! `add_selection`, `edit`, `transact`, `undo`) do the rest.

use std::rc::Rc;

use gpui::{Context, Keystroke, SharedString, Window};

use crate::EditorState;

/// How the caret is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    /// A thin bar between characters (insert mode, and the default).
    #[default]
    Bar,
    /// A box over the character after the caret, which shows through in the
    /// background color (normal mode).
    Block,
    /// A line under the character after the caret (replace mode).
    Underline,
    /// A box outline over the character (an unfocused block caret).
    Hollow,
}

/// Decides about a keystroke before the keymap and text input do; returns
/// whether it consumed it.
pub type KeystrokeInterceptor =
    Rc<dyn Fn(&mut EditorState, &Keystroke, &mut Window, &mut Context<EditorState>) -> bool>;

impl EditorState {
    pub fn cursor_shape(&self) -> CursorShape {
        self.cursor_shape
    }

    pub fn set_cursor_shape(&mut self, shape: CursorShape, cx: &mut Context<Self>) {
        if self.cursor_shape != shape {
            self.cursor_shape = shape;
            cx.notify();
        }
    }

    /// Installs (or with `None` removes) the keystroke interceptor.
    pub fn set_keystroke_interceptor(
        &mut self,
        interceptor: Option<KeystrokeInterceptor>,
        cx: &mut Context<Self>,
    ) {
        self.keystroke_interception = interceptor.map(|interceptor| {
            let editor = cx.weak_entity();
            let focus = self.focus_handle.clone();
            let subscription = cx.intercept_keystrokes(move |event, window, cx| {
                if !focus.is_focused(window) {
                    return;
                }
                let handled = editor
                    .update(cx, |state, cx| interceptor(state, &event.keystroke, window, cx))
                    .unwrap_or(false);
                if handled {
                    cx.stop_propagation();
                }
            });
            subscription
        });
    }

    /// Adds `context` (such as `vim_mode=normal`) to the editor's key
    /// context, beside `Input mode=editor`, for a layer's own key bindings.
    pub fn set_extra_key_context(&mut self, context: Option<SharedString>, cx: &mut Context<Self>) {
        self.extra_key_context = context;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use gpui::{KeyBinding, TestAppContext, actions};

    use super::*;
    use crate::test_support::open_editor;

    actions!(modal_test, [VimDown]);

    #[gpui::test]
    fn an_interceptor_takes_keystrokes_before_bindings_and_typing(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "one\ntwo\n");
        let seen = Rc::new(std::cell::RefCell::new(vec![]));
        cx.update(|window, cx| {
            let seen = seen.clone();
            editor.update(cx, |state, cx| {
                state.focus(window, cx);
                // A tiny normal mode: `j` moves down, `x` deletes a character,
                // anything else (like `i`) goes through.
                state.set_keystroke_interceptor(
                    Some(Rc::new(move |state, keystroke, window, cx| {
                        seen.borrow_mut().push(keystroke.key.clone());
                        match keystroke.key.as_str() {
                            "j" => {
                                let next = state.text().to_string().find('\n').map_or(0, |i| i + 1);
                                state.set_selected_range(next..next, cx);
                                true
                            }
                            "x" => {
                                let at = state.cursor();
                                state.transact(window, cx, |state, window, cx| state.edit([(at..at + 1, "")], window, cx));
                                true
                            }
                            _ => false,
                        }
                    })),
                    cx,
                );
                state.set_cursor_shape(CursorShape::Block, cx);
            })
        });
        cx.simulate_keystrokes("j x");
        editor.read_with(&cx, |state, _| {
            assert_eq!(state.text().to_string(), "one\nwo\n");
            assert_eq!(state.cursor_shape(), CursorShape::Block);
        });
        // Unhandled keystrokes type as usual.
        cx.simulate_keystrokes("i");
        assert_eq!(editor.read_with(&cx, |state, _| state.text().to_string()), "one\niwo\n");
        assert_eq!(*seen.borrow(), ["j", "x", "i"]);
        // Removed, `j` types again.
        cx.update(|_, cx| editor.update(cx, |state, cx| state.set_keystroke_interceptor(None, cx)));
        cx.simulate_keystrokes("j");
        assert_eq!(editor.read_with(&cx, |state, _| state.text().to_string()), "one\nijwo\n");
    }

    #[gpui::test]
    fn an_extra_key_context_selects_bindings(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "one\ntwo\n");
        let hits = Rc::new(std::cell::Cell::new(0));
        cx.update(|window, cx| {
            cx.bind_keys([KeyBinding::new("k", VimDown, Some("Input && vim_mode == normal"))]);
            let hits = hits.clone();
            editor.update(cx, |state, cx| {
                state.focus(window, cx);
                state.set_extra_key_context(Some("vim_mode=normal".into()), cx);
            });
            cx.on_action(move |_: &VimDown, _| hits.set(hits.get() + 1));
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("k");
        assert_eq!(hits.get(), 1);
        assert_eq!(editor.read_with(&cx, |state, _| state.text().to_string()), "one\ntwo\n");
    }

    #[gpui::test]
    fn caret_shapes_cover_the_next_character(cx: &mut TestAppContext) {
        let (editor, mut cx) = open_editor(cx, "abc\n");
        let caret = |cx: &mut gpui::VisualTestContext| {
            editor.read_with(cx, |state, _| state.prepaint_caret_geometry().unwrap().cursor_bounds())
        };
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.focus(window, cx);
                state.set_selected_range(1..1, cx);
            })
        });
        cx.run_until_parked();
        let bar = caret(&mut cx);
        let (b, c) = editor.read_with(&cx, |state, _| {
            (state.range_to_bounds(&(1..1)).unwrap().origin.x, state.range_to_bounds(&(2..2)).unwrap().origin.x)
        });
        for shape in [CursorShape::Block, CursorShape::Underline, CursorShape::Hollow] {
            cx.update(|_, cx| editor.update(cx, |state, cx| state.set_cursor_shape(shape, cx)));
            cx.run_until_parked();
            let bounds = caret(&mut cx);
            assert_eq!(bounds.origin.x, bar.origin.x, "{shape:?}");
            assert!((bounds.size.width - (c - b)).abs() < gpui::px(0.5), "{shape:?} covers the character");
            let line_height = editor.read_with(&cx, |state, _| state.line_height().unwrap());
            let height = if shape == CursorShape::Underline { gpui::px(2.) } else { line_height };
            assert_eq!(bounds.size.height, height, "{shape:?}");
        }
        assert!(bar.size.width < gpui::px(3.));
    }
}

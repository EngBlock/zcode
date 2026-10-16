//! A window holding one styled editor, for the extension points' tests.

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px, size,
};

use crate::EditorState;

pub(crate) struct EditorWindow(pub(crate) Entity<EditorState>);

impl Render for EditorWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(crate::Editor::new(&self.0))
    }
}

/// Opens an 800×600 window with an editor on `text`, laid out once.
pub(crate) fn open_editor(
    cx: &mut TestAppContext,
    text: &str,
) -> (Entity<EditorState>, VisualTestContext) {
    let text = text.to_string();
    let mut editor = None;
    let window = cx.open_window(size(px(800.), px(600.)), |window, cx| {
        cx.set_global(gpui_base::Theme::default());
        crate::test_init(cx);
        let state = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(true)
                .default_value(text)
        });
        editor = Some(state.clone());
        EditorWindow(state)
    });
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    (editor.unwrap(), cx)
}

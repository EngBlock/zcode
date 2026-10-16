use gpui::{
    App, AppContext as _, Context, Entity, FontWeight, HighlightStyle, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, StyledText, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::{
    EditorState, split_signature_label,
    ui::popovers::{Popover, render_markdown},
};

/// The signature of the call the caret is in, above the caret, with the
/// argument being typed in bold; the signature's (or the argument's)
/// documentation below it when the server has some.
pub struct SignatureHelpPopover {
    editor: Entity<EditorState>,
    help: lsp_types::SignatureHelp,
}

impl SignatureHelpPopover {
    pub fn new(editor: Entity<EditorState>, help: lsp_types::SignatureHelp, cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self { editor, help })
    }
}

fn documentation_text(documentation: &lsp_types::Documentation) -> String {
    match documentation {
        lsp_types::Documentation::String(text) => text.clone(),
        lsp_types::Documentation::MarkupContent(markup) => markup.value.clone(),
    }
}

impl Render for SignatureHelpPopover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.help.signatures.len();
        let index = (self.help.active_signature.unwrap_or(0) as usize).min(count.saturating_sub(1));
        let signature = self.help.signatures[index].clone();
        let active = signature.active_parameter.or(self.help.active_parameter);
        let (before, parameter, after) = split_signature_label(&signature, active);
        let label: SharedString = format!("{before}{parameter}{after}").into();
        let emphasis = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            color: Some(cx.theme().primary),
            ..Default::default()
        };
        let highlights = if parameter.is_empty() { vec![] } else { vec![(before.len()..before.len() + parameter.len(), emphasis)] };
        let documentation = active
            .and_then(|ix| signature.parameters.as_ref()?.get(ix as usize)?.documentation.as_ref())
            .or(signature.documentation.as_ref())
            .map(documentation_text)
            .filter(|text| !text.trim().is_empty());
        let mono = cx.theme().mono_font_family.clone();
        let muted = cx.theme().muted_foreground;
        let cursor = self.editor.read(cx).cursor();

        Popover::new("signature-help", self.editor.clone(), cursor..cursor, move |window, cx| {
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().font_family(mono.clone()).child(StyledText::new(label.clone()).with_highlights(highlights.clone())))
                        .when_some((count > 1).then(|| format!("{}/{count}", index + 1)), |el, counter| {
                            el.child(div().text_color(muted).child(counter))
                        }),
                )
                .when_some(documentation.clone(), |el, text| el.child(render_markdown("signature-doc", text, window, cx)))
        })
        .sticky()
    }
}

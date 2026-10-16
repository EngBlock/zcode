mod caret_anchored;
mod code_action_menu;
mod completion_menu;
mod diagnostic_popover;
mod hover_popover;
mod signature_help_popover;

pub(crate) use caret_anchored::*;
pub(crate) use code_action_menu::*;
pub(crate) use completion_menu::*;
pub(crate) use diagnostic_popover::*;
pub(crate) use hover_popover::*;
pub(crate) use signature_help_popover::*;

use gpui::{
    App, Div, ElementId, InteractiveElement as _, SharedString, Stateful, StyleRefinement,
    Styled as _, Window, div, px, rems,
};

use gpui::{Context, Hsla};
use gpui_component::{
    ActiveTheme, IndexPath, ThemeStyled as _,
    list::{ListDelegate, ListEvent, ListState},
    text::{TextView, TextViewStyle},
};

use crate::DiagnosticSeverity;

pub(super) fn render_markdown(
    id: impl Into<ElementId>,
    markdown: impl Into<SharedString>,
    _: &mut Window,
    cx: &mut App,
) -> TextView {
    TextView::markdown(id, markdown)
        .style(
            TextViewStyle::default()
                .paragraph_gap(rems(0.5))
                .heading_font_size(|level, rem_size| match level {
                    1..=3 => rem_size * 1,
                    4 => rem_size * 0.9,
                    _ => rem_size * 0.8,
                })
                .code_block(
                    StyleRefinement::default()
                        .bg(cx.theme().transparent)
                        .p_0()
                        .text_size(px(11.)),
                ),
        )
        // zcode patch: the code block's language label sits in its top-right
        // corner, relying on top padding the popover style removes, so it
        // overlapped the code. In an editor popover the language is the
        // file's own, so the label is dropped: providing (empty) actions
        // takes the corner instead of the label.
        .code_block_actions(|_, _, _| gpui::Empty)
        .selectable(true)
}

/// Moves a one-section list's selection by `delta`, wrapping around, as the
/// list's own Select Up and Select Down do (those handlers are private to
/// gpui-component).
pub(super) fn step_list_selection<D: ListDelegate>(
    list: &mut ListState<D>,
    delta: isize,
    window: &mut Window,
    cx: &mut Context<ListState<D>>,
) {
    let count = list.delegate().items_count(0, cx);
    if count == 0 {
        return;
    }
    let row = match list.selected_index() {
        Some(ix) => (ix.row as isize + delta).rem_euclid(count as isize) as usize,
        None if delta < 0 => count - 1,
        None => 0,
    };
    let ix = IndexPath::new(row);
    list.set_selected_index(Some(ix), window, cx);
    list.scroll_to_selected_item(window, cx);
    cx.emit(ListEvent::Select(ix));
    cx.notify();
}

pub(super) fn editor_popover(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .occlude()
        .popover_style(cx)
        .shadow_md()
        .text_xs()
        .p_1()
}

// The diagnostic colors, from gpui-component's `highlighter::diagnostic_styles`.

pub(crate) fn diagnostic_background(severity: DiagnosticSeverity, cx: &App) -> Hsla {
    let status = &cx.theme().highlight_theme.style.status;
    match severity {
        DiagnosticSeverity::Error => status.error_background(cx),
        DiagnosticSeverity::Warning => status.warning_background(cx),
        DiagnosticSeverity::Info => status.info_background(cx),
        DiagnosticSeverity::Hint => status.hint_background(cx),
    }
}

pub(crate) fn diagnostic_foreground(severity: DiagnosticSeverity, cx: &App) -> Hsla {
    let status = &cx.theme().highlight_theme.style.status;
    match severity {
        DiagnosticSeverity::Error => status.error(cx),
        DiagnosticSeverity::Warning => status.warning(cx),
        DiagnosticSeverity::Info => status.info(cx),
        DiagnosticSeverity::Hint => status.hint(cx),
    }
}

pub(crate) fn diagnostic_border(severity: DiagnosticSeverity, cx: &App) -> Hsla {
    let status = &cx.theme().highlight_theme.style.status;
    match severity {
        DiagnosticSeverity::Error => status.error_border(cx),
        DiagnosticSeverity::Warning => status.warning_border(cx),
        DiagnosticSeverity::Info => status.info_border(cx),
        DiagnosticSeverity::Hint => status.hint_border(cx),
    }
}

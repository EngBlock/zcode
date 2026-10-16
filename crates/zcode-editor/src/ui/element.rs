//! The styled code editor: the engine's text element in a frame that takes
//! its colors from the gpui-component theme, with the find panel above it and
//! the completion, code action, hover and diagnostic popovers over it.
//!
//! Adapted from gpui-component's `Editor` and the code-editor path of its
//! `Input` element, which render every kind of text field. This one renders
//! only [`EditorState`], so the single-line machinery (prefix and suffix
//! slots, masks, the clear button, touch handles) is gone.

use std::rc::Rc;

use gpui::{
    App, ClipboardItem, Edges, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _, px,
    relative,
};
use gpui_component::button::{Button, ButtonRounded, ButtonVariants as _};
use gpui_component::native_menu::NativeMenu;
use gpui_component::{ActiveTheme as _, IconName, Selectable as _, Sizable as _, StyledExt as _, v_flex};

use crate::{
    Copy, Cut, DiagnosticColors, EditorState, GoToDefinition, InputEditorStyle, Paste, SelectAll,
    ToggleCodeActions,
};

/// The theme's editor background, or its input background (gpui-component's
/// private `Theme::editor_background`).
fn editor_background(cx: &App) -> gpui::Hsla {
    let theme = cx.theme();
    theme
        .highlight_theme
        .style
        .editor_background
        .unwrap_or_else(|| theme.input_background())
}

/// A code editor takes its rows from the font, so that a smaller or larger
/// font keeps its leading in proportion.
const EDITOR_LINE_HEIGHT: f32 = 1.5;

/// Padding around the text, as gpui-component's medium-size inputs have it.
const PADDING_X: f32 = 10.;
const PADDING_Y: f32 = 8.;
/// The line numbers sit closer to the left edge than text does elsewhere.
const PADDING_LEFT: f32 = 6.;

type ContextMenuBuilder = Rc<dyn Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu>;
type PasteHandler = Rc<dyn Fn(&ClipboardItem, &mut Window, &mut App) -> bool>;

/// A styled source-code editor for an [`EditorState`].
///
/// Read-only mode lives on the state ([`EditorState::set_read_only`]), so
/// every caller sees the same answer, not just this element.
#[derive(IntoElement)]
pub struct Editor {
    state: Entity<EditorState>,
    style: StyleRefinement,
    bordered: bool,
    context_menu_builder: Option<ContextMenuBuilder>,
    paste_handler: Option<PasteHandler>,
}

impl Editor {
    pub fn new(state: &Entity<EditorState>) -> Self {
        Self {
            state: state.clone(),
            style: StyleRefinement::default(),
            bordered: true,
            context_menu_builder: None,
            paste_handler: None,
        }
    }

    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// Replace the built-in context menu shown on right-click.
    ///
    /// The closure receives an empty menu and returns the one to show.
    pub fn context_menu(
        mut self,
        f: impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static,
    ) -> Self {
        self.context_menu_builder = Some(Rc::new(f));
        self
    }

    /// Intercept paste payloads (images, files) before the default text
    /// insertion: `true` consumes the paste.
    pub fn on_paste(
        mut self,
        handler: impl Fn(&ClipboardItem, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.paste_handler = Some(Rc::new(handler));
        self
    }

    /// The engine's colors from the gpui-component theme.
    fn editor_style(cx: &App) -> InputEditorStyle {
        let theme = cx.theme();
        let status = &theme.highlight_theme.style.status;
        InputEditorStyle {
            foreground: theme.foreground,
            muted_foreground: theme.muted_foreground,
            background: editor_background(cx),
            border: theme.border,
            selection: theme.selection,
            caret: theme.caret,
            diagnostics: DiagnosticColors {
                error: status.error(cx),
                warning: status.warning(cx),
                info: status.info(cx),
                hint: status.hint(cx),
            },
            highlight_styles: theme.highlight_theme.clone(),
            editor_invisible: theme.highlight_theme.style.editor_invisible,
            editor_active_line: theme.highlight_theme.style.editor_active_line,
            editor_gutter_background: theme.highlight_theme.style.editor_gutter_background,
            fold_icon_renderer: Some(Rc::new(|ix, is_folded| {
                Button::new(("fold-icon", ix))
                    .ghost()
                    .icon(if is_folded {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    })
                    .xsmall()
                    .rounded(ButtonRounded::Small)
                    .size(px(14.))
                    .selected(is_folded)
                    .into_any_element()
            })),
        }
    }

    /// Go to Definition and Show Code Actions, then Cut, Copy, Paste and
    /// Select All, each disabled when it can't apply.
    fn default_context_menu(capabilities: crate::InputContextMenuCapabilities) -> NativeMenu {
        let enabled = !capabilities.is_disabled();
        // A read-only editor can still navigate the code; it only rejects the
        // items that would change the text.
        let editable = enabled && !capabilities.is_readonly();
        NativeMenu::new()
            .menu_with_disabled(
                "Go to Definition",
                !(enabled && capabilities.has_definition()),
                Box::new(GoToDefinition),
            )
            .menu_with_disabled(
                "Show Code Actions",
                !(editable && capabilities.has_code_actions()),
                Box::new(ToggleCodeActions),
            )
            .separator()
            .menu_with_disabled(
                "Cut",
                !(editable && capabilities.is_copyable()),
                Box::new(Cut),
            )
            .menu_with_disabled("Copy", !capabilities.is_copyable(), Box::new(Copy))
            .menu_with_disabled("Paste", !editable, Box::new(Paste))
            .separator()
            .menu("Select All", Box::new(SelectAll))
    }
}

impl Styled for Editor {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Editor {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.state.clone();
        let style = Self::editor_style(cx);
        let custom_menu = self.context_menu_builder.clone();
        state.update(cx, |state, _| {
            state.ensure_highlighter_factory(super::highlighter::input_highlighter_factory());
            state.set_editor_style(style);
            state.set_editor_paddings(Edges {
                top: px(PADDING_Y),
                right: px(PADDING_X),
                bottom: px(PADDING_Y),
                left: px(PADDING_LEFT),
            });
            state.on_context_menu(Rc::new(move |_, capabilities, position, window, cx| {
                let menu = match custom_menu.as_ref() {
                    Some(custom) => custom(NativeMenu::new(), window, cx),
                    None => Self::default_context_menu(capabilities),
                };
                menu.show(position, window, cx);
            }));
        });

        // The engine ignores `Paste` on a read-only editor; the hook must not
        // see a paste the editor itself would refuse.
        let paste_handler = self
            .paste_handler
            .clone()
            .filter(|_| state.read(cx).is_editable());
        let overlays = super::overlay::render_overlays(&state, window, cx);
        let background = editor_background(cx);
        let border = cx.theme().input;
        let radius = cx.theme().radius;

        v_flex()
            .id(("code-editor", state.entity_id()))
            .relative()
            .size_full()
            .bg(background)
            .rounded(radius)
            .when(self.bordered, |this| this.border_1().border_color(border))
            // Source code wants a monospace font at a code size, and rows
            // that follow that size. A text style set on this editor refines
            // over them, so `.text_size()` and `.font_family()` keep working.
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(cx.theme().mono_font_size)
            .line_height(relative(EDITOR_LINE_HEIGHT))
            .refine_style(&self.style)
            .children(overlays.search)
            .child(div().relative().flex_1().min_h_0().child(state.clone()))
            .children(overlays.floating)
            .when_some(paste_handler, |this, handler| {
                this.capture_action(move |_: &Paste, window, cx| {
                    if let Some(clipboard) = cx.read_from_clipboard()
                        && handler(&clipboard, window, cx)
                    {
                        cx.stop_propagation();
                    }
                })
            })
    }
}

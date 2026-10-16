//! A keyboard-driven modal list (Open Quickly, Quick Actions, Document
//! Items, Jump to Line, scheme/destination choosers).

use std::ops::Range;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::assets::IconName;

use crate::actions::{PickerConfirm, PickerConfirmAlternate, PickerDismiss, PickerSelectNext, PickerSelectPrevious};

#[derive(Clone, Default)]
pub struct PickerItem {
    pub label: SharedString,
    /// Dimmed secondary text (path, container, runtime...).
    pub detail: Option<SharedString>,
    pub icon: Option<IconName>,
    /// Character positions in `label` that matched the query.
    pub positions: Vec<usize>,
    /// Right-aligned hint (keystroke, symbol kind...).
    pub hint: Option<SharedString>,
    /// Indentation level (document outline).
    pub depth: usize,
    pub checked: bool,
}

pub trait PickerDelegate: Sized + 'static {
    fn placeholder(&self) -> SharedString;
    fn update_matches(&mut self, query: String, window: &mut Window, cx: &mut Context<Picker<Self>>);
    fn items(&self) -> &[PickerItem];
    /// Return `true` to close the picker.
    fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool;
    /// ⌥-Return or ⌥-click (Open Quickly: open in the next pane); `confirm`
    /// unless the delegate has an alternative.
    fn confirm_alternate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        self.confirm(index, window, cx)
    }
    fn two_line(&self) -> bool {
        false
    }
    fn empty_message(&self) -> SharedString {
        "No Matches".into()
    }
    fn initial_selection(&self) -> usize {
        0
    }
}

pub struct Picker<D: PickerDelegate> {
    pub delegate: D,
    query: Entity<InputState>,
    selected: usize,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl<D: PickerDelegate> EventEmitter<DismissEvent> for Picker<D> {}

impl<D: PickerDelegate> Focusable for Picker<D> {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.query.focus_handle(cx)
    }
}

impl<D: PickerDelegate> Picker<D> {
    pub fn new(delegate: D, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_query(delegate, "", window, cx)
    }

    pub fn with_query(
        delegate: D,
        initial_query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = delegate.placeholder();
        let initial_query = initial_query.to_string();
        let query = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(initial_query.clone());
            // A pre-filled query is selected so typing replaces it.
            if !initial_query.is_empty() {
                state.select_all(window, cx);
            }
            state
        });
        let subscription = cx.subscribe_in(&query, window, |this, input, event, window, cx| {
            if let InputEvent::Change = event {
                let text = input.read(cx).value().to_string();
                this.delegate.update_matches(text, window, cx);
                this.selected = 0;
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            }
        });
        let mut this = Self {
            delegate,
            query,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: vec![subscription],
        };
        this.delegate.update_matches(initial_query, window, cx);
        this.selected = this.delegate.initial_selection();
        this.scroll
            .scroll_to_item(this.selected, ScrollStrategy::Center);
        this
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.query.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// Re-runs matching for the current query (after the delegate's data
    /// changed) and restores the delegate's preferred selection.
    pub fn refresh_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        let filtering = !query.is_empty();
        self.delegate.update_matches(query, window, cx);
        self.selected = if filtering { 0 } else { self.delegate.initial_selection() };
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Center);
        cx.notify();
    }

    /// Call after the delegate's matches change asynchronously.
    pub fn matches_updated(&mut self, cx: &mut Context<Self>) {
        let count = self.delegate.items().len();
        if self.selected >= count {
            self.selected = count.saturating_sub(1);
        }
        cx.notify();
    }

    fn select_next(&mut self, _: &PickerSelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.delegate.items().len();
        if count > 0 {
            self.selected = (self.selected + 1) % count;
            self.scroll.scroll_to_item(self.selected, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn select_previous(&mut self, _: &PickerSelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.delegate.items().len();
        if count > 0 {
            self.selected = (self.selected + count - 1) % count;
            self.scroll.scroll_to_item(self.selected, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn confirm_selected(&mut self, _: &PickerConfirm, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_index(self.selected, window, cx);
    }

    fn confirm_selected_alternate(&mut self, _: &PickerConfirmAlternate, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_index_with(self.selected, true, window, cx);
    }

    fn confirm_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_index_with(index, false, window, cx);
    }

    fn confirm_index_with(&mut self, index: usize, alternate: bool, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.delegate.items().len() {
            return;
        }
        let close = if alternate {
            self.delegate.confirm_alternate(index, window, cx)
        } else {
            self.delegate.confirm(index, window, cx)
        };
        if close {
            cx.emit(DismissEvent);
        }
    }

    fn dismiss(&mut self, _: &PickerDismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn render_items(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let two_line = self.delegate.two_line();
        range
            .filter_map(|ix| {
                let item = self.delegate.items().get(ix)?.clone();
                let selected = ix == self.selected;
                let label = highlighted(&item.label, &item.positions, theme.primary);
                Some(
                    h_flex()
                        .id(("picker-item", ix))
                        .w_full()
                        .h(px(if two_line { 42. } else { 28. }))
                        .px_3()
                        .pl(px(12. + item.depth as f32 * 14.))
                        .gap_2()
                        .rounded(theme.radius)
                        .when(selected, |el| el.bg(theme.accent))
                        .hover(|el| el.bg(theme.accent.opacity(0.6)))
                        .when_some(item.icon, |el, icon| {
                            el.child(Icon::new(icon).small().text_color(theme.muted_foreground))
                        })
                        .child(
                            v_flex()
                                .flex_1()
                                .overflow_hidden()
                                .child(div().text_sm().truncate().child(label))
                                .when(two_line, |el| {
                                    el.when_some(item.detail.clone(), |el, detail| {
                                        el.child(
                                            div()
                                                .text_xs()
                                                .truncate()
                                                .text_color(theme.muted_foreground)
                                                .child(detail),
                                        )
                                    })
                                }),
                        )
                        .when(!two_line, |el| {
                            el.when_some(item.detail.clone(), |el, detail| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .truncate()
                                        .max_w(px(260.))
                                        .text_color(theme.muted_foreground)
                                        .child(detail),
                                )
                            })
                        })
                        .when(item.checked, |el| {
                            el.child(Icon::new(IconName::Check).small().text_color(theme.primary))
                        })
                        .when_some(item.hint.clone(), |el, hint| {
                            el.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(hint),
                            )
                        })
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            this.selected = ix;
                            this.confirm_index_with(ix, event.modifiers().alt, window, cx);
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }
}

fn highlighted(label: &SharedString, positions: &[usize], color: Hsla) -> StyledText {
    if positions.is_empty() {
        return StyledText::new(label.clone());
    }
    let mut highlights = Vec::new();
    for (char_ix, (byte_ix, ch)) in label.char_indices().enumerate() {
        if positions.binary_search(&char_ix).is_ok() {
            highlights.push((
                byte_ix..byte_ix + ch.len_utf8(),
                HighlightStyle {
                    color: Some(color),
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
            ));
        }
    }
    StyledText::new(label.clone()).with_highlights(highlights)
}

impl<D: PickerDelegate> Render for Picker<D> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = self.delegate.items().len();
        let row = if self.delegate.two_line() { 42. } else { 28. };
        let list_height = (count.min(12) as f32 * row).max(row) + 8.;
        v_flex()
            .key_context("Picker")
            .w(px(640.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .overflow_hidden()
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm_selected))
            .on_action(cx.listener(Self::confirm_selected_alternate))
            .on_action(cx.listener(Self::dismiss))
            .child(
                h_flex().p_2().child(
                    Input::new(&self.query)
                        .appearance(false)
                        .prefix(Icon::new(IconName::Search).small().text_color(theme.muted_foreground)),
                ),
            )
            .child(div().h(px(1.)).bg(theme.border))
            .map(|el| {
                if count == 0 {
                    el.child(
                        div()
                            .p_3()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(self.delegate.empty_message()),
                    )
                } else {
                    el.child(
                        uniform_list("picker-list", count, cx.processor(Self::render_items))
                            .track_scroll(&self.scroll)
                            .p_1()
                            .h(px(list_height)),
                    )
                }
            })
    }
}

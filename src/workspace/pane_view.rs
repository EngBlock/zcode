//! The editor area on screen: the split tree of panes, and each pane's tab
//! bar, jump bar and shown item.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::resizable::{ResizableState, h_resizable, resizable_panel, v_resizable};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zcode_editor::Editor;

use super::pane::{AssistantMode, EditorItem, Item, PaneId};
use super::pane_group::{Member, SplitAxis};
use super::pickers;
use super::Workspace;
use crate::actions::*;

impl Workspace {
    pub(super) fn render_editor_area(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.note_shown_documents(cx);
        if self.zoomed || self.panes.len() == 1 {
            return self.render_pane(self.active_pane, cx);
        }
        let root = self.pane_group.root.clone();
        // Sizes of splits that are gone go with them.
        let mut keys = vec![];
        root.split_keys(&mut keys);
        self.split_states.retain(|key, _| keys.iter().any(|k| k == key.as_ref()));
        self.render_member(&root, cx)
    }

    /// Tells the language servers which files the panes show, the focused
    /// pane's first, when that changed: sourcekit-lsp keeps only those and
    /// the few shown last open (`LspStore::set_shown`). Here rather than
    /// where tabs change, since whatever is drawn is what's shown.
    fn note_shown_documents(&mut self, cx: &mut Context<Self>) {
        let focused = self.pane(self.active_pane).into_iter();
        let others = self.panes.iter().filter(|p| !self.zoomed && p.id != self.active_pane);
        let shown: Vec<PathBuf> = focused
            .chain(others)
            .filter_map(|pane| match pane.active_item()? {
                Item::Editor(item) => Some(item.path.clone()),
                Item::Diff(item) => Some(item.path.clone()),
                _ => None,
            })
            .collect();
        if shown != self.lsp_shown {
            self.lsp_shown = shown.clone();
            let lsp = self.lsp.clone();
            cx.defer(move |cx| lsp.update(cx, |lsp, cx| lsp.set_shown(shown, cx)));
        }
    }

    fn render_member(&mut self, member: &Member, cx: &mut Context<Self>) -> AnyElement {
        let (axis, members) = match member {
            Member::Pane(id) => return self.render_pane(*id, cx),
            Member::Split { axis, members } => (*axis, members),
        };
        let key = SharedString::from(Member::split_key(members));
        let state = self.split_states.entry(key.clone()).or_insert_with(|| cx.new(|_| ResizableState::default())).clone();
        let mut group = match axis {
            SplitAxis::Horizontal => h_resizable(key.clone()),
            SplitAxis::Vertical => v_resizable(key.clone()),
        }
        .with_state(&state);
        // Sizes from the session, for every pane but the last (which takes
        // what's left, whatever the window's size now).
        let restored = self.restored_split_sizes.get(&key).cloned().unwrap_or_default();
        for (ix, member) in members.iter().enumerate() {
            let panel = resizable_panel().when_some(restored.get(ix).filter(|_| ix + 1 < members.len()), |panel, size| panel.size(px(*size)));
            group = group.child(panel.child(self.render_member(member, cx)));
        }
        group.into_any_element()
    }

    fn render_pane(&mut self, id: PaneId, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(pane) = self.pane(id) else { return div().into_any_element() };
        let content = match pane.active_item() {
            None => {
                let message = match pane.assistant.map(|a| a.mode) {
                    Some(AssistantMode::Counterparts) => "No Counterparts",
                    _ => "No Editor",
                };
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .bg(theme.background)
                    .text_color(theme.muted_foreground)
                    .child(div().text_lg().child(message))
                    .when(self.panes.len() == 1, |el| {
                        el.child(hint("Open Quickly", "⇧⌘O"))
                            .child(hint("Find in Workspace", "⇧⌘F"))
                            .child(hint("Quick Actions", "⇧⌘A"))
                            .child(hint("Build / Run", "⌘B / ⌘R"))
                    })
                    .into_any_element()
            }
            Some(Item::ProjectSettings(settings)) => div().flex_1().min_h_0().child(settings.clone()).into_any_element(),
            Some(Item::Organizer(organizer)) => div().flex_1().min_h_0().child(organizer.clone()).into_any_element(),
            Some(Item::AssetCatalog(item)) => div().flex_1().min_h_0().child(item.view.clone()).into_any_element(),
            Some(Item::Image(item)) => div().flex_1().min_h_0().child(item.view.clone()).into_any_element(),
            Some(Item::Diff(diff)) => {
                diff.view.update(cx, |view, cx| view.set_font_size(self.font_size, cx));
                div().flex_1().min_h_0().child(diff.view.clone()).into_any_element()
            }
            Some(Item::MultiBuffer(item)) => {
                item.view.update(cx, |view, cx| view.set_font_size(self.font_size, cx));
                div().flex_1().min_h_0().child(item.view.clone()).into_any_element()
            }
            Some(Item::Terminal(item)) => {
                item.view.update(cx, |view, cx| view.set_font_size(self.font_size, cx));
                div().flex_1().min_h_0().child(item.view.clone()).into_any_element()
            }
            Some(Item::Editor(item)) => {
                let body = match item.shown_plist() {
                    Some(plist) => div().flex_1().min_h_0().child(plist.clone()).into_any_element(),
                    None => div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(
                            Editor::new(&item.editor)
                                .bordered(false)
                                .p_0()
                                .h_full()
                                .font_family(theme.mono_font_family.clone())
                                .text_size(px(self.font_size)),
                        )
                        .into_any_element(),
                };
                v_flex().flex_1().min_h_0().child(self.render_jump_bar(id, item, cx)).child(body).into_any_element()
            }
        };
        let show_tabs = !(self.panes.len() == 1 && pane.items.is_empty());
        v_flex()
            .id(("pane", id.0))
            .size_full()
            // A click anywhere in a pane focuses it, before its content
            // takes the click.
            .capture_any_mouse_down(cx.listener(move |this, _, window, cx| this.pane_clicked(id, window, cx)))
            .when(show_tabs, |el| el.child(self.render_tabs(id, cx)))
            .child(content)
            .into_any_element()
    }

    fn render_tabs(&mut self, id: PaneId, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Some(pane) = self.pane(id) else { return div().into_any_element() };
        let focused = id == self.active_pane;
        let split = self.panes.len() > 1;
        let tabs = pane.items.iter().enumerate().map(|(ix, item)| {
            let active = ix == pane.active;
            let (key, icon, title, read_only, dirty) = match item {
                Item::Editor(item) => {
                    let buffer = self.buffer(&item.path);
                    (
                        SharedString::from(format!("tab-{}", item.path.display())),
                        Icon::new(pickers::file_icon(&item.path)).small().text_color(theme.muted_foreground),
                        item.title(),
                        buffer.is_some_and(|b| b.read_only),
                        buffer.is_some_and(|b| b.dirty),
                    )
                }
                Item::Diff(diff) => {
                    let buffer = self.buffer(&diff.path);
                    (
                        SharedString::from(format!("tab-diff-{}", diff.path.display())),
                        Icon::new(IconName::GitCompare).small().text_color(theme.muted_foreground),
                        diff.title(),
                        buffer.is_some_and(|b| b.read_only),
                        buffer.is_some_and(|b| b.dirty),
                    )
                }
                Item::MultiBuffer(item) => {
                    // Dirty while a file it changed isn't saved.
                    let dirty = item.view.read(cx).paths.values().any(|path| self.buffer(path).is_some_and(|b| b.dirty));
                    (
                        SharedString::from(format!("tab-multibuffer-{}", item.view.entity_id())),
                        Icon::new(item.icon(cx)).small().text_color(theme.muted_foreground),
                        item.title.clone(),
                        false,
                        dirty,
                    )
                }
                Item::Terminal(item) => (
                    SharedString::from(format!("tab-terminal-{}", item.view.entity_id())),
                    Icon::new(IconName::SquareTerminal).small().text_color(theme.muted_foreground),
                    item.view.read(cx).title(),
                    false,
                    false,
                ),
                Item::ProjectSettings(_) => (
                    SharedString::from("tab-project-settings"),
                    Icon::new(IconName::FolderCode).small().text_color(theme.primary),
                    self.project.name.clone(),
                    false,
                    false,
                ),
                Item::Organizer(_) => (
                    SharedString::from("tab-organizer"),
                    Icon::new(IconName::Package).small().text_color(theme.primary),
                    "Organizer".to_string(),
                    false,
                    false,
                ),
                Item::AssetCatalog(item) => (
                    SharedString::from(format!("tab-assets-{}", item.view.entity_id())),
                    Icon::new(IconName::Images).small().text_color(theme.primary),
                    super::pane::file_title(&item.path),
                    false,
                    false,
                ),
                Item::Image(item) => (
                    SharedString::from(format!("tab-image-{}", item.view.entity_id())),
                    Icon::new(IconName::FileImage).small().text_color(theme.muted_foreground),
                    super::pane::file_title(&item.path),
                    false,
                    false,
                ),
            };
            h_flex()
                .id(key)
                .group("tab")
                .h_full()
                .px_3()
                .gap_1p5()
                .min_w(px(110.))
                .max_w(px(220.))
                .text_sm()
                .border_r_1()
                .border_color(theme.border)
                .when(active, |el| {
                    el.bg(theme.tab_active).text_color(if focused || !split { theme.tab_active_foreground } else { theme.muted_foreground })
                })
                // The focused pane's tab, when there are several panes.
                .when(active && focused && split, |el| el.border_t_2().border_color(theme.primary))
                .when(!active, |el| el.text_color(theme.tab_foreground).hover(|el| el.bg(theme.secondary_hover)))
                .child(icon)
                .child(div().flex_1().truncate().child(title))
                .when(read_only, |el| el.child(Icon::new(IconName::Lock).xsmall().text_color(theme.muted_foreground)))
                .child(
                    div()
                        .id(("close-tab", ix))
                        .size(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(theme.radius)
                        .hover(|el| el.bg(theme.muted))
                        .map(|el| {
                            if dirty {
                                el.child(div().size(px(7.)).rounded_full().bg(theme.foreground.opacity(0.6)))
                            } else {
                                el.invisible().group_hover("tab", |s| s.visible()).child(Icon::new(IconName::X).xsmall())
                            }
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_item(id, ix, window, cx);
                        })),
                )
                .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.close_item(id, ix, window, cx)))
                .on_click(cx.listener(move |this, _, window, cx| this.activate_item(id, ix, window, cx)))
                .into_any_element()
        });
        let tabs: Vec<AnyElement> = tabs.collect();
        let assistant = pane.assistant.map(|a| a.mode);
        let button = |key: &'static str, icon: IconName, tooltip: &'static str| {
            div()
                .id(key)
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme.radius)
                .text_color(theme.muted_foreground)
                .hover(|el| el.bg(theme.secondary_hover))
                .child(Icon::new(icon).xsmall())
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tooltip).build(window, cx))
        };
        h_flex()
            .id(("tabs", id.0))
            .h(px(32.))
            .w_full()
            .bg(theme.tab_bar)
            .border_b_1()
            .border_color(theme.border)
            .child(h_flex().id(("tab-strip", id.0)).h_full().flex_1().min_w_0().overflow_x_scroll().children(tabs))
            .child(
                h_flex()
                    .flex_none()
                    .px_1()
                    .gap_0p5()
                    // Xcode names the assistant's mode in its jump bar; a click
                    // switches between Counterparts and Manual.
                    .when_some(assistant, |el, mode| {
                        el.child(
                            div()
                                .id(("assistant-mode", id.0))
                                .px_1p5()
                                .text_xs()
                                .rounded(theme.radius)
                                .text_color(theme.muted_foreground)
                                .hover(|el| el.bg(theme.secondary_hover))
                                .child(match mode {
                                    AssistantMode::Counterparts => "Counterparts",
                                    AssistantMode::Manual => "Manual",
                                })
                                .on_click(cx.listener(move |this, _, window, cx| this.toggle_assistant_mode(id, window, cx))),
                        )
                    })
                    .child(button("split-right", IconName::PanelRight, "Add Editor Pane on Right (⌃⌘T)").on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.pane_clicked(id, window, cx);
                            this.split_pane(SplitAxis::Horizontal, window, cx);
                        },
                    )))
                    .when(split, |el| {
                        el.child(button("close-pane", IconName::X, "Close Editor Pane (⌃⇧⌘W)").on_click(cx.listener(
                            move |this, _, window, cx| this.close_pane(id, window, cx),
                        )))
                    }),
            )
            .into_any_element()
    }

    fn render_jump_bar(&self, id: PaneId, item: &EditorItem, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rel = item.path.strip_prefix(&self.project.root).unwrap_or(&item.path).to_path_buf();
        // The symbol at the cursor, from the outline of the focused file.
        let symbol = (id == self.active_pane && self.outline_path.as_ref() == Some(&item.path))
            .then(|| {
                let cursor_line = item.editor.read(cx).cursor_position().line;
                self.outline.iter().rfind(|e| e.selection_range.start.line <= cursor_line).map(|e| e.name.clone())
            })
            .flatten();
        // A package's sources show under the package, as in Xcode.
        let mut crumbs = vec![];
        match self.package_path(&item.path) {
            Some((name, rel)) => {
                crumbs.push(name);
                crumbs.extend(rel.iter().map(|c| c.to_string_lossy().to_string()));
            }
            None => {
                crumbs.push(self.project.name.clone());
                crumbs.extend(rel.iter().map(|c| c.to_string_lossy().to_string()));
            }
        }
        let count = crumbs.len();
        h_flex()
            .h(px(24.))
            .px_2()
            .gap_1()
            .text_xs()
            .text_color(theme.muted_foreground)
            .border_b_1()
            .border_color(theme.border)
            .children(crumbs.into_iter().enumerate().flat_map(move |(ix, crumb)| {
                let mut parts = vec![];
                if ix > 0 {
                    parts.push(Icon::new(IconName::ChevronRight).xsmall().into_any_element());
                }
                parts.push(div().when(ix + 1 == count, |el| el.text_color(theme.foreground)).child(crumb).into_any_element());
                parts
            }))
            .when_some(symbol, |el, symbol| {
                el.child(Icon::new(IconName::ChevronRight).xsmall()).child(
                    div()
                        .id("jump-symbol")
                        .text_color(cx.theme().foreground)
                        .child(symbol)
                        .on_click(cx.listener(|this, _, window, cx| this.show_document_items(&ShowDocumentItems, window, cx))),
                )
            })
    }

    /// The assistant's mode, from its tab bar: Counterparts follows the file
    /// it assists again.
    fn toggle_assistant_mode(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(assistant) = self.pane_mut(pane).and_then(|p| p.assistant.as_mut()) else { return };
        assistant.mode = match assistant.mode {
            AssistantMode::Counterparts => AssistantMode::Manual,
            AssistantMode::Manual => AssistantMode::Counterparts,
        };
        let of = assistant.of;
        self.follow_counterparts(of, window, cx);
        cx.notify();
    }
}

fn hint(label: &'static str, keys: &'static str) -> impl IntoElement {
    h_flex()
        .gap_3()
        .text_sm()
        .child(div().w(px(160.)).text_right().child(label))
        .child(div().w(px(120.)).child(keys))
}

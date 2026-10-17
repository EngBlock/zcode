//! The integrated terminal in the workspace: terminals in the debug area's
//! Terminal tab, beside the console (like Zed's terminal panel), and as
//! tabs in editor panes. View ▸ Activate Terminal (⌃⌘`) shows the debug
//! area's, File ▸ New ▸ Terminal Tab opens one in the focused pane, and
//! Xcode's File ▸ Open Terminal at Location (also in the navigator's
//! context menu) opens one in the selected item's folder. ⌘-clicked paths
//! open in the editor. The session keeps each terminal as the folder it was
//! in; a restored one is a fresh shell there, started when first shown.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::pane::{Item, TerminalItem};
use super::{DebugTab, Target, Workspace};
use crate::session::SavedTerminal;
use crate::terminal::{Spawn, TerminalEvent, TerminalView};

pub(super) struct PanelTerminal {
    pub view: Entity<TerminalView>,
    _subscriptions: Vec<Subscription>,
}

/// The debug area's terminals and the one shown.
#[derive(Default)]
pub(super) struct TerminalsState {
    pub panel: Vec<PanelTerminal>,
    pub active: usize,
}

impl Workspace {
    fn new_terminal_view(&mut self, spawn: Spawn, window: &mut Window, cx: &mut Context<Self>) -> (Entity<TerminalView>, Vec<Subscription>) {
        let font_size = self.font_size;
        let view = cx.new(|cx| TerminalView::new(spawn, font_size, cx));
        let id = view.entity_id();
        let focus = view.read(cx).focus_handle(cx);
        let subscriptions = vec![
            cx.subscribe_in(&view, window, Self::on_terminal_event),
            // A terminal in a pane makes its pane the focused one.
            cx.on_focus_in(&focus, window, move |this, window, cx| this.view_focused(id, window, cx)),
        ];
        (view, subscriptions)
    }

    fn on_terminal_event(&mut self, view: &Entity<TerminalView>, event: &TerminalEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TerminalEvent::TitleChanged => cx.notify(),
            TerminalEvent::OpenFile { path, line, column } => {
                let target = line.map(|line| Target::Line { line: line.saturating_sub(1), column: column.unwrap_or(1).saturating_sub(1) });
                self.open_file(path.clone(), target, true, window, cx);
            }
            TerminalEvent::Exited { close: true } => self.close_terminal(view.entity_id(), window, cx),
            TerminalEvent::Exited { close: false } => cx.notify(),
        }
    }

    /// Where Open Terminal at Location opens: the folder picked in the
    /// navigator, or the focused file's, or the project's.
    fn terminal_location(&self) -> PathBuf {
        let path = self.inspected.clone().or_else(|| self.active_file_path()).unwrap_or_else(|| self.project.root.clone());
        folder_of(&path)
    }

    /// A new terminal in the debug area, shown and focused.
    pub fn new_panel_terminal(&mut self, spawn: Spawn, window: &mut Window, cx: &mut Context<Self>) {
        let (view, subscriptions) = self.new_terminal_view(spawn, window, cx);
        self.terminals.panel.push(PanelTerminal { view, _subscriptions: subscriptions });
        self.terminals.active = self.terminals.panel.len() - 1;
        self.show_panel_terminal(window, cx);
    }

    /// Shows the debug area's Terminal tab and focuses its terminal.
    fn show_panel_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_debug_area = true;
        self.debug_tab = DebugTab::Terminal;
        if let Some(terminal) = self.terminals.panel.get(self.terminals.active) {
            terminal.view.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// View ▸ Activate Terminal (⌃⌘`): the debug area's terminal, a new one
    /// in the project's folder when there's none.
    pub(super) fn activate_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminals.panel.is_empty() {
            self.new_panel_terminal(Spawn::shell(self.project.root.clone()), window, cx);
        } else {
            self.show_panel_terminal(window, cx);
        }
    }

    /// File ▸ New ▸ Terminal Tab: a terminal in the focused pane.
    pub fn new_terminal_tab(&mut self, spawn: Spawn, window: &mut Window, cx: &mut Context<Self>) {
        let (view, subscriptions) = self.new_terminal_view(spawn, window, cx);
        let pane = self.active_pane;
        let Some(p) = self.pane_mut(pane) else { return };
        p.items.push(Item::Terminal(TerminalItem { view, _subscriptions: subscriptions }));
        let ix = p.items.len() - 1;
        self.activate_item(pane, ix, window, cx);
    }

    /// File ▸ Open Terminal at Location: a terminal in `path`'s folder (the
    /// selected item's when `None`), in the debug area.
    pub fn open_terminal_at(&mut self, path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let dir = path.map(|path| folder_of(&path)).unwrap_or_else(|| self.terminal_location());
        self.new_panel_terminal(Spawn::shell(dir), window, cx);
    }

    /// The debug area's terminal that has focus.
    pub(super) fn focused_panel_terminal(&self, window: &Window, cx: &App) -> Option<usize> {
        self.terminals.panel.iter().position(|t| t.view.read(cx).focus_handle(cx).contains_focused(window, cx))
    }

    /// Closes a terminal, in the debug area or a pane, ending its processes.
    pub(super) fn close_terminal(&mut self, view: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.terminals.panel.iter().position(|t| t.view.entity_id() == view) {
            let closed = self.terminals.panel.remove(ix);
            let had_focus = closed.view.read(cx).focus_handle(cx).contains_focused(window, cx);
            closed.view.update(cx, |view, _| view.stop());
            self.terminals.active = match self.terminals.active {
                active if active > ix => active - 1,
                active => active.min(self.terminals.panel.len().saturating_sub(1)),
            };
            if had_focus {
                match self.terminals.panel.get(self.terminals.active) {
                    Some(next) => next.view.read(cx).focus_handle(cx).focus(window, cx),
                    None => self.focus_active(window, cx),
                }
            }
            cx.notify();
        } else if let Some((pane, ix)) = self.find_view(view) {
            self.force_close_item(pane, ix, window, cx);
        }
    }

    /// Ends every terminal's processes (the window is closing).
    pub fn stop_terminals(&mut self, cx: &mut Context<Self>) {
        let views: Vec<Entity<TerminalView>> = self
            .terminals
            .panel
            .iter()
            .map(|t| t.view.clone())
            .chain(self.panes.iter().flat_map(|p| p.items.iter()).filter_map(|item| match item {
                Item::Terminal(item) => Some(item.view.clone()),
                _ => None,
            }))
            .collect();
        for view in views {
            view.update(cx, |view, _| view.stop());
        }
    }

    // ---------------------------------------------------------------------
    // The debug area's Terminal tab

    /// Its terminals' tabs and + (in the debug area's header).
    pub(super) fn render_terminal_tabs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.terminals.active;
        h_flex()
            .id("terminal-tabs")
            .gap_0p5()
            .min_w_0()
            .overflow_x_scroll()
            .children(self.terminals.panel.iter().enumerate().map(|(ix, terminal)| {
                let id = terminal.view.entity_id();
                h_flex()
                    .id(("terminal-tab", ix))
                    .group("terminal-tab")
                    .h(px(20.))
                    .px_1p5()
                    .gap_1()
                    .max_w(px(180.))
                    .rounded(theme.radius)
                    .text_xs()
                    .when(ix == active, |el| el.bg(theme.secondary_active).text_color(theme.foreground))
                    .when(ix != active, |el| el.text_color(theme.muted_foreground).hover(|el| el.bg(theme.secondary_hover)))
                    .child(Icon::new(IconName::SquareTerminal).xsmall())
                    .child(div().truncate().child(terminal.view.read(cx).title()))
                    .child(
                        div()
                            .id(("close-terminal", ix))
                            .invisible()
                            .group_hover("terminal-tab", |s| s.visible())
                            .child(Icon::new(IconName::X).xsmall())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_terminal(id, window, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.terminals.active = ix;
                        this.show_panel_terminal(window, cx);
                    }))
            }))
            .child(
                Button::new("new-terminal")
                    .icon(IconName::Plus)
                    .xsmall()
                    .ghost()
                    .tooltip("New Terminal")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let dir = this.terminals.panel.get(this.terminals.active).map(|t| t.view.read(cx).current_dir());
                        this.new_panel_terminal(Spawn::shell(dir.unwrap_or_else(|| this.project.root.clone())), window, cx);
                    })),
            )
    }

    /// The shown terminal, or a way to start one.
    pub(super) fn render_terminal_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        match self.terminals.panel.get(self.terminals.active) {
            Some(terminal) => {
                let font_size = self.font_size;
                terminal.view.update(cx, |view, cx| view.set_font_size(font_size, cx));
                div().size_full().child(terminal.view.clone()).into_any_element()
            }
            None => v_flex()
                .p_3()
                .gap_2()
                .items_start()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("No terminal is open.")
                .child(
                    Button::new("start-terminal")
                        .label("New Terminal (⌃⌘`)")
                        .small()
                        .selected(false)
                        .on_click(cx.listener(|this, _, window, cx| this.activate_terminal(window, cx))),
                )
                .into_any_element(),
        }
    }

    // ---------------------------------------------------------------------
    // Session

    /// The debug area's terminals as the session keeps them: the folder
    /// each shell is in.
    pub(super) fn panel_terminal_dirs(&self, cx: &App) -> Vec<PathBuf> {
        self.terminals.panel.iter().map(|t| t.view.read(cx).current_dir()).collect()
    }

    /// A pane's terminals as the session keeps them.
    pub(super) fn saved_terminals(&self, items: &[Item], active: usize, cx: &App) -> Vec<SavedTerminal> {
        items
            .iter()
            .enumerate()
            .filter_map(|(ix, item)| match item {
                Item::Terminal(item) => Some(SavedTerminal { position: ix, dir: item.view.read(cx).current_dir(), active: ix == active }),
                _ => None,
            })
            .collect()
    }

    /// Reopens a pane's saved terminals among its files (fresh shells,
    /// started when shown).
    pub(super) fn restore_pane_terminals(&mut self, pane: super::PaneId, saved: &[SavedTerminal], window: &mut Window, cx: &mut Context<Self>) {
        let mut saved: Vec<&SavedTerminal> = saved.iter().filter(|t| t.dir.is_dir()).collect();
        saved.sort_by_key(|t| t.position);
        for terminal in saved {
            let (view, subscriptions) = self.new_terminal_view(Spawn::shell(terminal.dir.clone()), window, cx);
            let Some(p) = self.pane_mut(pane) else { return };
            let ix = terminal.position.min(p.items.len());
            p.items.insert(ix, Item::Terminal(TerminalItem { view, _subscriptions: subscriptions }));
            if terminal.active {
                p.active = ix;
            } else if p.active >= ix && p.items.len() > 1 {
                p.active += 1;
            }
        }
    }

    /// Reopens the debug area's saved terminals (started when shown).
    pub(super) fn restore_panel_terminals(&mut self, dirs: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        for dir in dirs.iter().filter(|dir| dir.is_dir()) {
            let (view, subscriptions) = self.new_terminal_view(Spawn::shell(dir.clone()), window, cx);
            self.terminals.panel.push(PanelTerminal { view, _subscriptions: subscriptions });
        }
    }

    // ---------------------------------------------------------------------
    // The control channel

    /// Every terminal: where it is and what it shows.
    pub(super) fn terminals_summary(&self, window: &Window, cx: &App) -> serde_json::Value {
        let mut list = vec![];
        for (ix, terminal) in self.terminals.panel.iter().enumerate() {
            let mut summary = terminal.view.read(cx).summary(window, cx);
            summary["location"] = "panel".into();
            summary["shown"] = (self.show_debug_area && self.debug_tab == DebugTab::Terminal && ix == self.terminals.active).into();
            list.push(summary);
        }
        for (pane_ix, id) in self.pane_group.panes().into_iter().enumerate() {
            let Some(pane) = self.pane(id) else { continue };
            for (ix, item) in pane.items.iter().enumerate() {
                if let Item::Terminal(item) = item {
                    let mut summary = item.view.read(cx).summary(window, cx);
                    summary["location"] = format!("pane {pane_ix} tab {ix}").into();
                    summary["shown"] = (pane.active == ix).into();
                    list.push(summary);
                }
            }
        }
        serde_json::json!({ "list": list, "panel_active": self.terminals.active, "tab_shown": self.debug_tab == DebugTab::Terminal })
    }

    /// The terminals in `terminals_summary`'s order.
    fn terminal_views(&self) -> Vec<Entity<TerminalView>> {
        let mut views: Vec<Entity<TerminalView>> = self.terminals.panel.iter().map(|t| t.view.clone()).collect();
        for id in self.pane_group.panes() {
            views.extend(self.pane(id).into_iter().flat_map(|p| p.items.iter()).filter_map(|item| match item {
                Item::Terminal(item) => Some(item.view.clone()),
                _ => None,
            }));
        }
        views
    }

    /// The control channel's `terminal` command:
    /// `panel [<dir>] [-- <script>]` and `tab [<dir>] [-- <script>]` open a
    /// terminal (a `/bin/sh -c` script instead of the shell), `at <path>`
    /// is Open Terminal at Location, `type <n> <text>` types into terminal
    /// `n` (`\n` is Return), `focus <n>`, `close <n>`, `select-all <n>`,
    /// `copy <n>` (⌘C), `clear <n>` (⌘K), `scroll <n> <lines>`,
    /// `link <n> <row> <column>` (⌘-click a cell), `clipboard` (its text).
    pub fn terminal_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        let (verb, rest) = command.split_once(' ').unwrap_or((command, ""));
        let views = self.terminal_views();
        let nth = |arg: Option<&str>| -> Result<Entity<TerminalView>, String> {
            let n: usize = arg.and_then(|a| a.parse().ok()).ok_or(format!("usage: terminal {verb} <n> …"))?;
            views.get(n).cloned().ok_or(format!("no terminal {n}"))
        };
        let mut words = rest.split_whitespace();
        match verb {
            "panel" | "tab" => {
                let (dir, script) = match rest.split_once("--") {
                    Some((dir, script)) => (dir.trim(), Some(script.trim())),
                    None => (rest.trim(), None),
                };
                let dir = if dir.is_empty() { self.project.root.clone() } else { PathBuf::from(dir) };
                let spawn = match script {
                    Some(script) => Spawn::script(dir, script),
                    None => Spawn::shell(dir),
                };
                if verb == "panel" {
                    self.new_panel_terminal(spawn, window, cx);
                } else {
                    self.new_terminal_tab(spawn, window, cx);
                }
            }
            "at" => self.open_terminal_at(Some(PathBuf::from(rest.trim())), window, cx),
            "type" => {
                let view = nth(words.next())?;
                let text = rest.split_once(' ').map(|(_, text)| text).unwrap_or("").replace("\\n", "\n");
                view.update(cx, |view, cx| view.type_text(&text, cx))?;
            }
            "focus" => {
                let view = nth(words.next())?;
                if let Some(ix) = self.terminals.panel.iter().position(|t| t.view == view) {
                    self.terminals.active = ix;
                    self.show_panel_terminal(window, cx);
                } else if let Some((pane, ix)) = self.find_view(view.entity_id()) {
                    self.activate_item(pane, ix, window, cx);
                }
            }
            "close" => {
                let view = nth(words.next())?;
                self.close_terminal(view.entity_id(), window, cx);
            }
            "select-all" => nth(words.next())?.update(cx, |view, cx| view.select_all(cx)),
            "copy" => {
                let text = nth(words.next())?.read(cx).selected_text().ok_or("nothing is selected")?;
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            "clear" => nth(words.next())?.update(cx, |view, cx| view.clear(cx)),
            "scroll" => {
                let view = nth(words.next())?;
                let lines: i32 = words.next().and_then(|l| l.parse().ok()).ok_or("usage: terminal scroll <n> <lines>")?;
                view.update(cx, |view, cx| view.scroll_lines(lines, cx));
            }
            "link" => {
                let view = nth(words.next())?;
                let numbers: Vec<usize> = words.filter_map(|w| w.parse().ok()).collect();
                let [row, column] = numbers[..] else { return Err("usage: terminal link <n> <row> <column>".into()) };
                if !view.update(cx, |view, cx| view.open_link_at_cell(row, column, cx)) {
                    return Err(format!("no link at row {row}, column {column}"));
                }
            }
            "clipboard" => return Ok(cx.read_from_clipboard().and_then(|item| item.text()).unwrap_or_default()),
            _ => return Err(format!("unknown terminal command “{verb}”")),
        }
        Ok(String::new())
    }
}

/// A folder, or the folder a file is in.
fn folder_of(path: &Path) -> PathBuf {
    if path.is_dir() { path.to_path_buf() } else { path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.to_path_buf()) }
}

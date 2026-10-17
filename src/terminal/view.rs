//! A terminal on screen, in an editor pane's tab or the debug area: keys
//! to the shell (⌘ keys stay zcode's), selection with the mouse (double-
//! click a word, triple-click a line) and ⌘C, ⌘V paste, the wheel through
//! the scrollback, ⌘-click on a link, and the mouse reported to programs
//! that ask (`vim`, `htop`). The shell starts when the terminal is first
//! laid out, at that size, on a background thread.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use alacritty_terminal::grid::{Dimensions as _, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::Handler as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::colors::Palette;
use super::element::TerminalElement;
use super::keys::{self, MouseAction, MouseButton as ReportButton};
use super::links::{self, Target};
use super::{GridSize, Notice, Spawn, Terminal};
use crate::actions::{ClearConsole, CopyOrCopyLine, PasteOrPasteLine};

pub enum TerminalEvent {
    /// What its tab is called changed: a program set a title, or it exited.
    TitleChanged,
    /// ⌘-click on a file's path (`file:line:column`).
    OpenFile { path: PathBuf, line: Option<u32>, column: Option<u32> },
    /// The process ended; `close` when its tab should go with it (the
    /// user's shell, ended with `exit`).
    Exited { close: bool },
}

enum State {
    /// Waiting for its first layout, which says what size to start at.
    Pending,
    Starting,
    Running(Arc<Terminal>),
    Failed(String),
}

/// Where the grid was last painted, so the mouse can find its cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GridLayout {
    pub origin: Point<Pixels>,
    pub cell: Size<Pixels>,
    pub columns: usize,
    pub rows: usize,
}

/// The link under the mouse while ⌘ is down: its cells, first to last.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct HoveredLink {
    pub start: GridPoint,
    pub end: GridPoint,
    pub target: Target,
}

pub struct TerminalView {
    focus_handle: FocusHandle,
    spawn: Spawn,
    state: State,
    /// The title a program set (OSC 0/2).
    title: Option<String>,
    /// The exit code once the process ended (`Some(None)`: killed).
    exit: Option<Option<i32>>,
    pub font_size: f32,
    pub(super) layout: Option<GridLayout>,
    /// A selection is being dragged.
    selecting: bool,
    /// The program got the mouse button that's down.
    reported_press: Option<ReportButton>,
    pub(super) hovered_link: Option<HoveredLink>,
    /// Pixels of wheel movement short of a line.
    scroll_remainder: f32,
    _notices: Option<Task<()>>,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    pub fn new(spawn: Spawn, font_size: f32, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            spawn,
            state: State::Pending,
            title: None,
            exit: None,
            font_size,
            layout: None,
            selecting: false,
            reported_press: None,
            hovered_link: None,
            scroll_remainder: 0.,
            _notices: None,
        }
    }

    pub fn spawn(&self) -> &Spawn {
        &self.spawn
    }

    pub(super) fn terminal(&self) -> Option<&Arc<Terminal>> {
        match &self.state {
            State::Running(terminal) => Some(terminal),
            _ => None,
        }
    }

    /// Ends the process (its tab or window is closing).
    pub fn stop(&mut self) {
        if let Some(terminal) = self.terminal() {
            terminal.stop();
        }
    }

    pub fn is_running(&self) -> bool {
        self.terminal().is_some_and(|t| t.is_running())
    }

    pub fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        if self.font_size != size {
            self.font_size = size;
            cx.notify();
        }
    }

    /// The directory a new shell would start in to pick up where this one
    /// is: the shell's own (after any `cd`), or where it started.
    pub fn current_dir(&self) -> PathBuf {
        self.terminal().and_then(|t| t.current_dir()).unwrap_or_else(|| self.spawn.dir.clone())
    }

    /// Its tab's name: the program's title, else what's running.
    pub fn title(&self) -> String {
        let name = self
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| self.terminal().and_then(|t| t.process_name()))
            .unwrap_or_else(|| "Terminal".into());
        match self.exit {
            Some(_) => format!("{name} (ended)"),
            None => name,
        }
    }

    // ---------------------------------------------------------------------
    // Starting, and what the shell says

    /// After a layout: starts the shell at this size, or fits the PTY to it.
    pub(super) fn laid_out(&mut self, layout: GridLayout, cx: &mut Context<Self>) {
        self.layout = Some(layout);
        let size = GridSize {
            columns: layout.columns as u16,
            rows: layout.rows as u16,
            cell_width: f32::from(layout.cell.width).round() as u16,
            cell_height: f32::from(layout.cell.height).round() as u16,
        };
        match &self.state {
            State::Pending => self.start(size, cx),
            State::Running(terminal) => terminal.resize(size),
            State::Starting | State::Failed(_) => {}
        }
    }

    fn start(&mut self, size: GridSize, cx: &mut Context<Self>) {
        self.state = State::Starting;
        let spawn = self.spawn.clone();
        let task = cx.spawn(async move |this, cx| {
            let started = cx.background_spawn(async move { Terminal::start(spawn, size) }).await;
            let Ok(notices) = this.update(cx, |view, cx| match started {
                Ok((terminal, notices)) => {
                    view.state = State::Running(Arc::new(terminal));
                    cx.emit(TerminalEvent::TitleChanged);
                    cx.notify();
                    Some(notices)
                }
                Err(err) => {
                    log::warn!("couldn’t start a terminal in {}: {err}", view.spawn.dir.display());
                    view.state = State::Failed(err.to_string());
                    cx.notify();
                    None
                }
            }) else {
                return;
            };
            let Some(notices) = notices else { return };
            while let Ok(first) = notices.recv().await {
                let mut batch = vec![first];
                batch.extend(std::iter::from_fn(|| notices.try_recv().ok()));
                if this.update(cx, |view, cx| view.handle(batch, cx)).is_err() {
                    break;
                }
                // Output is shown at most once a frame, however fast it comes.
                cx.background_executor().timer(Duration::from_millis(8)).await;
            }
        });
        self._notices = Some(task);
    }

    fn handle(&mut self, notices: Vec<Notice>, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal().cloned() else { return };
        // Output from here on is announced again.
        terminal.output_pending.store(false, Ordering::Release);
        for notice in notices {
            match notice {
                Notice::Output => {}
                Notice::Title(title) => {
                    self.title = title;
                    cx.emit(TerminalEvent::TitleChanged);
                }
                Notice::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                Notice::ColorRequest(index, format) => {
                    let palette = Palette::from_theme(cx.theme());
                    let rgb = palette.rgb(index, terminal.lock().colors());
                    terminal.write(format(rgb).into_bytes());
                }
                Notice::Exited(code) => {
                    if self.exit.is_some() {
                        continue;
                    }
                    self.exit = Some(code);
                    terminal.stop();
                    let message = match code {
                        Some(0) | None => "\r\n[Process completed]".to_string(),
                        Some(code) => format!("\r\n[Process exited with code {code}]"),
                    };
                    let mut parser = alacritty_terminal::vte::ansi::Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new();
                    parser.advance(&mut *terminal.lock(), message.as_bytes());
                    cx.emit(TerminalEvent::TitleChanged);
                    cx.emit(TerminalEvent::Exited { close: self.spawn.is_shell() && code == Some(0) });
                }
            }
        }
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // Keys

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal().cloned().filter(|t| t.is_running()) else { return };
        let keystroke = &event.keystroke;
        let m = &keystroke.modifiers;
        // ⇧PageUp / ⇧PageDown and ⇧Home / ⇧End go through the scrollback.
        if m.shift && !m.control && !m.alt && !m.platform {
            let scroll = match keystroke.key.as_str() {
                "pageup" => Some(Scroll::PageUp),
                "pagedown" => Some(Scroll::PageDown),
                "home" => Some(Scroll::Top),
                "end" => Some(Scroll::Bottom),
                _ => None,
            };
            if let Some(scroll) = scroll {
                terminal.lock().scroll_display(scroll);
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
        let mode = *terminal.lock().mode();
        let Some(bytes) = keys::bytes_for(keystroke, mode) else { return };
        cx.stop_propagation();
        self.typed(&terminal, cx);
        terminal.write(bytes);
    }

    /// Typing shows the bottom of the output and ends a selection.
    fn typed(&mut self, terminal: &Terminal, cx: &mut Context<Self>) {
        let mut term = terminal.lock();
        if term.grid().display_offset() != 0 || term.selection.is_some() {
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
            cx.notify();
        }
    }

    /// Writes `text` to the terminal as if typed (the control channel's).
    pub fn type_text(&mut self, text: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let terminal = self.terminal().cloned().filter(|t| t.is_running()).ok_or("the terminal isn’t running")?;
        self.typed(&terminal, cx);
        terminal.write(text.replace('\n', "\r").into_bytes());
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Copy, paste, select all, clear

    pub fn selected_text(&self) -> Option<String> {
        self.terminal()?.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    fn copy(&mut self, _: &CopyOrCopyLine, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &PasteOrPasteLine, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        let Some(terminal) = self.terminal().cloned().filter(|t| t.is_running()) else { return };
        let mode = *terminal.lock().mode();
        self.typed(&terminal, cx);
        terminal.write(keys::paste(&text, mode));
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal().cloned() else { return };
        let mut term = terminal.lock();
        let (top, bottom) = (term.topmost_line(), term.bottommost_line());
        let mut selection = Selection::new(SelectionType::Simple, GridPoint::new(top, Column(0)), Side::Left);
        selection.update(GridPoint::new(bottom, term.last_column()), Side::Right);
        term.selection = Some(selection);
        cx.notify();
    }

    /// Clear (⌘K, Clear Console): the scrollback and the screen above the
    /// cursor's line, which moves to the top, as in Terminal.app.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal().cloned() else { return };
        let mut term = terminal.lock();
        let cursor = term.grid().cursor.point;
        if cursor.line.0 > 0 {
            term.scroll_up(cursor.line.0 as usize);
            term.goto(0, cursor.column.0);
        }
        term.grid_mut().clear_history();
        term.selection = None;
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // The mouse

    /// The cell under `position` (clamped to the grid) and which half of it.
    fn grid_point(&self, position: Point<Pixels>, display_offset: usize) -> Option<(GridPoint, Side)> {
        let layout = self.layout?;
        let x = f32::from(position.x - layout.origin.x) / f32::from(layout.cell.width);
        let y = f32::from(position.y - layout.origin.y) / f32::from(layout.cell.height);
        let column = (x.max(0.) as usize).min(layout.columns.saturating_sub(1));
        let row = (y.max(0.) as usize).min(layout.rows.saturating_sub(1));
        let side = if x.fract() > 0.5 && x >= 0. { Side::Right } else { Side::Left };
        Some((GridPoint::new(Line(row as i32 - display_offset as i32), Column(column)), side))
    }

    fn viewport_cell(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        let (point, _) = self.grid_point(position, 0)?;
        Some((point.column.0, point.line.0 as usize))
    }

    pub(super) fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let Some(terminal) = self.terminal().cloned() else { return };
        if event.modifiers.platform && event.button == MouseButton::Left {
            self.update_hovered_link(event.position, true, cx);
            if let Some(link) = self.hovered_link.clone() {
                self.open(link.target, cx);
                return;
            }
        }
        let mode = *terminal.lock().mode();
        if !event.modifiers.shift && mode.intersects(TermMode::MOUSE_MODE) {
            let button = match event.button {
                MouseButton::Left => ReportButton::Left,
                MouseButton::Middle => ReportButton::Middle,
                MouseButton::Right => ReportButton::Right,
                _ => return,
            };
            if let Some((column, line)) = self.viewport_cell(event.position)
                && let Some(report) = keys::mouse_report(button, MouseAction::Press, column, line, &event.modifiers, mode)
            {
                self.reported_press = Some(button);
                terminal.write(report);
            }
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        let mut term = terminal.lock();
        let Some((point, side)) = self.grid_point(event.position, term.grid().display_offset()) else { return };
        let kind = match event.click_count {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        match (&mut term.selection, event.modifiers.shift) {
            // ⇧-click extends the selection.
            (Some(selection), true) => selection.update(point, side),
            _ => term.selection = Some(Selection::new(kind, point, side)),
        }
        self.selecting = true;
        cx.notify();
    }

    pub(super) fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal().cloned() else { return };
        if self.selecting && event.pressed_button == Some(MouseButton::Left) {
            let mut term = terminal.lock();
            // Dragging past the top or bottom scrolls.
            if let Some(layout) = self.layout {
                let top = layout.origin.y;
                let bottom = top + layout.cell.height * layout.rows as f32;
                if event.position.y < top {
                    term.scroll_display(Scroll::Delta(1));
                } else if event.position.y > bottom {
                    term.scroll_display(Scroll::Delta(-1));
                }
            }
            let offset = term.grid().display_offset();
            if let Some((point, side)) = self.grid_point(event.position, offset)
                && let Some(selection) = term.selection.as_mut()
            {
                selection.update(point, side);
            }
            cx.notify();
            return;
        }
        if let Some(button) = self.reported_press {
            let mode = *terminal.lock().mode();
            if let Some((column, line)) = self.viewport_cell(event.position)
                && let Some(report) = keys::mouse_report(button, MouseAction::Drag, column, line, &event.modifiers, mode)
            {
                terminal.write(report);
            }
            return;
        }
        self.update_hovered_link(event.position, event.modifiers.platform, cx);
    }

    pub(super) fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal().cloned() else { return };
        if let Some(button) = self.reported_press.take() {
            let mode = *terminal.lock().mode();
            if let Some((column, line)) = self.viewport_cell(event.position)
                && let Some(report) = keys::mouse_report(button, MouseAction::Release, column, line, &event.modifiers, mode)
            {
                terminal.write(report);
            }
        }
        if self.selecting {
            self.selecting = false;
            let mut term = terminal.lock();
            if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                term.selection = None;
            }
            cx.notify();
        }
    }

    pub(super) fn scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let (Some(terminal), Some(layout)) = (self.terminal().cloned(), self.layout) else { return };
        let delta = f32::from(event.delta.pixel_delta(layout.cell.height).y) + self.scroll_remainder;
        let lines = (delta / f32::from(layout.cell.height)) as i32;
        self.scroll_remainder = delta - lines as f32 * f32::from(layout.cell.height);
        if lines == 0 {
            return;
        }
        let mode = *terminal.lock().mode();
        if mode.intersects(TermMode::MOUSE_MODE) && !event.modifiers.shift {
            let button = if lines > 0 { ReportButton::WheelUp } else { ReportButton::WheelDown };
            if let Some((column, line)) = self.viewport_cell(event.position) {
                for _ in 0..lines.unsigned_abs() {
                    if let Some(report) = keys::mouse_report(button, MouseAction::Press, column, line, &event.modifiers, mode) {
                        terminal.write(report);
                    }
                }
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            // Full-screen programs without the mouse (`less`, `man`) get arrows.
            let arrow: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            terminal.write(arrow.repeat(lines.unsigned_abs() as usize));
        } else {
            terminal.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    /// Scrolls the scrollback by `lines` (up is positive), as the wheel
    /// does (the control channel's).
    pub fn scroll_lines(&mut self, lines: i32, cx: &mut Context<Self>) {
        if let Some(terminal) = self.terminal() {
            terminal.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    pub(super) fn modifiers_changed(&mut self, modifiers: Modifiers, window: &Window, cx: &mut Context<Self>) {
        self.update_hovered_link(window.mouse_position(), modifiers.platform, cx);
    }

    /// The link under the mouse, while ⌘ is down.
    fn update_hovered_link(&mut self, position: Point<Pixels>, command: bool, cx: &mut Context<Self>) {
        let link = command.then(|| self.link_at(position)).flatten();
        if link != self.hovered_link {
            self.hovered_link = link;
            cx.notify();
        }
    }

    /// The link at a window position: the line under it (with the rows it
    /// wraps onto), searched around that cell.
    fn link_at(&self, position: Point<Pixels>) -> Option<HoveredLink> {
        let terminal = self.terminal()?;
        let dir = terminal.current_dir().unwrap_or_else(|| self.spawn.dir.clone());
        let term = terminal.lock();
        let (point, _) = self.grid_point(position, term.grid().display_offset())?;
        let grid = term.grid();
        let last = term.last_column();
        let wraps = |line: Line| grid[line][last].flags.contains(alacritty_terminal::term::cell::Flags::WRAPLINE);
        let mut first = point.line;
        while first > term.topmost_line() && wraps(first - 1) {
            first -= 1;
        }
        let mut end = point.line;
        while end < term.bottommost_line() && wraps(end) {
            end += 1;
        }
        let mut text = String::new();
        let mut cells = vec![];
        let mut at = None;
        for line in first.0..=end.0 {
            for column in 0..term.columns() {
                let cell = &grid[Line(line)][Column(column)];
                if cell.flags.intersects(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                let here = GridPoint::new(Line(line), Column(column));
                if here == point {
                    at = Some(cells.len());
                }
                text.push(cell.c);
                cells.push(here);
            }
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let (range, target) = links::link_at(&text, at?, &dir, home.as_deref())?;
        Some(HoveredLink { start: cells[range.start], end: cells[range.end - 1], target })
    }

    fn open(&mut self, target: Target, cx: &mut Context<Self>) {
        match target {
            Target::Url(url) => cx.open_url(&url),
            Target::File { path, line, column } => cx.emit(TerminalEvent::OpenFile { path, line, column }),
        }
        self.hovered_link = None;
        cx.notify();
    }

    /// Opens the link at a cell of the screen, as ⌘-click does (the control
    /// channel's); whether there was one.
    pub fn open_link_at_cell(&mut self, row: usize, column: usize, cx: &mut Context<Self>) -> bool {
        let Some(layout) = self.layout else { return false };
        let position = layout.origin
            + point(layout.cell.width * (column as f32 + 0.5), layout.cell.height * (row as f32 + 0.5));
        match self.link_at(position) {
            Some(link) => {
                self.open(link.target, cx);
                true
            }
            None => false,
        }
    }

    // ---------------------------------------------------------------------
    // The control channel

    /// The screen's text (trailing blanks trimmed), and how the terminal is.
    pub fn summary(&self, window: &Window, cx: &App) -> serde_json::Value {
        let focused = self.focus_handle.contains_focused(window, cx);
        let state = match &self.state {
            State::Pending => "pending",
            State::Starting => "starting",
            State::Running(t) if t.is_running() => "running",
            State::Running(_) => "ended",
            State::Failed(_) => "failed",
        };
        let mut summary = serde_json::json!({
            "title": self.title(),
            "state": state,
            "dir": self.spawn.dir,
            "exit": self.exit.map(|code| code.map_or(serde_json::Value::Null, serde_json::Value::from)),
            "focused": focused,
            "font_size": self.font_size,
            "hovered_link": self.hovered_link.as_ref().map(|l| format!("{:?}", l.target)),
            "layout": self.layout.map(|l| serde_json::json!({
                "x": f32::from(l.origin.x), "y": f32::from(l.origin.y),
                "cell_width": f32::from(l.cell.width), "cell_height": f32::from(l.cell.height),
                "columns": l.columns, "rows": l.rows,
            })),
        });
        if let Some(terminal) = self.terminal() {
            let size = terminal.size();
            let term = terminal.lock();
            let mut lines: Vec<String> = vec![];
            let mut current = String::new();
            let mut line = None;
            for cell in term.grid().display_iter() {
                if line.is_some_and(|l| l != cell.point.line) {
                    lines.push(current.trim_end().to_string());
                    current.clear();
                }
                line = Some(cell.point.line);
                if !cell.flags.contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER) {
                    current.push(cell.c);
                }
            }
            lines.push(current.trim_end().to_string());
            summary["pid"] = terminal.pid.into();
            summary["current_dir"] = serde_json::json!(terminal.current_dir());
            summary["process"] = serde_json::json!(terminal.process_name());
            summary["pty"] = serde_json::json!({ "columns": size.columns, "rows": size.rows });
            summary["grid"] = serde_json::json!({ "columns": term.columns(), "rows": term.screen_lines() });
            summary["history"] = term.history_size().into();
            summary["display_offset"] = term.grid().display_offset().into();
            summary["cursor"] = serde_json::json!([term.grid().cursor.point.line.0, term.grid().cursor.point.column.0]);
            summary["alt_screen"] = term.mode().contains(TermMode::ALT_SCREEN).into();
            summary["selection"] = serde_json::json!(term.selection_to_string());
            summary["lines"] = serde_json::json!(lines);
        }
        summary
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let focused = self.focus_handle.clone();
        let failed = match &self.state {
            State::Failed(err) => Some(err.clone()),
            _ => None,
        };
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .bg(theme.background)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(|this, _: &zcode_editor::SelectAll, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &ClearConsole, _, cx| this.clear(cx)))
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, window, cx| {
                this.modifiers_changed(event.modifiers, window, cx)
            }))
            .map(|el| match failed {
                Some(err) => el.p_3().text_sm().text_color(theme.danger).child(format!("The terminal couldn’t start: {err}")),
                None => el.child(TerminalElement::new(cx.entity(), focused)),
            })
    }
}

//! The LLDB console in the debug area: while the program is paused, an
//! `(lldb)` prompt under the output takes `po`, `p`, `v`, `expr` and any
//! other lldb command (lldb-dap `evaluate` in the `repl` context), with ↑/↓
//! for history and an empty Return repeating the last command, as lldb
//! does. Also the Variables view's Copy Value and Print Description.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::*;

use super::{DebugTab, Workspace};
use crate::actions::{PickerSelectNext, PickerSelectPrevious};
use crate::debugger::DebugState;

#[derive(Default)]
pub(super) struct Console {
    /// Made the first time execution pauses.
    input: Option<Entity<InputState>>,
    history: Vec<String>,
    /// Where ↑/↓ are in `history`.
    cursor: Option<usize>,
    _subscription: Option<Subscription>,
}

impl Workspace {
    /// The prompt's input, made on first use.
    pub(super) fn console_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(input) = &self.console.input {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("lldb command, e.g. po self"));
        self.console._subscription = Some(cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit_console(window, cx);
            }
        }));
        self.console.input = Some(input.clone());
        input
    }

    fn console_paused(&self, cx: &App) -> bool {
        matches!(self.debugger.read(cx).state, DebugState::Stopped { .. })
    }

    /// Runs what's at the prompt; an empty line repeats the last command.
    pub(super) fn submit_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.console_input(window, cx);
        let typed = input.read(cx).value().trim().to_string();
        let Some(command) = Some(typed).filter(|c| !c.is_empty()).or_else(|| self.console.history.last().cloned()) else {
            return;
        };
        if self.console.history.last() != Some(&command) {
            self.console.history.push(command.clone());
        }
        self.console.cursor = None;
        input.update(cx, |input, cx| input.set_value("", window, cx));
        self.debug_tab = DebugTab::Console;
        self.debugger.update(cx, |d, cx| d.run_console_command(&command, cx));
    }

    /// Types and runs a console command (the test harness's `console`).
    pub fn console_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        if !self.console_paused(cx) {
            return Err("the debugger isn't paused".into());
        }
        let input = self.console_input(window, cx);
        input.update(cx, |input, cx| input.set_value(command.to_string(), window, cx));
        self.submit_console(window, cx);
        Ok(())
    }

    /// Activate Console (⇧⌘C): shows the console and, when paused, focuses
    /// its prompt.
    pub(super) fn activate_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_debug_area = true;
        self.debug_tab = DebugTab::Console;
        if self.console_paused(cx) {
            let input = self.console_input(window, cx);
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    /// ↑ (`-1`) and ↓ (`1`) through the commands run before.
    fn step_console_history(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.console.history.len();
        if len == 0 {
            return;
        }
        let cursor = match (self.console.cursor, delta < 0) {
            (None, true) => Some(len - 1),
            (None, false) => None,
            (Some(ix), true) => Some(ix.saturating_sub(1)),
            (Some(ix), false) => (ix + 1 < len).then_some(ix + 1),
        };
        self.console.cursor = cursor;
        let text = cursor.map(|ix| self.console.history[ix].clone()).unwrap_or_default();
        let input = self.console_input(window, cx);
        input.update(cx, |input, cx| input.set_value(text, window, cx));
    }

    /// The `(lldb)` prompt, shown under the console while paused.
    pub(super) fn render_console_prompt(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.debug_tab != DebugTab::Console || !self.console_paused(cx) {
            return None;
        }
        let input = self.console.input.clone()?;
        let theme = cx.theme().clone();
        Some(
            h_flex()
                .key_context("LldbConsole")
                .on_action(cx.listener(|this, _: &PickerSelectPrevious, window, cx| this.step_console_history(-1, window, cx)))
                .on_action(cx.listener(|this, _: &PickerSelectNext, window, cx| this.step_console_history(1, window, cx)))
                .flex_none()
                .px_2()
                .gap_1()
                .border_t_1()
                .border_color(theme.border)
                .font_family(theme.mono_font_family.clone())
                .text_xs()
                .child(div().flex_none().text_color(theme.muted_foreground).child("(lldb)"))
                .child(div().flex_1().min_w_0().child(Input::new(&input).xsmall().appearance(false)))
                .into_any_element(),
        )
    }

    /// Variables view: Copy Value and Print Description.
    pub(super) fn variable_menu(menu: PopupMenu, index: usize, name: &str, value: String, workspace: WeakEntity<Workspace>) -> PopupMenu {
        menu.item(PopupMenuItem::new("Copy Value").on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
        }))
        .item(PopupMenuItem::new(format!("Print Description of “{name}”")).on_click(move |_, _, cx| {
            workspace
                .update(cx, |ws, cx| {
                    ws.debug_tab = DebugTab::Console;
                    ws.debugger.update(cx, |d, cx| d.print_description(index, cx));
                })
                .ok();
        }))
    }

    /// The Variables view's commands for the test harness: `copy` or
    /// `describe` the first variable named `name`.
    pub fn variable_command(&mut self, name: &str, command: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let debugger = self.debugger.read(cx);
        let index = debugger.variables.iter().position(|v| v.name == name).ok_or(format!("no variable {name}"))?;
        match command {
            "copy" => cx.write_to_clipboard(ClipboardItem::new_string(debugger.variables[index].value.clone())),
            "describe" => {
                self.debug_tab = DebugTab::Console;
                self.debugger.update(cx, |d, cx| d.print_description(index, cx));
            }
            _ => return Err("expected copy or describe".into()),
        }
        Ok(())
    }

    pub(super) fn console_summary(&self, cx: &App) -> serde_json::Value {
        serde_json::json!({
            "prompt": self.debug_tab == DebugTab::Console && self.console_paused(cx) && self.console.input.is_some(),
            "input": self.console.input.as_ref().map(|i| i.read(cx).value().to_string()),
            "history": self.console.history,
        })
    }
}

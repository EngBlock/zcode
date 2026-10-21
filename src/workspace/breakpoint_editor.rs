//! Xcode's breakpoint editor popover (Edit Breakpoint…, or double-click a
//! breakpoint): name, condition, ignore count, actions (Log Message,
//! Debugger Command, Sound) and "Automatically continue after evaluating
//! actions", plus a symbolic breakpoint's symbol and module and an
//! exception breakpoint's exception and break. Like Xcode's popover it has
//! no Cancel: closing it (Done, Escape, a click outside) keeps the changes.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::{PickerConfirm, PickerDismiss};
use crate::debugger::Breakpoint;
use crate::debugger::breakpoints::{self, Action, ExceptionLanguage, ExceptionPhase, Kind, SOUNDS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionKind {
    Log,
    Command,
    Sound,
}

impl ActionKind {
    const ALL: [Self; 3] = [Self::Log, Self::Command, Self::Sound];

    fn label(self) -> &'static str {
        match self {
            Self::Log => "Log Message",
            Self::Command => "Debugger Command",
            Self::Sound => "Sound",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "log" => Some(Self::Log),
            "command" => Some(Self::Command),
            "sound" => Some(Self::Sound),
            _ => None,
        }
    }
}

struct ActionRow {
    kind: ActionKind,
    /// The message or the command.
    text: Entity<InputState>,
    speak: bool,
    sound: String,
}

pub struct BreakpointEditor {
    focus_handle: FocusHandle,
    /// The breakpoint as opened; exception choices are edited in place.
    breakpoint: Breakpoint,
    name: Entity<InputState>,
    condition: Entity<InputState>,
    ignore: Entity<InputState>,
    symbol: Entity<InputState>,
    module: Entity<InputState>,
    actions: Vec<ActionRow>,
    auto_continue: bool,
    /// Where Done was last drawn (window points), for the test harness.
    done_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl EventEmitter<DismissEvent> for BreakpointEditor {}

impl Focusable for BreakpointEditor {
    /// Typing goes to a symbolic breakpoint's symbol, else to the condition,
    /// else to the name.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        let field = match self.breakpoint.kind {
            Kind::Symbolic { .. } => &self.symbol,
            _ if self.breakpoint.has_condition() => &self.condition,
            _ => &self.name,
        };
        field.read(cx).focus_handle(cx)
    }
}

fn input(placeholder: &'static str, value: String, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
}

fn placeholder(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::Log => "Message: @expression@, %B breakpoint name, %H hit count",
        ActionKind::Command => "Debugger command, e.g. po self",
        ActionKind::Sound => "",
    }
}

impl BreakpointEditor {
    pub fn new(breakpoint: Breakpoint, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (symbol, module) = match &breakpoint.kind {
            Kind::Symbolic { symbol, module } => (symbol.clone(), module.clone()),
            _ => Default::default(),
        };
        let ignore = if breakpoint.ignore_count > 0 { breakpoint.ignore_count.to_string() } else { String::new() };
        let actions = breakpoint
            .actions
            .iter()
            .map(|action| {
                let (kind, text, speak, sound) = match action {
                    Action::Log { message, speak } => (ActionKind::Log, message.clone(), *speak, String::new()),
                    Action::Command { command } => (ActionKind::Command, command.clone(), false, String::new()),
                    Action::Sound { name } => (ActionKind::Sound, String::new(), false, name.clone()),
                };
                ActionRow { kind, text: input(placeholder(kind), text, window, cx), speak, sound }
            })
            .collect();
        Self {
            focus_handle: cx.focus_handle(),
            name: input("Name", breakpoint.name.clone(), window, cx),
            condition: input("Condition, e.g. count > 3", breakpoint.condition.clone(), window, cx),
            ignore: input("0", ignore, window, cx),
            symbol: input("Symbol, e.g. -[NSException raise] or Greeter.greeting", symbol, window, cx),
            module: input("Module (optional)", module, window, cx),
            actions,
            auto_continue: breakpoint.auto_continue,
            breakpoint,
            done_bounds: Rc::default(),
        }
    }

    /// The breakpoint as edited.
    pub fn edited(&self, cx: &App) -> Breakpoint {
        let mut bp = self.breakpoint.clone();
        bp.name = self.name.read(cx).value().trim().to_string();
        if let Kind::Symbolic { symbol, module } = &mut bp.kind {
            *symbol = self.symbol.read(cx).value().trim().to_string();
            *module = self.module.read(cx).value().trim().to_string();
        }
        if bp.has_condition() {
            bp.condition = self.condition.read(cx).value().trim().to_string();
            bp.ignore_count = self.ignore.read(cx).value().trim().parse().unwrap_or(0);
        }
        bp.actions = self
            .actions
            .iter()
            .filter_map(|row| {
                let text = row.text.read(cx).value().to_string();
                match row.kind {
                    ActionKind::Log if !text.trim().is_empty() => Some(Action::Log { message: text, speak: row.speak }),
                    ActionKind::Command if !text.trim().is_empty() => Some(Action::Command { command: text.trim().to_string() }),
                    ActionKind::Sound => Some(Action::Sound { name: row.sound.clone() }),
                    _ => None,
                }
            })
            .collect();
        bp.auto_continue = self.auto_continue;
        bp
    }

    /// The workspace saves the edit as the modal closes, once this update
    /// is over: it reads the editor, which it can't do from inside it.
    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn confirm(&mut self, _: &PickerConfirm, _: &mut Window, cx: &mut Context<Self>) {
        self.close(cx);
    }

    fn dismiss(&mut self, _: &PickerDismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.close(cx);
    }

    fn add_action(&mut self, kind: ActionKind, at: usize, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        // A sound action's text names the sound.
        let sound = SOUNDS.iter().find(|s| **s == text).unwrap_or(&SOUNDS[0]).to_string();
        let text = if kind == ActionKind::Sound { "" } else { text };
        let row = ActionRow { kind, text: input(placeholder(kind), text.to_string(), window, cx), speak: false, sound };
        self.actions.insert(at.min(self.actions.len()), row);
        cx.notify();
    }

    fn set_action_kind(&mut self, ix: usize, kind: ActionKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.actions.get_mut(ix) else { return };
        if row.kind != kind {
            row.kind = kind;
            row.text = input(placeholder(kind), String::new(), window, cx);
            cx.notify();
        }
    }

    /// Sets a field as the popover's controls do (for the test harness):
    /// `name`, `condition`, `ignore`, `symbol`, `module` take text;
    /// `exception All|Objective-C|C++`, `break throw|catch`,
    /// `add-action log|command|sound [text]`, `speak on|off` and `sound
    /// <name>` (the last action), `auto-continue on|off`.
    pub fn set_field(&mut self, field: &str, value: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let on = || match value {
            "on" => Ok(true),
            "off" => Ok(false),
            _ => Err(format!("{field}: expected on or off")),
        };
        let text = match field {
            "name" => Some(&self.name),
            "condition" => Some(&self.condition),
            "ignore" => Some(&self.ignore),
            "symbol" => Some(&self.symbol),
            "module" => Some(&self.module),
            _ => None,
        };
        if let Some(text) = text.cloned() {
            text.update(cx, |input, cx| input.set_value(value.to_string(), window, cx));
            return Ok(());
        }
        match field {
            "exception" => {
                let language = ExceptionLanguage::ALL
                    .into_iter()
                    .find(|l| l.label() == value)
                    .ok_or(format!("no exception {value}"))?;
                if let Kind::Exception { language: current, .. } = &mut self.breakpoint.kind {
                    *current = language;
                }
            }
            "break" => {
                let phase = match value {
                    "throw" => ExceptionPhase::Throw,
                    "catch" => ExceptionPhase::Catch,
                    _ => return Err("break: expected throw or catch".into()),
                };
                if let Kind::Exception { on, .. } = &mut self.breakpoint.kind {
                    *on = phase;
                }
            }
            "add-action" => {
                let (kind, text) = value.split_once(' ').unwrap_or((value, ""));
                let kind = ActionKind::parse(kind).ok_or(format!("no action {kind}"))?;
                self.add_action(kind, self.actions.len(), text, window, cx);
            }
            "speak" => {
                let speak = on()?;
                self.actions.last_mut().ok_or("no action")?.speak = speak;
            }
            "sound" => {
                let sound = SOUNDS.iter().find(|s| **s == value).ok_or(format!("no sound {value}"))?;
                self.actions.last_mut().ok_or("no action")?.sound = sound.to_string();
            }
            "auto-continue" => self.auto_continue = on()?,
            other => return Err(format!("unknown field {other}")),
        }
        cx.notify();
        Ok(())
    }

    pub fn summary(&self, cx: &App) -> serde_json::Value {
        let bp = self.edited(cx);
        let done = self.done_bounds.get().map(|b| serde_json::json!({ "x": f32::from(b.center().x), "y": f32::from(b.center().y) }));
        serde_json::json!({
            "title": bp.title(),
            "shows_condition": bp.has_condition(),
            "actions": bp.actions.iter().map(Action::label).collect::<Vec<_>>(),
            "done": done,
        })
    }
}

fn row(label: &'static str, content: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .gap_3()
        .py_0p5()
        .items_center()
        .child(div().w(px(84.)).flex_none().text_right().text_color(cx.theme().muted_foreground).child(label))
        .child(div().flex_1().min_w_0().child(content))
}

impl BreakpointEditor {
    fn render_kind(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let editor = cx.entity().downgrade();
        match self.breakpoint.kind.clone() {
            Kind::Line { .. } => v_flex().child(row("Location", div().child(self.breakpoint.title()), cx)),
            Kind::Symbolic { .. } => v_flex()
                .child(row("Symbol", Input::new(&self.symbol).small(), cx))
                .child(row("Module", Input::new(&self.module).small(), cx)),
            Kind::SwiftError => v_flex().child(row(
                "Stops",
                div().text_color(theme.muted_foreground).child("when Swift code throws an error"),
                cx,
            )),
            Kind::Exception { language, on } => {
                let languages = {
                    let editor = editor.clone();
                    Button::new("bp-exception")
                        .label(language.label())
                        .small()
                        .outline()
                        .w(px(160.))
                        .dropdown_menu(move |mut menu, _, _| {
                            for choice in ExceptionLanguage::ALL {
                                let editor = editor.clone();
                                menu = menu.item(PopupMenuItem::new(choice.label()).checked(choice == language).on_click(
                                    move |_, _, cx| {
                                        editor
                                            .update(cx, |this, cx| {
                                                if let Kind::Exception { language, .. } = &mut this.breakpoint.kind {
                                                    *language = choice;
                                                }
                                                cx.notify();
                                            })
                                            .ok();
                                    },
                                ));
                            }
                            menu
                        })
                };
                let phases = Button::new("bp-break")
                    .label(on.label())
                    .small()
                    .outline()
                    .w(px(160.))
                    .dropdown_menu(move |mut menu, _, _| {
                        for choice in [ExceptionPhase::Throw, ExceptionPhase::Catch] {
                            let editor = editor.clone();
                            menu = menu.item(PopupMenuItem::new(choice.label()).checked(choice == on).on_click(move |_, _, cx| {
                                editor
                                    .update(cx, |this, cx| {
                                        if let Kind::Exception { on, .. } = &mut this.breakpoint.kind {
                                            *on = choice;
                                        }
                                        cx.notify();
                                    })
                                    .ok();
                            }));
                        }
                        menu
                    });
                v_flex().child(row("Exception", languages, cx)).child(row("Break", phases, cx))
            }
        }
    }

    fn render_actions(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let editor = cx.entity().downgrade();
        if self.actions.is_empty() {
            return v_flex().child(row(
                "Action",
                Button::new("bp-add-action").label("Add Action").small().outline().on_click(cx.listener(
                    |this, _, window, cx| this.add_action(ActionKind::Log, 0, "", window, cx),
                )),
                cx,
            ));
        }
        let mut list = v_flex().gap_1();
        for (ix, action) in self.actions.iter().enumerate() {
            let kind = action.kind;
            let picker = {
                let editor = editor.clone();
                Button::new(("bp-action-kind", ix))
                    .label(kind.label())
                    .small()
                    .outline()
                    .w(px(150.))
                    .dropdown_menu(move |mut menu, _, _| {
                        for choice in ActionKind::ALL {
                            let editor = editor.clone();
                            menu = menu.item(PopupMenuItem::new(choice.label()).checked(choice == kind).on_click(
                                move |_, window, cx| {
                                    editor.update(cx, |this, cx| this.set_action_kind(ix, choice, window, cx)).ok();
                                },
                            ));
                        }
                        menu
                    })
            };
            let field = match kind {
                ActionKind::Log | ActionKind::Command => Input::new(&action.text).small().into_any_element(),
                ActionKind::Sound => {
                    let sound = action.sound.clone();
                    let editor = editor.clone();
                    Button::new(("bp-sound", ix))
                        .label(sound.clone())
                        .small()
                        .outline()
                        .w(px(140.))
                        .dropdown_menu(move |mut menu, _, _| {
                            for name in SOUNDS {
                                let editor = editor.clone();
                                menu = menu.item(PopupMenuItem::new(name).checked(name == sound).on_click(move |_, _, cx| {
                                    editor
                                        .update(cx, |this, cx| {
                                            if let Some(row) = this.actions.get_mut(ix) {
                                                row.sound = name.to_string();
                                                // A preview, as Xcode plays it on choosing.
                                                breakpoints::play_sound(name);
                                            }
                                            cx.notify();
                                        })
                                        .ok();
                                }));
                            }
                            menu
                        })
                        .into_any_element()
                }
            };
            let buttons = h_flex()
                .flex_none()
                .child(
                    Button::new(("bp-action-add", ix))
                        .icon(IconName::Plus)
                        .xsmall()
                        .ghost()
                        .tooltip("Add an action after this one")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.add_action(ActionKind::Log, ix + 1, "", window, cx)
                        })),
                )
                .child(
                    Button::new(("bp-action-remove", ix))
                        .icon(IconName::Minus)
                        .xsmall()
                        .ghost()
                        .tooltip("Remove this action")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if ix < this.actions.len() {
                                this.actions.remove(ix);
                            }
                            cx.notify();
                        })),
                );
            let mut entry = v_flex()
                .gap_1()
                .child(h_flex().gap_1().child(picker).child(div().flex_1().min_w_0().child(field)).child(buttons));
            if kind == ActionKind::Log {
                let speak = action.speak;
                entry = entry.child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new(("bp-log-console", ix))
                                .label("Log message to console")
                                .xsmall()
                                .ghost()
                                .selected(!speak)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(row) = this.actions.get_mut(ix) {
                                        row.speak = false;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new(("bp-log-speak", ix))
                                .label("Speak message")
                                .xsmall()
                                .ghost()
                                .selected(speak)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(row) = this.actions.get_mut(ix) {
                                        row.speak = true;
                                    }
                                    cx.notify();
                                })),
                        ),
                );
            }
            list = list.child(entry);
        }
        v_flex().child(
            h_flex()
                .gap_3()
                .items_start()
                .child(
                    div()
                        .w(px(84.))
                        .flex_none()
                        .pt_1()
                        .text_right()
                        .text_color(theme.muted_foreground)
                        .child("Action"),
                )
                .child(div().flex_1().min_w_0().child(list)),
        )
    }
}

impl Render for BreakpointEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let has_condition = self.breakpoint.has_condition();
        let kind = self.render_kind(cx);
        let actions = self.render_actions(cx);
        let done_bounds = self.done_bounds.clone();
        v_flex()
            .id("breakpoint-editor")
            .key_context("BreakpointEditor")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .w(px(520.))
            .p_3()
            .gap_1()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .text_sm()
            .child(row("Name", Input::new(&self.name).small(), cx))
            .child(kind)
            .when(has_condition, |el| {
                el.child(row("Condition", Input::new(&self.condition).small(), cx)).child(row(
                    "Ignore",
                    h_flex()
                        .gap_2()
                        .child(div().w(px(64.)).child(Input::new(&self.ignore).small()))
                        .child(div().text_color(theme.muted_foreground).child("times before stopping")),
                    cx,
                ))
            })
            .child(actions)
            .child(row(
                "Options",
                Checkbox::new("bp-auto-continue")
                    .label("Automatically continue after evaluating actions")
                    .checked(self.auto_continue)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.auto_continue = *checked;
                        cx.notify();
                    })),
                cx,
            ))
            .child(
                h_flex().justify_end().pt_1().child(
                    div()
                        .relative()
                        .child(
                            Button::new("bp-done")
                                .label("Done")
                                .small()
                                .primary()
                                .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                        )
                        .child(canvas(move |bounds, _, _| done_bounds.set(Some(bounds)), |_, _, _, _| {}).absolute().top_0().left_0().size_full()),
                ),
            )
    }
}

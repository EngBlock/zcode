//! Xcode's scheme editor (Product ▸ Scheme ▸ Edit Scheme…, ⌘<) for the Run
//! and Test actions: Info (build configuration, debugger), Arguments
//! (arguments passed on launch, environment variables, each with its
//! checkbox), Options and Diagnostics. Run… (⌥⌘R) and Test… (⌥⌘U) open it
//! with a button that starts the action.
//!
//! Like Xcode's sheet it has no Cancel: closing it (Close, Escape, a click
//! outside) keeps the changes.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::Workspace;
use crate::actions::{PickerConfirm, PickerDismiss};
use crate::build::{BuildAction, BuildController};
use crate::project::ProjectKind;
use crate::scheme::{Argument, Diagnostics, Loaded, SchemeOptions, Source, Variable};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemeAction {
    Run,
    Test,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Info,
    Arguments,
    Options,
    Diagnostics,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Arguments => "Arguments",
            Self::Options => "Options",
            Self::Diagnostics => "Diagnostics",
        }
    }
}

struct ArgumentRow {
    enabled: bool,
    input: Entity<InputState>,
}

struct VariableRow {
    enabled: bool,
    key: Entity<InputState>,
    value: Entity<InputState>,
}

/// One action's argument and environment rows.
#[derive(Default)]
struct Lists {
    arguments: Vec<ArgumentRow>,
    environment: Vec<VariableRow>,
}

impl Lists {
    fn new(arguments: &[Argument], environment: &[Variable], window: &mut Window, cx: &mut App) -> Self {
        Self {
            arguments: arguments.iter().map(|a| argument_row(a.enabled, &a.value, window, cx)).collect(),
            environment: environment.iter().map(|v| variable_row(v.enabled, &v.key, &v.value, window, cx)).collect(),
        }
    }

    fn read(&self, cx: &App) -> (Vec<Argument>, Vec<Variable>) {
        let arguments = self
            .arguments
            .iter()
            .map(|row| Argument { value: row.input.read(cx).value().to_string(), enabled: row.enabled })
            .filter(|a| !a.value.trim().is_empty())
            .collect();
        let environment = self
            .environment
            .iter()
            .map(|row| Variable {
                key: row.key.read(cx).value().trim().to_string(),
                value: row.value.read(cx).value().to_string(),
                enabled: row.enabled,
            })
            .filter(|v| !v.key.is_empty())
            .collect();
        (arguments, environment)
    }
}

fn argument_row(enabled: bool, value: &str, window: &mut Window, cx: &mut App) -> ArgumentRow {
    let value = value.to_string();
    ArgumentRow { enabled, input: cx.new(|cx| InputState::new(window, cx).placeholder("Argument").default_value(value)) }
}

fn variable_row(enabled: bool, key: &str, value: &str, window: &mut Window, cx: &mut App) -> VariableRow {
    let (key, value) = (key.to_string(), value.to_string());
    VariableRow {
        enabled,
        key: cx.new(|cx| InputState::new(window, cx).placeholder("Name").default_value(key)),
        value: cx.new(|cx| InputState::new(window, cx).placeholder("Value").default_value(value)),
    }
}

pub struct EditSchemeDialog {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    scheme: String,
    configurations: Vec<String>,
    kind: ProjectKind,
    loaded: Loaded,
    test_plans: Vec<(String, bool)>,
    /// The options being edited, apart from the lists below.
    options: SchemeOptions,
    run_lists: Lists,
    test_lists: Lists,
    working_directory: Entity<InputState>,
    action: SchemeAction,
    tab: Tab,
    /// Opened by Run… / Test…: the button that closes the sheet also starts
    /// this action.
    perform: Option<SchemeAction>,
}

/// What the sheet shows about the project besides the scheme's options.
pub struct ProjectFacts {
    configurations: Vec<String>,
    kind: ProjectKind,
    /// The scheme's test plans, and whether each is its default.
    test_plans: Vec<(String, bool)>,
}

impl ProjectFacts {
    fn of(build: &BuildController) -> Self {
        Self {
            configurations: build.schemes.configurations.clone(),
            kind: build.project.kind.clone(),
            test_plans: build.test_plans.iter().map(|p| (p.name.clone(), p.is_default)).collect(),
        }
    }
}

impl EventEmitter<DismissEvent> for EditSchemeDialog {}

impl Focusable for EditSchemeDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EditSchemeDialog {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        project: ProjectFacts,
        loaded: Loaded,
        action: SchemeAction,
        perform: Option<SchemeAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let options = loaded.options.clone();
        let run_lists = Lists::new(&options.run.arguments, &options.run.environment, window, cx);
        let test_lists = Lists::new(&options.test.arguments, &options.test.environment, window, cx);
        let directory = options.custom_working_directory.clone();
        let working_directory =
            cx.new(|cx| InputState::new(window, cx).placeholder("$(PROJECT_DIR)").default_value(directory));
        let mut configurations = project.configurations;
        for configuration in [&options.run.configuration, &options.test.configuration] {
            if !configurations.contains(configuration) {
                configurations.push(configuration.clone());
            }
        }
        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            scheme: loaded.scheme.clone(),
            configurations,
            kind: project.kind,
            test_plans: project.test_plans,
            loaded,
            options,
            run_lists,
            test_lists,
            working_directory,
            action,
            tab: if perform.is_some() { Tab::Arguments } else { Tab::Info },
            perform,
        }
    }

    fn is_package(&self) -> bool {
        matches!(self.kind, ProjectKind::SwiftPackage)
    }

    /// The test plan, not the scheme, sets the test arguments and diagnostics.
    fn test_plan_owns_options(&self) -> bool {
        self.loaded.uses_test_plans
    }

    /// The options as edited.
    pub fn edited(&self, cx: &App) -> SchemeOptions {
        let mut options = self.options.clone();
        (options.run.arguments, options.run.environment) = self.run_lists.read(cx);
        (options.test.arguments, options.test.environment) = self.test_lists.read(cx);
        options.custom_working_directory = self.working_directory.read(cx).value().trim().to_string();
        options
    }

    fn lists_mut(&mut self) -> &mut Lists {
        match self.action {
            SchemeAction::Run => &mut self.run_lists,
            SchemeAction::Test => &mut self.test_lists,
        }
    }

    fn diagnostics_mut(&mut self) -> &mut Diagnostics {
        match self.action {
            SchemeAction::Run => &mut self.options.run.diagnostics,
            SchemeAction::Test => &mut self.options.test.diagnostics,
        }
    }

    fn close(&mut self, perform: Option<SchemeAction>, window: &mut Window, cx: &mut Context<Self>) {
        let options = self.edited(cx);
        self.workspace.update(cx, |ws, cx| ws.finish_edit_scheme(options, perform, window, cx)).ok();
    }

    fn confirm(&mut self, _: &PickerConfirm, window: &mut Window, cx: &mut Context<Self>) {
        self.close(self.perform, window, cx);
    }

    fn dismiss(&mut self, _: &PickerDismiss, window: &mut Window, cx: &mut Context<Self>) {
        self.close(None, window, cx);
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn set_action(&mut self, action: SchemeAction, cx: &mut Context<Self>) {
        self.action = action;
        if action == SchemeAction::Test && self.tab == Tab::Options {
            self.tab = Tab::Info;
        }
        cx.notify();
    }

    fn set_configuration(&mut self, configuration: String, cx: &mut Context<Self>) {
        match self.action {
            SchemeAction::Run => self.options.run.configuration = configuration,
            SchemeAction::Test => self.options.test.configuration = configuration,
        }
        cx.notify();
    }

    fn add_argument(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let row = argument_row(true, value, window, cx);
        row.input.update(cx, |input, cx| input.focus(window, cx));
        self.lists_mut().arguments.push(row);
        cx.notify();
    }

    fn add_variable(&mut self, key: &str, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let row = variable_row(true, key, value, window, cx);
        row.key.update(cx, |input, cx| input.focus(window, cx));
        self.lists_mut().environment.push(row);
        cx.notify();
    }

    /// Sets one Diagnostics checkbox. The Address and Thread Sanitizers
    /// exclude each other, as in Xcode.
    fn set_diagnostic(&mut self, name: &str, on: bool, cx: &mut Context<Self>) -> Result<(), String> {
        let d = self.diagnostics_mut();
        match name {
            "address_sanitizer" => {
                d.address_sanitizer = on;
                d.thread_sanitizer &= !on;
            }
            "thread_sanitizer" => {
                d.thread_sanitizer = on;
                d.address_sanitizer &= !on;
            }
            "undefined_behavior_sanitizer" => d.undefined_behavior_sanitizer = on,
            "main_thread_checker" => d.main_thread_checker = on,
            "malloc_scribble" => d.malloc_scribble = on,
            "malloc_guard_edges" => d.malloc_guard_edges = on,
            "zombie_objects" => d.zombie_objects = on,
            "malloc_stack_logging" => d.malloc_stack_logging = on,
            other => return Err(format!("unknown diagnostic {other}")),
        }
        cx.notify();
        Ok(())
    }

    /// Edits the sheet as its controls would, for the control channel:
    /// `action run|test`, `tab <name>`, `configuration <name>`,
    /// `argument <text>`, `variable KEY=VALUE`, `toggle-argument <text>`,
    /// `remove-argument <text>`, `toggle-variable KEY`, `remove-variable KEY`, `diagnostic <name> on|off`,
    /// `debug-executable`/`use-run-arguments`/`persistent-state`/`custom-directory on|off`,
    /// `working-directory <path>` (`close` and `perform` are the workspace's).
    pub fn set_field(&mut self, name: &str, value: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let on = || match value {
            "on" => Ok(true),
            "off" => Ok(false),
            _ => Err(format!("{name} takes on or off")),
        };
        match name {
            "action" => self.set_action(if value == "test" { SchemeAction::Test } else { SchemeAction::Run }, cx),
            "tab" => {
                let tab = [Tab::Info, Tab::Arguments, Tab::Options, Tab::Diagnostics]
                    .into_iter()
                    .find(|t| t.label().eq_ignore_ascii_case(value))
                    .ok_or(format!("no tab {value}"))?;
                self.set_tab(tab, cx);
            }
            "configuration" => self.set_configuration(value.to_string(), cx),
            "argument" => self.add_argument(value, window, cx),
            "variable" => {
                let (key, value) = value.split_once('=').ok_or("usage: variable KEY=VALUE")?;
                self.add_variable(key, value, window, cx);
            }
            "toggle-argument" | "remove-argument" | "toggle-variable" | "remove-variable" => {
                let lists = match self.action {
                    SchemeAction::Run => &mut self.run_lists,
                    SchemeAction::Test => &mut self.test_lists,
                };
                if name.ends_with("-argument") {
                    let ix = lists.arguments.iter().position(|r| r.input.read(cx).value() == value).ok_or(format!("no argument {value}"))?;
                    if name == "toggle-argument" {
                        lists.arguments[ix].enabled = !lists.arguments[ix].enabled;
                    } else {
                        lists.arguments.remove(ix);
                    }
                } else {
                    let ix = lists.environment.iter().position(|r| r.key.read(cx).value() == value).ok_or(format!("no variable {value}"))?;
                    if name == "toggle-variable" {
                        lists.environment[ix].enabled = !lists.environment[ix].enabled;
                    } else {
                        lists.environment.remove(ix);
                    }
                }
                cx.notify();
            }
            "diagnostic" => {
                let (diagnostic, state) = value.split_once(' ').ok_or("usage: diagnostic <name> on|off")?;
                self.set_diagnostic(diagnostic, state == "on", cx)?;
            }
            "debug-executable" => self.options.debug_executable = on()?,
            "use-run-arguments" => self.options.test_uses_run_arguments = on()?,
            "persistent-state" => self.options.ignore_persistent_state = on()?,
            "custom-directory" => self.options.use_custom_working_directory = on()?,
            "working-directory" => {
                let value = value.to_string();
                self.working_directory.update(cx, |input, cx| input.set_value(value, window, cx));
            }
            other => return Err(format!("unknown field {other}")),
        }
        cx.notify();
        Ok(())
    }

    pub fn summary(&self, cx: &App) -> serde_json::Value {
        let options = self.edited(cx);
        let state = |on: bool| if on { "on" } else { "off" };
        let (action, lists) = match self.action {
            SchemeAction::Run => (&options.run, (&options.run.arguments, &options.run.environment)),
            SchemeAction::Test => (&options.test, (&options.test.arguments, &options.test.environment)),
        };
        serde_json::json!({
            "scheme": self.scheme,
            "action": format!("{:?}", self.action),
            "tab": self.tab.label(),
            "storage": self.storage_label(),
            "configuration": action.configuration,
            "arguments": lists.0.iter().map(|a| format!("{} {}", state(a.enabled), a.value)).collect::<Vec<_>>(),
            "environment": lists.1.iter().map(|v| format!("{} {}={}", state(v.enabled), v.key, v.value)).collect::<Vec<_>>(),
            "test_plan_owns_options": self.test_plan_owns_options(),
            "perform": self.perform.map(|p| format!("{p:?}")),
        })
    }

    fn storage_label(&self) -> String {
        match &self.loaded.source {
            Source::File(path) => {
                let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if self.loaded.is_shared() { format!("Shared scheme · {file}") } else { format!("User scheme · {file}") }
            }
            Source::Session if self.is_package() => "Swift package · options kept by zcode".to_string(),
            Source::Session => "No scheme file · options kept by zcode".to_string(),
        }
    }
}

impl Workspace {
    /// Product ▸ Scheme ▸ Edit Scheme… (⌘<), or with `perform` Run… (⌥⌘R)
    /// and Test… (⌥⌘U).
    pub(super) fn edit_scheme(
        &mut self,
        action: SchemeAction,
        perform: Option<SchemeAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let build = self.build.clone();
        if !build.read(cx).can_build() {
            return;
        }
        let loaded = build.update(cx, |b, _| b.scheme.clone().map(|s| b.fresh_scheme_options(&s)));
        let Some(loaded) = loaded else {
            let message = if build.read(cx).schemes_loading { "Schemes are still loading." } else { "There is no scheme to edit." };
            self.notify_error(message.into(), window, cx);
            return;
        };
        let workspace = cx.weak_entity();
        let facts = ProjectFacts::of(build.read(cx));
        let dialog = cx.new(|cx| EditSchemeDialog::new(workspace, facts, loaded, action, perform, window, cx));
        self.edit_scheme_dialog = Some(dialog.downgrade());
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn finish_edit_scheme(
        &mut self,
        options: SchemeOptions,
        perform: Option<SchemeAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_scheme_dialog = None;
        self.dismiss_modal(window, cx);
        self.save_edited_scheme(options, window, cx);
        match perform {
            Some(SchemeAction::Run) => self.start_build(BuildAction::Run, window, cx),
            Some(SchemeAction::Test) => self.test_all(window, cx),
            None => {}
        }
    }

    pub(super) fn save_edited_scheme(&mut self, options: SchemeOptions, window: &mut Window, cx: &mut Context<Self>) {
        let unchanged = self.build.read(cx).scheme_options.as_ref().is_some_and(|l| l.options == options);
        if unchanged {
            return;
        }
        if let Err(err) = self.build.update(cx, |b, cx| b.save_scheme_options(options, cx)) {
            self.notify_error(format!("Couldn’t save the scheme: {err:#}"), window, cx);
        }
    }

    /// The control channel's `edit-scheme <field> <value>`.
    pub fn edit_scheme_field(&mut self, field: &str, value: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let dialog = self.edit_scheme_dialog.as_ref().and_then(|d| d.upgrade()).ok_or("the scheme editor isn’t open")?;
        match field {
            // The Close button, and Run… / Test…'s Run or Test button.
            "close" | "perform" => {
                let (options, perform) = {
                    let dialog = dialog.read(cx);
                    (dialog.edited(cx), dialog.perform.filter(|_| field == "perform"))
                };
                self.finish_edit_scheme(options, perform, window, cx);
                Ok(())
            }
            _ => dialog.update(cx, |d, cx| d.set_field(field, value, window, cx)),
        }
    }
}

/// The current scheme's options, for the control channel's `dump`.
pub fn options_summary(loaded: &Loaded) -> serde_json::Value {
    let o = &loaded.options;
    let enabled = |on: bool| if on { "" } else { "(off) " };
    let list = |a: &[Argument], v: &[Variable]| {
        serde_json::json!({
            "arguments": a.iter().map(|a| format!("{}{}", enabled(a.enabled), a.value)).collect::<Vec<_>>(),
            "environment": v.iter().map(|v| format!("{}{}={}", enabled(v.enabled), v.key, v.value)).collect::<Vec<_>>(),
        })
    };
    let diagnostics = |d: &Diagnostics| {
        [
            ("address_sanitizer", d.address_sanitizer),
            ("thread_sanitizer", d.thread_sanitizer),
            ("undefined_behavior_sanitizer", d.undefined_behavior_sanitizer),
            ("main_thread_checker", d.main_thread_checker),
            ("malloc_scribble", d.malloc_scribble),
            ("malloc_guard_edges", d.malloc_guard_edges),
            ("zombie_objects", d.zombie_objects),
            ("malloc_stack_logging", d.malloc_stack_logging),
        ]
        .into_iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| name)
        .collect::<Vec<_>>()
    };
    serde_json::json!({
        "scheme": loaded.scheme,
        "storage": match &loaded.source {
            Source::File(path) => path.display().to_string(),
            Source::Session => "session".to_string(),
        },
        "run": {
            "configuration": o.run.configuration,
            "lists": list(&o.run.arguments, &o.run.environment),
            "diagnostics": diagnostics(&o.run.diagnostics),
            "debug_executable": o.debug_executable,
        },
        "test": {
            "configuration": o.test.configuration,
            "uses_run_arguments": o.test_uses_run_arguments,
            "lists": list(&o.test.arguments, &o.test.environment),
            "diagnostics": diagnostics(&o.test.diagnostics),
        },
        "uses_test_plans": loaded.uses_test_plans,
    })
}

fn section(label: &'static str, cx: &App) -> Div {
    div()
        .pt_3()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(label)
}

fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    div().pt_1().text_xs().text_color(cx.theme().muted_foreground).child(text.into())
}

fn row(label: &'static str, content: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .gap_3()
        .py_0p5()
        .child(div().w(px(140.)).flex_none().text_right().text_color(cx.theme().muted_foreground).child(label))
        .child(div().flex_1().min_w_0().child(content))
}

impl EditSchemeDialog {
    fn render_info(&mut self, cx: &mut Context<Self>) -> Div {
        let configuration = match self.action {
            SchemeAction::Run => self.options.run.configuration.clone(),
            SchemeAction::Test => self.options.test.configuration.clone(),
        };
        let dialog = cx.entity().downgrade();
        let configurations = self.configurations.clone();
        let picker = Button::new("es-configuration")
            .label(configuration.clone())
            .small()
            .outline()
            .w(px(200.))
            .dropdown_menu(move |mut menu, _, _| {
                for name in configurations.clone() {
                    let dialog = dialog.clone();
                    let value = name.clone();
                    menu = menu.item(PopupMenuItem::new(name.clone()).checked(name == configuration).on_click(
                        move |_, _, cx| {
                            dialog.update(cx, |d, cx| d.set_configuration(value.clone(), cx)).ok();
                        },
                    ));
                }
                menu
            });
        let mut body = v_flex().gap_1().child(row("Build Configuration", picker, cx));
        match self.action {
            SchemeAction::Run => {
                let executable = self.loaded.executable.clone().unwrap_or_else(|| "None".into());
                body = body.child(row("Executable", div().child(executable), cx)).child(row(
                    "Debugger",
                    Checkbox::new("es-debug-executable")
                        .label("Debug executable")
                        .checked(self.options.debug_executable)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.options.debug_executable = *checked;
                            cx.notify();
                        })),
                    cx,
                ));
                body = body.child(note(
                    "With breakpoints, Run starts under the debugger (lldb-dap) unless Debug executable is off.",
                    cx,
                ));
            }
            SchemeAction::Test => {
                if self.test_plans.is_empty() {
                    body = body.child(row(
                        "Tests",
                        div().child(if self.is_package() { "The package’s test targets" } else { "The scheme’s test targets" }),
                        cx,
                    ));
                } else {
                    let plans = v_flex().children(self.test_plans.iter().map(|(name, is_default)| {
                        div().child(if *is_default { format!("{name} (Default)") } else { name.clone() })
                    }));
                    body = body.child(row("Test Plans", plans, cx));
                }
            }
        }
        body
    }

    fn render_lists(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let mut body = v_flex().gap_1();
        let testing = self.action == SchemeAction::Test;
        if testing && self.test_plan_owns_options() {
            return body.child(note(
                "This scheme runs test plans: their configurations set the arguments, environment variables and diagnostics tests run with.",
                cx,
            ));
        }
        if testing {
            body = body.child(
                Checkbox::new("es-use-run")
                    .label("Use the Run action’s arguments and environment variables")
                    .checked(self.options.test_uses_run_arguments)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.options.test_uses_run_arguments = *checked;
                        cx.notify();
                    })),
            );
            if self.options.test_uses_run_arguments {
                return body;
            }
        }
        let arguments_unused = testing && self.is_package();
        let lists = if testing { &self.test_lists } else { &self.run_lists };
        body = body.child(section("ARGUMENTS PASSED ON LAUNCH", cx));
        for (ix, argument) in lists.arguments.iter().enumerate() {
            body = body.child(
                h_flex()
                    .gap_2()
                    .child(Checkbox::new(("es-arg-on", ix)).checked(argument.enabled).on_click(cx.listener(
                        move |this, checked: &bool, _, cx| {
                            this.lists_mut().arguments[ix].enabled = *checked;
                            cx.notify();
                        },
                    )))
                    .child(div().flex_1().min_w_0().child(Input::new(&argument.input).small()))
                    .child(
                        Button::new(("es-arg-remove", ix)).icon(IconName::X).xsmall().ghost().tooltip("Remove").on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.lists_mut().arguments.remove(ix);
                                cx.notify();
                            }),
                        ),
                    ),
            );
        }
        body = body.child(
            h_flex().child(
                Button::new("es-arg-add")
                    .icon(IconName::Plus)
                    .label("Add Argument")
                    .xsmall()
                    .ghost()
                    .disabled(arguments_unused)
                    .on_click(cx.listener(|this, _, window, cx| this.add_argument("", window, cx))),
            ),
        );
        if arguments_unused {
            body = body.child(note("swift test doesn’t pass launch arguments to tests.", cx));
        }
        body = body.child(section("ENVIRONMENT VARIABLES", cx));
        if !lists.environment.is_empty() {
            body = body.child(
                h_flex()
                    .gap_2()
                    .pl(px(24.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().w(px(200.)).child("Name"))
                    .child(div().flex_1().child("Value")),
            );
        }
        for (ix, variable) in lists.environment.iter().enumerate() {
            body = body.child(
                h_flex()
                    .gap_2()
                    .child(Checkbox::new(("es-env-on", ix)).checked(variable.enabled).on_click(cx.listener(
                        move |this, checked: &bool, _, cx| {
                            this.lists_mut().environment[ix].enabled = *checked;
                            cx.notify();
                        },
                    )))
                    .child(div().w(px(200.)).flex_none().child(Input::new(&variable.key).small()))
                    .child(div().flex_1().min_w_0().child(Input::new(&variable.value).small()))
                    .child(
                        Button::new(("es-env-remove", ix)).icon(IconName::X).xsmall().ghost().tooltip("Remove").on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.lists_mut().environment.remove(ix);
                                cx.notify();
                            }),
                        ),
                    ),
            );
        }
        body.child(
            h_flex().child(
                Button::new("es-env-add")
                    .icon(IconName::Plus)
                    .label("Add Variable")
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| this.add_variable("", "", window, cx))),
            ),
        )
        .child(note("$(BUILD_SETTING) references expand when an Xcode project’s app launches.", cx))
    }

    fn render_options(&mut self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .gap_1()
            .child(section("WORKING DIRECTORY", cx))
            .child(
                Checkbox::new("es-custom-directory")
                    .label("Use custom working directory")
                    .checked(self.options.use_custom_working_directory)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.options.use_custom_working_directory = *checked;
                        cx.notify();
                    })),
            )
            .child(div().pl(px(24.)).child(Input::new(&self.working_directory).small().disabled(!self.options.use_custom_working_directory)))
            .child(note("For Mac executables and Swift packages; simulator apps start in their container.", cx))
            .child(section("PERSISTENT STATE", cx))
            .child(
                Checkbox::new("es-persistent-state")
                    .label("Launch app without state restoration")
                    .checked(self.options.ignore_persistent_state)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.options.ignore_persistent_state = *checked;
                        cx.notify();
                    })),
            )
    }

    fn render_diagnostics(&mut self, cx: &mut Context<Self>) -> Div {
        if self.action == SchemeAction::Test && self.test_plan_owns_options() {
            return v_flex().child(note(
                "This scheme runs test plans: their configurations set the diagnostics tests run with.",
                cx,
            ));
        }
        let d = match self.action {
            SchemeAction::Run => self.options.run.diagnostics.clone(),
            SchemeAction::Test => self.options.test.diagnostics.clone(),
        };
        let check = |id: &'static str, label: &'static str, checked: bool, cx: &mut Context<Self>| {
            Checkbox::new(id).label(label).checked(checked).on_click(cx.listener(move |this, checked: &bool, _, cx| {
                this.set_diagnostic(id, *checked, cx).ok();
            }))
        };
        let mut body = v_flex()
            .gap_1()
            .child(section("RUNTIME SANITIZATION", cx))
            .child(check("address_sanitizer", "Address Sanitizer", d.address_sanitizer, cx))
            .child(check("thread_sanitizer", "Thread Sanitizer", d.thread_sanitizer, cx))
            .child(check("undefined_behavior_sanitizer", "Undefined Behavior Sanitizer", d.undefined_behavior_sanitizer, cx))
            .child(note("Sanitizers apply when building (xcodebuild -enable…Sanitizer, swift --sanitize): turning one on or off rebuilds.", cx));
        if !(self.action == SchemeAction::Test && self.is_package()) {
            body = body
                .child(section("RUNTIME API CHECKING", cx))
                .child(check("main_thread_checker", "Main Thread Checker", d.main_thread_checker, cx));
            if self.action == SchemeAction::Run {
                body = body.child(note("Applies when the app runs under the debugger.", cx));
            }
        }
        body.child(section("MEMORY MANAGEMENT", cx))
            .child(check("malloc_scribble", "Malloc Scribble", d.malloc_scribble, cx))
            .child(check("malloc_guard_edges", "Malloc Guard Edges", d.malloc_guard_edges, cx))
            .child(check("zombie_objects", "Zombie Objects", d.zombie_objects, cx))
            .child(check("malloc_stack_logging", "Malloc Stack Logging", d.malloc_stack_logging, cx))
    }
}

impl Render for EditSchemeDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tabs: &[Tab] = match self.action {
            SchemeAction::Run => &[Tab::Info, Tab::Arguments, Tab::Options, Tab::Diagnostics],
            SchemeAction::Test => &[Tab::Info, Tab::Arguments, Tab::Diagnostics],
        };
        let content = match self.tab {
            Tab::Info => self.render_info(cx),
            Tab::Arguments => self.render_lists(cx),
            Tab::Options => self.render_options(cx),
            Tab::Diagnostics => self.render_diagnostics(cx),
        };
        let action_item = |action: SchemeAction, cx: &mut Context<Self>| {
            let (label, icon, configuration) = match action {
                SchemeAction::Run => ("Run", IconName::Play, self.options.run.configuration.clone()),
                SchemeAction::Test => ("Test", IconName::Diamond, self.options.test.configuration.clone()),
            };
            let selected = self.action == action;
            h_flex()
                .id(label)
                .gap_2()
                .px_2()
                .py_1()
                .rounded(theme.radius)
                .when(selected, |el| el.bg(theme.accent).text_color(theme.accent_foreground))
                .hover(|el| el.bg(theme.accent))
                .on_click(cx.listener(move |this, _, _, cx| this.set_action(action, cx)))
                .child(Icon::new(icon).small())
                .child(v_flex().child(label).child(div().text_xs().text_color(theme.muted_foreground).child(configuration)))
        };
        let sidebar = v_flex()
            .w(px(150.))
            .flex_none()
            .p_2()
            .gap_1()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(action_item(SchemeAction::Run, cx))
            .child(action_item(SchemeAction::Test, cx));
        let tab_bar = h_flex()
            .justify_center()
            .gap_1()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .children(tabs.iter().map(|&tab| {
                Button::new(tab.label())
                    .label(tab.label())
                    .xsmall()
                    .ghost()
                    .selected(self.tab == tab)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
            }));
        let perform = self.perform;
        let mut buttons = h_flex().gap_2().child(
            Button::new("es-close")
                .label("Close")
                .small()
                .when(perform.is_some(), |b| b.ghost())
                .when(perform.is_none(), |b| b.primary())
                .on_click(cx.listener(|this, _, window, cx| this.close(None, window, cx))),
        );
        if let Some(action) = perform {
            buttons = buttons.child(
                Button::new("es-perform")
                    .label(match action {
                        SchemeAction::Run => "Run",
                        SchemeAction::Test => "Test",
                    })
                    .small()
                    .primary()
                    .on_click(cx.listener(move |this, _, window, cx| this.close(Some(action), window, cx))),
            );
        }
        let error = self.loaded.error.clone();

        v_flex()
            .id("edit-scheme")
            .key_context("EditSchemeDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .w(px(760.))
            .h(px(500.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .overflow_hidden()
            .text_sm()
            .child(
                div()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Edit Scheme “{}”", self.scheme)),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(sidebar)
                    .child(
                        v_flex().flex_1().min_w_0().child(tab_bar).child(
                            div()
                                .id("es-content")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .px_4()
                                .py_2()
                                .when_some(error, |el, error| {
                                    el.child(div().pb_2().text_color(theme.danger).child(format!("Couldn’t read the scheme: {error}")))
                                })
                                .child(content),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(div().text_xs().text_color(theme.muted_foreground).child(self.storage_label()))
                    .child(buttons),
            )
    }
}

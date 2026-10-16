//! Breakpoints as Xcode has them: file and line, symbolic, Swift error and
//! exception breakpoints, each with a condition, an ignore count, actions
//! and "Automatically continue after evaluating actions". They are saved per
//! project in the session.
//!
//! lldb-dap gets line breakpoints through `setBreakpoints` (condition and
//! hit condition included) and Swift error and exception breakpoints as
//! exception filters. Symbolic breakpoints are lldb commands, because DAP
//! function breakpoints can't name a module. Actions run in zcode when a
//! breakpoint stops, so a log message can use Xcode's `@expression@`, `%B`
//! and `%H` and auto-continuing breakpoints never show the stop.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What a breakpoint stops on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    /// A file and 1-based line.
    Line { path: PathBuf, line: u32 },
    /// A function or method name, optionally limited to one module (image).
    Symbolic { symbol: String, module: String },
    /// Any Swift error thrown.
    SwiftError,
    Exception { language: ExceptionLanguage, on: ExceptionPhase },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExceptionLanguage {
    #[default]
    All,
    ObjC,
    Cpp,
}

impl ExceptionLanguage {
    pub const ALL: [Self; 3] = [Self::All, Self::ObjC, Self::Cpp];

    /// Xcode's names in the Exception popup.
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::ObjC => "Objective-C",
            Self::Cpp => "C++",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExceptionPhase {
    #[default]
    Throw,
    Catch,
}

impl ExceptionPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Throw => "On Throw",
            Self::Catch => "On Catch",
        }
    }
}

/// lldb-dap's exception filters and the labels its stops report.
const FILTERS: [(&str, &str); 6] = [
    ("swift_throw", "Swift Throw"),
    ("swift_catch", "Swift Catch"),
    ("objc_throw", "Objective-C Throw"),
    ("objc_catch", "Objective-C Catch"),
    ("cpp_throw", "C++ Throw"),
    ("cpp_catch", "C++ Catch"),
];

/// The exception filter a stop's description names ("Swift Throw").
pub fn filter_for_stop(description: &str) -> Option<&'static str> {
    FILTERS.iter().find(|(_, label)| *label == description).map(|(filter, _)| *filter)
}

/// The macOS system sounds Xcode's Sound action offers.
pub const SOUNDS: [&str; 14] = [
    "Basso", "Blow", "Bottle", "Frog", "Funk", "Glass", "Hero", "Morse", "Ping", "Pop", "Purr", "Sosumi", "Submarine", "Tink",
];

/// Something a breakpoint does when it stops.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Printed to the console (or spoken), with `@expression@`, `%B` and `%H`.
    Log {
        message: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        speak: bool,
    },
    /// An lldb command; its output goes to the console.
    Command { command: String },
    /// A system sound.
    Sound { name: String },
}

impl Action {
    /// Xcode's names in the action popup.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Log { .. } => "Log Message",
            Self::Command { .. } => "Debugger Command",
            Self::Sound { .. } => "Sound",
        }
    }
}

fn enabled_default() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Breakpoint {
    /// Stable for the life of the window; not saved.
    #[serde(skip)]
    pub id: u64,
    pub kind: Kind,
    #[serde(default = "enabled_default", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub condition: String,
    /// "Ignore N times before stopping".
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ignore_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    /// "Automatically continue after evaluating actions".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_continue: bool,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

impl Breakpoint {
    pub fn new(kind: Kind) -> Self {
        Self {
            id: 0,
            kind,
            enabled: true,
            name: String::new(),
            condition: String::new(),
            ignore_count: 0,
            actions: vec![],
            auto_continue: false,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match &self.kind {
            Kind::Line { path, .. } => Some(path),
            _ => None,
        }
    }

    pub fn line(&self) -> Option<u32> {
        match &self.kind {
            Kind::Line { line, .. } => Some(*line),
            _ => None,
        }
    }

    pub fn is_at(&self, at: &Path, at_line: u32) -> bool {
        matches!(&self.kind, Kind::Line { path, line } if path == at && *line == at_line)
    }

    /// Whether the breakpoint has a condition and an ignore count (Swift
    /// error and exception breakpoints have neither, as in Xcode).
    pub fn has_condition(&self) -> bool {
        matches!(self.kind, Kind::Line { .. } | Kind::Symbolic { .. })
    }

    /// How the Breakpoint navigator names it.
    pub fn title(&self) -> String {
        match &self.kind {
            Kind::Line { path, line } => {
                format!("{}:{line}", path.file_name().unwrap_or_default().to_string_lossy())
            }
            Kind::Symbolic { symbol, .. } if symbol.is_empty() => "Symbolic Breakpoint".into(),
            Kind::Symbolic { symbol, .. } => symbol.clone(),
            Kind::SwiftError => "Swift Error".into(),
            Kind::Exception { language, on } => {
                let what = match language {
                    ExceptionLanguage::All => "All Exceptions",
                    ExceptionLanguage::ObjC => "All Objective-C Exceptions",
                    ExceptionLanguage::Cpp => "All C++ Exceptions",
                };
                match on {
                    ExceptionPhase::Throw => what.into(),
                    ExceptionPhase::Catch => format!("{what} (catch)"),
                }
            }
        }
    }

    /// `%B` in a log message: the name, or what the breakpoint is on.
    pub fn display_name(&self) -> String {
        if self.name.is_empty() { self.title() } else { self.name.clone() }
    }

    /// lldb-dap's exception filters for a Swift error or exception breakpoint.
    pub fn exception_filters(&self) -> &'static [&'static str] {
        match &self.kind {
            Kind::SwiftError => &["swift_throw"],
            Kind::Exception { language, on } => match (language, on) {
                (ExceptionLanguage::All, ExceptionPhase::Throw) => &["objc_throw", "cpp_throw"],
                (ExceptionLanguage::All, ExceptionPhase::Catch) => &["objc_catch", "cpp_catch"],
                (ExceptionLanguage::ObjC, ExceptionPhase::Throw) => &["objc_throw"],
                (ExceptionLanguage::ObjC, ExceptionPhase::Catch) => &["objc_catch"],
                (ExceptionLanguage::Cpp, ExceptionPhase::Throw) => &["cpp_throw"],
                (ExceptionLanguage::Cpp, ExceptionPhase::Catch) => &["cpp_catch"],
            },
            _ => &[],
        }
    }

    /// A line breakpoint in a `setBreakpoints` request. lldb's hit condition
    /// `N` stops on the Nth hit, so ignoring N hits is `N + 1`.
    pub fn source_breakpoint(&self) -> Option<Value> {
        let line = self.line()?;
        let mut value = json!({ "line": line });
        if !self.condition.trim().is_empty() {
            value["condition"] = self.condition.trim().into();
        }
        if self.ignore_count > 0 {
            value["hitCondition"] = (self.ignore_count + 1).to_string().into();
        }
        Some(value)
    }

    /// The lldb breakpoint name zcode gives the breakpoints it creates by
    /// command, so they can be deleted without knowing lldb's ids.
    pub fn lldb_name(id: u64) -> String {
        format!("zcode_{id}")
    }

    /// The lldb command that creates a symbolic breakpoint.
    pub fn symbolic_command(&self) -> Option<String> {
        let Kind::Symbolic { symbol, module } = &self.kind else { return None };
        let symbol = symbol.trim();
        if symbol.is_empty() {
            return None;
        }
        let mut command = format!("breakpoint set --name {}", quote(symbol));
        if !module.trim().is_empty() {
            command += &format!(" --shlib {}", quote(module.trim()));
        }
        if !self.condition.trim().is_empty() {
            command += &format!(" --condition {}", quote(self.condition.trim()));
        }
        if self.ignore_count > 0 {
            command += &format!(" --ignore-count {}", self.ignore_count);
        }
        command += &format!(" -N {}", Self::lldb_name(self.id));
        Some(command)
    }
}

/// Quotes an argument for lldb's command line.
fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for ch in text.chars() {
        if matches!(ch, '"' | '\\' | '`') {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    quoted
}

/// lldb's id from `breakpoint set` output ("Breakpoint 4: where = …").
pub fn created_id(output: &str) -> Option<i64> {
    let rest = output.trim_start().strip_prefix("Breakpoint ")?;
    rest[..rest.find(':')?].trim().parse().ok()
}

/// Plays a system sound (the Sound action) without waiting for it.
pub fn play_sound(name: &str) {
    run_detached("/usr/bin/afplay", format!("/System/Library/Sounds/{name}.aiff"));
}

/// Speaks a log message (Log Message's "Speak message").
pub fn speak(text: String) {
    run_detached("/usr/bin/say", text);
}

fn run_detached(tool: &'static str, argument: String) {
    std::thread::spawn(move || std::process::Command::new(tool).arg(argument).status().ok());
}

/// A piece of a log message.
#[derive(Clone, Debug, PartialEq)]
pub enum LogPart {
    Text(String),
    Expression(String),
}

/// Splits an Xcode log message into text and `@expression@`s, replacing
/// `%B` with the breakpoint's name and `%H` with its hit count. An `@`
/// without a closing one is kept as text.
pub fn parse_log_message(message: &str, name: &str, hits: u32) -> Vec<LogPart> {
    let mut parts = vec![];
    let mut text = String::new();
    let mut rest = message;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('@')
            && let Some(end) = after.find('@')
        {
            if !text.is_empty() {
                parts.push(LogPart::Text(std::mem::take(&mut text)));
            }
            let expression = after[..end].trim();
            if !expression.is_empty() {
                parts.push(LogPart::Expression(expression.to_string()));
            }
            rest = &after[end + 1..];
        } else if let Some(after) = rest.strip_prefix("%B") {
            text.push_str(name);
            rest = after;
        } else if let Some(after) = rest.strip_prefix("%H") {
            text.push_str(&hits.to_string());
            rest = after;
        } else {
            let ch = rest.chars().next().unwrap_or_default();
            text.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    if !text.is_empty() {
        parts.push(LogPart::Text(text));
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_messages_interpolate_expressions_and_tokens() {
        assert_eq!(
            parse_log_message("tick @i * 10@ in %B, hit %H", "main.swift:13", 2),
            vec![
                LogPart::Text("tick ".into()),
                LogPart::Expression("i * 10".into()),
                LogPart::Text(" in main.swift:13, hit 2".into()),
            ]
        );
        assert_eq!(parse_log_message("mail me@example.com", "b", 1), vec![LogPart::Text("mail me@example.com".into())]);
        assert_eq!(
            parse_log_message("@a@@b@ ünïcode", "b", 1),
            vec![LogPart::Expression("a".into()), LogPart::Expression("b".into()), LogPart::Text(" ünïcode".into())]
        );
        assert_eq!(parse_log_message("", "b", 1), vec![]);
    }

    #[test]
    fn line_breakpoints_carry_condition_and_ignore_count() {
        let mut bp = Breakpoint::new(Kind::Line { path: "/a.swift".into(), line: 7 });
        assert_eq!(bp.source_breakpoint(), Some(json!({ "line": 7 })));
        bp.condition = " i == 2 ".into();
        bp.ignore_count = 2;
        assert_eq!(bp.source_breakpoint(), Some(json!({ "line": 7, "condition": "i == 2", "hitCondition": "3" })));
    }

    #[test]
    fn symbolic_breakpoints_are_lldb_commands() {
        let mut bp = Breakpoint::new(Kind::Symbolic { symbol: "Greeter.greeting".into(), module: String::new() });
        bp.id = 4;
        assert_eq!(bp.symbolic_command().unwrap(), r#"breakpoint set --name "Greeter.greeting" -N zcode_4"#);
        bp.kind = Kind::Symbolic { symbol: "-[NSView layout]".into(), module: "AppKit".into() };
        bp.condition = r#"name == "a\b""#.into();
        bp.ignore_count = 1;
        assert_eq!(
            bp.symbolic_command().unwrap(),
            r#"breakpoint set --name "-[NSView layout]" --shlib "AppKit" --condition "name == \"a\\b\"" --ignore-count 1 -N zcode_4"#
        );
        bp.kind = Kind::Symbolic { symbol: " ".into(), module: String::new() };
        assert_eq!(bp.symbolic_command(), None);
        assert_eq!(created_id("Breakpoint 12: where = zhello`main + 4"), Some(12));
        assert_eq!(created_id("Breakpoint 4: no locations (pending)."), Some(4));
        assert_eq!(created_id("error: bad"), None);
    }

    #[test]
    fn exception_breakpoints_map_to_filters() {
        let bp = Breakpoint::new(Kind::SwiftError);
        assert_eq!(bp.exception_filters(), ["swift_throw"]);
        assert_eq!(bp.title(), "Swift Error");
        let bp = Breakpoint::new(Kind::Exception { language: ExceptionLanguage::All, on: ExceptionPhase::Throw });
        assert_eq!(bp.exception_filters(), ["objc_throw", "cpp_throw"]);
        assert_eq!(bp.title(), "All Exceptions");
        let bp = Breakpoint::new(Kind::Exception { language: ExceptionLanguage::ObjC, on: ExceptionPhase::Catch });
        assert_eq!(bp.exception_filters(), ["objc_catch"]);
        assert_eq!(filter_for_stop("Swift Throw"), Some("swift_throw"));
        assert_eq!(filter_for_stop("breakpoint 1.1"), None);
    }

    #[test]
    fn saved_breakpoints_round_trip_and_omit_defaults() {
        let mut bp = Breakpoint::new(Kind::Line { path: "/a.swift".into(), line: 3 });
        assert_eq!(serde_json::to_value(&bp).unwrap(), json!({ "kind": { "type": "line", "path": "/a.swift", "line": 3 } }));
        bp.enabled = false;
        bp.auto_continue = true;
        bp.actions = vec![
            Action::Log { message: "@i@".into(), speak: false },
            Action::Command { command: "bt".into() },
            Action::Sound { name: "Pop".into() },
        ];
        let saved = serde_json::to_string(&bp).unwrap();
        assert_eq!(serde_json::from_str::<Breakpoint>(&saved).unwrap(), bp);
        let exception = Breakpoint::new(Kind::Exception { language: ExceptionLanguage::Cpp, on: ExceptionPhase::Catch });
        let saved = serde_json::to_string(&exception).unwrap();
        assert_eq!(serde_json::from_str::<Breakpoint>(&saved).unwrap(), exception);
    }
}

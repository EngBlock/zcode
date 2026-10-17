//! What a settings file may say (`SettingsContent`, every value optional)
//! and the settings that result (`Settings`), defaults first, then the
//! user's file, then the project's. Names and shapes are Zed's where Zed has
//! the setting (`buffer_font_size`, `soft_wrap`, `inlay_hints.enabled`,
//! `git.inline_blame.enabled`, `terminal.shell`, `edit_predictions`);
//! `default.json` documents them.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// The commented defaults, shown by zcode ▸ Open Default Settings.
pub const DEFAULT_SETTINGS: &str = include_str!("default.json");

/// The settings in effect for a window.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub theme: ThemeSelection,
    /// `None`: the theme's monospace font.
    pub buffer_font_family: Option<String>,
    pub buffer_font_size: f32,
    pub tab_size: u32,
    pub hard_tabs: bool,
    pub soft_wrap: bool,
    pub show_whitespaces: bool,
    pub line_numbers: bool,
    pub indent_guides: bool,
    pub format_on_save: bool,
    pub remove_trailing_whitespace_on_save: bool,
    pub ensure_final_newline_on_save: bool,
    pub inlay_hints: bool,
    pub vim_mode: bool,
    pub inline_blame: bool,
    pub show_authors: bool,
    pub side_by_side_comparison: bool,
    /// A program and its arguments; `None` is the login shell.
    pub terminal_shell: Option<(String, Vec<String>)>,
    /// The agents the agent panel can run (`crate::acp::agents`).
    pub agent_servers: Vec<crate::acp::AgentServer>,
    /// The agent a new panel starts with, by id.
    pub default_agent: String,
    /// Answer agents' permission requests with Allow without asking.
    pub always_allow_tool_actions: bool,
    /// Hand agents in the panel zcode's own MCP server (`crate::mcp`).
    pub zcode_mcp_server: bool,
    pub edit_predictions: EditPredictionSettings,
    /// Product ▸ Profile stops recording after this many seconds (`None`:
    /// at Stop).
    pub profiling_time_limit: Option<u32>,
    /// Distribute App notarizes Developer ID apps with this keychain
    /// profile (`xcrun notarytool store-credentials <name>`).
    pub notary_keychain_profile: String,
}

/// Predictive code completion (`crate::predictions`).
#[derive(Clone, Debug, PartialEq)]
pub struct EditPredictionSettings {
    pub provider: PredictionProvider,
    /// Ask while typing pauses; otherwise only on Show Predictive Completion.
    pub automatic: bool,
    pub debounce_ms: u64,
    /// Files never sent to a provider.
    pub disabled_globs: Vec<String>,
    pub claude: ClaudeSettings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PredictionProvider {
    #[default]
    None,
    /// The Claude API (`crate::predictions::claude`).
    Claude,
    /// The language server's `textDocument/inlineCompletion`.
    Lsp,
}

impl PredictionProvider {
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Claude => "claude",
            Self::Lsp => "lsp",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeSettings {
    pub model: String,
    /// `None`: `ANTHROPIC_API_KEY` from the environment or the login shell.
    pub api_key: Option<Secret>,
    pub api_url: String,
    pub max_tokens: u32,
    /// `output_config.effort`; `None` leaves it to the model.
    pub effort: Option<String>,
}

/// A value that never shows up in logs or `{:?}`.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

impl Default for EditPredictionSettings {
    fn default() -> Self {
        Self {
            provider: PredictionProvider::None,
            automatic: true,
            debounce_ms: 300,
            disabled_globs: ["**/.env*", "**/*.pem", "**/*.key", "**/*.cert", "**/*.crt", "**/secrets.yml", "**/*.p12", "**/*.mobileprovision"]
                .map(String::from)
                .to_vec(),
            claude: ClaudeSettings {
                model: "claude-haiku-5-5".into(),
                api_key: None,
                api_url: "https://api.anthropic.com".into(),
                max_tokens: 256,
                effort: Some("low".into()),
            },
        }
    }
}

pub const DEFAULT_FONT_SIZE: f32 = 13.;

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeSelection::default(),
            buffer_font_family: None,
            buffer_font_size: DEFAULT_FONT_SIZE,
            tab_size: 4,
            hard_tabs: false,
            soft_wrap: false,
            show_whitespaces: false,
            line_numbers: true,
            indent_guides: true,
            format_on_save: false,
            remove_trailing_whitespace_on_save: false,
            ensure_final_newline_on_save: true,
            inlay_hints: false,
            vim_mode: false,
            inline_blame: true,
            show_authors: false,
            side_by_side_comparison: true,
            terminal_shell: None,
            agent_servers: crate::acp::agents::builtins(),
            default_agent: "claude".into(),
            always_allow_tool_actions: false,
            zcode_mcp_server: true,
            edit_predictions: EditPredictionSettings::default(),
            profiling_time_limit: None,
            notary_keychain_profile: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// `"theme": "Name"`, or `{ "mode", "light", "dark" }`.
#[derive(Clone, Debug, PartialEq)]
pub enum ThemeSelection {
    Static(String),
    Dynamic { mode: ThemeMode, light: String, dark: String },
}

pub const DEFAULT_LIGHT_THEME: &str = "Default Light";
pub const DEFAULT_DARK_THEME: &str = "Default Dark";

impl Default for ThemeSelection {
    fn default() -> Self {
        Self::Dynamic { mode: ThemeMode::System, light: DEFAULT_LIGHT_THEME.into(), dark: DEFAULT_DARK_THEME.into() }
    }
}

impl ThemeSelection {
    /// The theme to show when the system's appearance is dark or not.
    pub fn name(&self, system_dark: bool) -> &str {
        match self {
            Self::Static(name) => name,
            Self::Dynamic { mode, light, dark } => {
                let dark_mode = match mode {
                    ThemeMode::System => system_dark,
                    ThemeMode::Light => false,
                    ThemeMode::Dark => true,
                };
                if dark_mode { dark } else { light }
            }
        }
    }

    /// Every theme it names.
    pub fn names(&self) -> Vec<&str> {
        match self {
            Self::Static(name) => vec![name],
            Self::Dynamic { light, dark, .. } => vec![light, dark],
        }
    }
}

/// A settings file as written: whatever it leaves out comes from the file
/// below it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SettingsContent {
    #[serde(deserialize_with = "theme")]
    pub theme: Option<ThemeSelection>,
    pub buffer_font_family: Option<String>,
    #[serde(deserialize_with = "font_size")]
    pub buffer_font_size: Option<f32>,
    #[serde(deserialize_with = "tab_size")]
    pub tab_size: Option<u32>,
    pub hard_tabs: Option<bool>,
    #[serde(deserialize_with = "soft_wrap")]
    pub soft_wrap: Option<bool>,
    #[serde(deserialize_with = "show_whitespaces")]
    pub show_whitespaces: Option<bool>,
    pub gutter: Option<GutterContent>,
    #[serde(deserialize_with = "enabled")]
    pub indent_guides: Option<bool>,
    #[serde(deserialize_with = "on_off")]
    pub format_on_save: Option<bool>,
    pub remove_trailing_whitespace_on_save: Option<bool>,
    pub ensure_final_newline_on_save: Option<bool>,
    #[serde(deserialize_with = "enabled")]
    pub inlay_hints: Option<bool>,
    pub vim_mode: Option<bool>,
    pub git: Option<GitContent>,
    pub terminal: Option<TerminalContent>,
    #[serde(deserialize_with = "agent_servers")]
    pub agent_servers: Option<Vec<(String, crate::acp::agents::AgentServerContent)>>,
    pub agent: Option<AgentContent>,
    pub edit_predictions: Option<EditPredictionsContent>,
    pub profiling: Option<ProfilingContent>,
    pub distribution: Option<DistributionContent>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DistributionContent {
    pub notary_keychain_profile: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ProfilingContent {
    /// Seconds; 0 records until Stop.
    pub time_limit: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GutterContent {
    pub line_numbers: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GitContent {
    #[serde(deserialize_with = "enabled")]
    pub inline_blame: Option<bool>,
    pub show_authors: Option<bool>,
    pub side_by_side_comparison: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TerminalContent {
    /// `Some(None)`: "system".
    #[serde(deserialize_with = "shell")]
    pub shell: Option<Option<(String, Vec<String>)>>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AgentContent {
    pub default_agent: Option<String>,
    pub always_allow_tool_actions: Option<bool>,
    pub zcode_mcp_server: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct EditPredictionsContent {
    #[serde(deserialize_with = "provider")]
    pub provider: Option<PredictionProvider>,
    pub automatic: Option<bool>,
    #[serde(deserialize_with = "debounce")]
    pub debounce_ms: Option<u64>,
    pub disabled_globs: Option<Vec<String>>,
    pub claude: Option<ClaudeContent>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ClaudeContent {
    pub model: Option<String>,
    pub api_key: Option<Secret>,
    pub api_url: Option<String>,
    pub max_tokens: Option<u32>,
    /// `Some(None)`: `null`, the model's own default.
    #[serde(deserialize_with = "effort")]
    pub effort: Option<Option<String>>,
}

/// Every key a settings file may have, with the keys of its objects: what
/// isn't here is reported as unknown (and otherwise ignored, as in Zed).
pub const KNOWN_KEYS: &[(&str, &[&str])] = &[
    ("theme", &["mode", "light", "dark"]),
    ("buffer_font_family", &[]),
    ("buffer_font_size", &[]),
    ("tab_size", &[]),
    ("hard_tabs", &[]),
    ("soft_wrap", &[]),
    ("show_whitespaces", &[]),
    ("gutter", &["line_numbers"]),
    ("indent_guides", &["enabled"]),
    ("format_on_save", &[]),
    ("remove_trailing_whitespace_on_save", &[]),
    ("ensure_final_newline_on_save", &[]),
    ("inlay_hints", &["enabled"]),
    ("vim_mode", &[]),
    ("git", &["inline_blame", "show_authors", "side_by_side_comparison"]),
    ("terminal", &["shell"]),
    // Any name: Zed's built-ins (claude, codex, gemini) or an agent of yours.
    ("agent_servers", &[]),
    ("agent", &["default_agent", "always_allow_tool_actions", "zcode_mcp_server"]),
    ("edit_predictions", &["provider", "automatic", "debounce_ms", "disabled_globs", "claude"]),
    ("profiling", &["time_limit"]),
    ("distribution", &["notary_keychain_profile"]),
];

/// Settings a project's file can't change: they're the app's, not a window's.
/// A project's file can't name programs to run or skip the permission
/// prompts either: a cloned repository could carry it. Nor can it choose
/// where code is sent for predictions (or with whose key); it can only turn
/// them off (`"edit_predictions": { "provider": "none" }`), or name the
/// keychain credentials apps are notarized with.
pub const USER_ONLY_KEYS: &[&str] = &["theme", "buffer_font_family", "vim_mode", "agent_servers", "agent", "distribution"];

/// What to say about the keys of a settings file zcode doesn't know
/// (`git.inline_blam`), and those a project's file sets that only the
/// user's can (`theme`).
pub fn unknown_keys(value: &Value, project: bool) -> Vec<String> {
    let Some(object) = value.as_object() else { return vec![] };
    let mut unknown = vec![];
    for (key, inner) in object {
        match KNOWN_KEYS.iter().find(|(known, _)| known == key) {
            None => unknown.push(format!("unknown setting “{key}”")),
            Some((_, nested)) => {
                if let Some(inner) = inner.as_object()
                    && !nested.is_empty()
                {
                    unknown.extend(inner.keys().filter(|k| !nested.contains(&k.as_str())).map(|k| format!("unknown setting “{key}.{k}”")));
                }
            }
        }
    }
    if project {
        unknown.extend(object.keys().filter(|k| USER_ONLY_KEYS.contains(&k.as_str())).map(|k| format!("“{k}” is only read from your own settings file")));
        if let Some(predictions) = object.get("edit_predictions").and_then(Value::as_object) {
            unknown.extend(
                predictions
                    .iter()
                    .filter(|(key, value)| !(key.as_str() == "provider" && value.as_str() == Some("none")))
                    .map(|(key, _)| format!("“edit_predictions.{key}” is only read from your own settings file (a project can only set \"provider\": \"none\")")),
            );
        }
    }
    unknown
}

impl Settings {
    /// These settings with what `content` says over them; a project's file
    /// (`project`) leaves the app's own settings alone.
    pub fn merged(mut self, content: &SettingsContent, project: bool) -> Self {
        macro_rules! take {
            ($field:ident) => {
                if let Some(value) = content.$field.clone() {
                    self.$field = value;
                }
            };
        }
        if !project {
            take!(theme);
            take!(vim_mode);
            if let Some(servers) = &content.agent_servers {
                self.agent_servers = crate::acp::agents::resolve(servers);
            }
            if let Some(agent) = &content.agent {
                self.default_agent = agent.default_agent.clone().unwrap_or(self.default_agent);
                self.always_allow_tool_actions = agent.always_allow_tool_actions.unwrap_or(self.always_allow_tool_actions);
                self.zcode_mcp_server = agent.zcode_mcp_server.unwrap_or(self.zcode_mcp_server);
            }
            if let Some(family) = &content.buffer_font_family {
                self.buffer_font_family = Some(family.clone()).filter(|f| !f.trim().is_empty());
            }
            if let Some(predictions) = &content.edit_predictions {
                self.edit_predictions.merge(predictions);
            }
            if let Some(profile) = content.distribution.as_ref().and_then(|d| d.notary_keychain_profile.clone()) {
                self.notary_keychain_profile = profile.trim().to_string();
            }
        } else if content.edit_predictions.as_ref().and_then(|p| p.provider) == Some(PredictionProvider::None) {
            self.edit_predictions.provider = PredictionProvider::None;
        }
        take!(buffer_font_size);
        take!(tab_size);
        take!(hard_tabs);
        take!(soft_wrap);
        take!(show_whitespaces);
        take!(indent_guides);
        take!(format_on_save);
        take!(remove_trailing_whitespace_on_save);
        take!(ensure_final_newline_on_save);
        take!(inlay_hints);
        if let Some(line_numbers) = content.gutter.as_ref().and_then(|g| g.line_numbers) {
            self.line_numbers = line_numbers;
        }
        if let Some(git) = &content.git {
            self.inline_blame = git.inline_blame.unwrap_or(self.inline_blame);
            self.show_authors = git.show_authors.unwrap_or(self.show_authors);
            self.side_by_side_comparison = git.side_by_side_comparison.unwrap_or(self.side_by_side_comparison);
        }
        if let Some(limit) = content.profiling.as_ref().and_then(|p| p.time_limit) {
            self.profiling_time_limit = (limit > 0).then_some(limit);
        }
        if let Some(shell) = content.terminal.as_ref().and_then(|t| t.shell.clone()) {
            self.terminal_shell = shell;
        }
        self
    }
}

impl EditPredictionSettings {
    fn merge(&mut self, content: &EditPredictionsContent) {
        self.provider = content.provider.unwrap_or(self.provider);
        self.automatic = content.automatic.unwrap_or(self.automatic);
        self.debounce_ms = content.debounce_ms.unwrap_or(self.debounce_ms);
        if let Some(globs) = &content.disabled_globs {
            self.disabled_globs = globs.clone();
        }
        if let Some(claude) = &content.claude {
            let mine = &mut self.claude;
            if let Some(model) = claude.model.clone().filter(|m| !m.trim().is_empty()) {
                mine.model = model;
            }
            if let Some(key) = claude.api_key.clone().filter(|k| !k.expose().trim().is_empty()) {
                mine.api_key = Some(key);
            }
            if let Some(url) = claude.api_url.clone().filter(|u| !u.trim().is_empty()) {
                mine.api_url = url;
            }
            mine.max_tokens = claude.max_tokens.unwrap_or(mine.max_tokens);
            if let Some(effort) = claude.effort.clone() {
                mine.effort = effort;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Values with more than one way to write them. Each reads a JSON value and
// says what it expected otherwise; serde_json adds the line and column.

fn read<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    Value::deserialize(deserializer)
}

/// A value that isn't one a setting takes. (`jsonc` puts these errors at
/// the value rather than at what follows it.)
fn expected<E: serde::de::Error>(what: &str, found: &Value) -> E {
    E::custom(format!("{}{what}, found {found}", super::jsonc::VALUE_ERROR))
}

fn theme<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<ThemeSelection>, D::Error> {
    match read(deserializer)? {
        Value::Null => Ok(None),
        Value::String(name) => Ok(Some(ThemeSelection::Static(name))),
        Value::Object(object) => {
            let name = |key: &str, default: &str| match object.get(key) {
                None | Some(Value::Null) => Ok(default.to_string()),
                Some(Value::String(name)) => Ok(name.clone()),
                Some(other) => Err(expected::<D::Error>(&format!("a theme name for \"{key}\""), other)),
            };
            let mode = match object.get("mode") {
                None | Some(Value::Null) => ThemeMode::System,
                Some(Value::String(mode)) if mode == "system" => ThemeMode::System,
                Some(Value::String(mode)) if mode == "light" => ThemeMode::Light,
                Some(Value::String(mode)) if mode == "dark" => ThemeMode::Dark,
                Some(other) => return Err(expected(r#""system", "light" or "dark" for "mode""#, other)),
            };
            Ok(Some(ThemeSelection::Dynamic { mode, light: name("light", DEFAULT_LIGHT_THEME)?, dark: name("dark", DEFAULT_DARK_THEME)? }))
        }
        other => Err(expected(r#"a theme name or { "mode", "light", "dark" }"#, &other)),
    }
}

fn font_size<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f32>, D::Error> {
    match read(deserializer)? {
        Value::Null => Ok(None),
        Value::Number(n) if n.as_f64().is_some_and(|n| (6. ..=72.).contains(&n)) => Ok(n.as_f64().map(|n| n as f32)),
        other => Err(expected("a size in points from 6 to 72", &other)),
    }
}

fn tab_size<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    match read(deserializer)? {
        Value::Null => Ok(None),
        Value::Number(n) if n.as_u64().is_some_and(|n| (1..=16).contains(&n)) => Ok(n.as_u64().map(|n| n as u32)),
        other => Err(expected("a number of columns from 1 to 16", &other)),
    }
}

/// A switch written as `true`/`false` or as one of its words.
fn words<E: serde::de::Error>(value: Value, on: &[&str], off: &[&str]) -> Result<Option<bool>, E> {
    match value {
        Value::Null => Ok(None),
        Value::Bool(on) => Ok(Some(on)),
        Value::String(word) if on.contains(&word.as_str()) => Ok(Some(true)),
        Value::String(word) if off.contains(&word.as_str()) => Ok(Some(false)),
        other => {
            let all: Vec<String> = off.iter().chain(on).map(|w| format!("\"{w}\"")).collect();
            Err(expected(&all.join(" or "), &other))
        }
    }
}

fn soft_wrap<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    // Zed's other modes wrap at the editor's edge here too.
    words(read(deserializer)?, &["editor_width", "prefer_line", "preferred_line_length", "bounded"], &["none"])
}

fn show_whitespaces<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    words(read(deserializer)?, &["all"], &["none"])
}

fn on_off<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    words(read(deserializer)?, &["on"], &["off"])
}

/// `true`, or Zed's `{ "enabled": true }`.
fn enabled<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    match read(deserializer)? {
        Value::Object(object) => match object.get("enabled") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(on)) => Ok(Some(*on)),
            Some(other) => Err(expected("true or false for \"enabled\"", other)),
        },
        other => {
            let found = other.clone();
            words(other, &[], &[]).map_err(|_: D::Error| expected::<D::Error>("{ \"enabled\": true } or false", &found))
        }
    }
}

/// `{ "name": { "command", "args", "env" }, … }` in the order written.
fn agent_servers<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Vec<(String, crate::acp::agents::AgentServerContent)>>, D::Error> {
    match read(deserializer)? {
        Value::Null => Ok(None),
        Value::Object(object) => object
            .into_iter()
            .map(|(name, value)| {
                if value.is_null() {
                    return Ok((name, Default::default()));
                }
                serde_json::from_value(value.clone())
                    .map(|content| (name.clone(), content))
                    .map_err(|_| expected::<D::Error>(&format!(r#"{{ "command", "args", "env" }} for "{name}""#), &value))
            })
            .collect::<Result<_, _>>()
            .map(Some),
        other => Err(expected(r#"{ "Agent Name": { "command", "args", "env" } }"#, &other)),
    }
}

fn provider<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<PredictionProvider>, D::Error> {
    match read(deserializer)? {
        Value::Null => Ok(None),
        Value::String(name) if name == "none" => Ok(Some(PredictionProvider::None)),
        Value::String(name) if name == "claude" => Ok(Some(PredictionProvider::Claude)),
        Value::String(name) if name == "lsp" => Ok(Some(PredictionProvider::Lsp)),
        other => Err(expected(r#""none", "claude" or "lsp""#, &other)),
    }
}

fn debounce<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    match read(deserializer)? {
        Value::Null => Ok(None),
        Value::Number(n) if n.as_u64().is_some_and(|n| n <= 5000) => Ok(n.as_u64()),
        other => Err(expected("a number of milliseconds from 0 to 5000", &other)),
    }
}

fn effort<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<String>>, D::Error> {
    match read(deserializer)? {
        Value::Null => Ok(Some(None)),
        Value::String(level) if ["low", "medium", "high", "xhigh", "max"].contains(&level.as_str()) => Ok(Some(Some(level))),
        other => Err(expected(r#""low", "medium", "high", "xhigh", "max" or null"#, &other)),
    }
}

fn shell<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<(String, Vec<String>)>>, D::Error> {
    let program_and_args = |object: &serde_json::Map<String, Value>| -> Result<(String, Vec<String>), D::Error> {
        let program = object.get("program").and_then(Value::as_str).ok_or_else(|| D::Error::custom("expected a \"program\" for the shell"))?;
        let args = match object.get("args") {
            None | Some(Value::Null) => vec![],
            Some(Value::Array(args)) => args.iter().map(|a| a.as_str().map(str::to_string).ok_or_else(|| expected::<D::Error>("strings in \"args\"", a))).collect::<Result<_, _>>()?,
            Some(other) => return Err(expected("a list of strings for \"args\"", other)),
        };
        Ok((program.to_string(), args))
    };
    match read(deserializer)? {
        Value::Null => Ok(None),
        Value::String(word) if word == "system" => Ok(Some(None)),
        Value::String(program) => Ok(Some(Some((program, vec![])))),
        // Zed writes `{ "program": … }` and `{ "with_arguments": { "program": …, "args": … } }`.
        Value::Object(object) => match object.get("with_arguments").and_then(Value::as_object) {
            Some(inner) => program_and_args(inner).map(|p| Some(Some(p))),
            None => program_and_args(&object).map(|p| Some(Some(p))),
        },
        other => Err(expected(r#""system", a program, or { "program", "args" }"#, &other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::jsonc;

    #[test]
    fn the_documented_defaults_are_the_defaults() {
        let content: SettingsContent = jsonc::parse(DEFAULT_SETTINGS).unwrap();
        assert_eq!(Settings::default().merged(&content, false), Settings::default());
        let value = jsonc::parse_value(DEFAULT_SETTINGS).unwrap();
        assert_eq!(unknown_keys(&value, false), Vec::<String>::new());
        let documented: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
        let known: Vec<&str> = KNOWN_KEYS.iter().map(|(key, _)| *key).collect();
        assert_eq!(documented, known, "every setting is documented, in the same order");
    }

    #[test]
    fn values_in_their_other_forms() {
        let text = r#"{
            "theme": "Midnight",
            "soft_wrap": "editor_width",
            "show_whitespaces": true,
            "format_on_save": "on",
            "inlay_hints": true,
            "git": { "inline_blame": false },
            "terminal": { "shell": { "with_arguments": { "program": "/bin/zsh", "args": ["-f"] } } },
        }"#;
        let settings = Settings::default().merged(&jsonc::parse(text).unwrap(), false);
        assert_eq!(settings.theme, ThemeSelection::Static("Midnight".into()));
        assert!(settings.soft_wrap && settings.show_whitespaces && settings.format_on_save && settings.inlay_hints);
        assert!(!settings.inline_blame);
        assert_eq!(settings.terminal_shell, Some(("/bin/zsh".into(), vec!["-f".into()])));
        let system: SettingsContent = jsonc::parse(r#"{ "terminal": { "shell": "system" } }"#).unwrap();
        assert_eq!(settings.merged(&system, false).terminal_shell, None);
    }

    #[test]
    fn a_project_overrides_the_editor_but_not_the_app() {
        let user: SettingsContent = jsonc::parse(r#"{ "tab_size": 2, "vim_mode": true, "theme": { "mode": "dark" } }"#).unwrap();
        let project: SettingsContent = jsonc::parse(r#"{ "tab_size": 8, "vim_mode": false, "theme": "Dusk" }"#).unwrap();
        let settings = Settings::default().merged(&user, false).merged(&project, true);
        assert_eq!(settings.tab_size, 8);
        assert!(settings.vim_mode);
        assert_eq!(settings.theme.name(false), DEFAULT_DARK_THEME);
        assert_eq!(
            unknown_keys(&serde_json::json!({ "theme": "Dusk", "tab_szie": 3, "git": { "inline_blam": true } }), true),
            ["unknown setting “tab_szie”", "unknown setting “git.inline_blam”", "“theme” is only read from your own settings file"]
        );
    }

    #[test]
    fn agents_from_the_users_file_only() {
        let user: SettingsContent = jsonc::parse(
            r#"{
            "agent_servers": { "claude": { "env": { "A": "1" } }, "Mine": { "command": "/opt/mine", "args": ["--acp"] } },
            "agent": { "default_agent": "Mine", "always_allow_tool_actions": true, "zcode_mcp_server": false },
        }"#,
        )
        .unwrap();
        let project: SettingsContent =
            jsonc::parse(r#"{ "agent_servers": { "Evil": { "command": "/tmp/evil" } }, "agent": { "always_allow_tool_actions": false, "zcode_mcp_server": true } }"#).unwrap();
        let settings = Settings::default().merged(&user, false).merged(&project, true);
        let ids: Vec<&str> = settings.agent_servers.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["claude", "codex", "gemini", "Mine"]);
        assert_eq!(settings.agent_servers[0].env.get("A").map(String::as_str), Some("1"));
        assert_eq!((settings.default_agent.as_str(), settings.always_allow_tool_actions), ("Mine", true));
        assert!(!settings.zcode_mcp_server, "a project can't hand agents zcode's tools either");
        assert!(Settings::default().zcode_mcp_server);
        let warnings = unknown_keys(&serde_json::json!({ "agent_servers": {}, "agent": { "default_agent": "x", "follow": true } }), true);
        assert_eq!(
            warnings,
            ["unknown setting “agent.follow”", "“agent_servers” is only read from your own settings file", "“agent” is only read from your own settings file"]
        );
        let err = jsonc::parse::<SettingsContent>(r#"{ "agent_servers": { "Mine": { "command": 3 } } }"#).unwrap_err();
        assert!(err.message.contains(r#"{ "command", "args", "env" } for "Mine""#), "{err}");
    }

    #[test]
    fn edit_predictions_from_the_users_file_and_off_from_a_projects() {
        let user: SettingsContent = jsonc::parse(
            r#"{ "edit_predictions": { "provider": "claude", "debounce_ms": 150,
                 "claude": { "model": "claude-sonnet-5-5", "api_key": "sk-ant-test", "effort": null } } }"#,
        )
        .unwrap();
        let settings = Settings::default().merged(&user, false);
        let predictions = &settings.edit_predictions;
        assert_eq!((predictions.provider, predictions.debounce_ms, predictions.automatic), (PredictionProvider::Claude, 150, true));
        assert_eq!(predictions.claude.model, "claude-sonnet-5-5");
        assert_eq!(predictions.claude.api_key.as_ref().map(Secret::expose), Some("sk-ant-test"));
        assert_eq!(predictions.claude.effort, None);
        assert_eq!(predictions.claude.api_url, "https://api.anthropic.com");
        assert!(!format!("{settings:?}").contains("sk-ant-test"), "the key stays out of Debug");

        // A project can't point predictions elsewhere, only turn them off.
        let evil: SettingsContent =
            jsonc::parse(r#"{ "edit_predictions": { "provider": "claude", "claude": { "api_url": "https://evil.example" } } }"#).unwrap();
        let kept = settings.clone().merged(&evil, true);
        assert_eq!(kept.edit_predictions, settings.edit_predictions);
        let off: SettingsContent = jsonc::parse(r#"{ "edit_predictions": { "provider": "none" } }"#).unwrap();
        assert_eq!(settings.clone().merged(&off, true).edit_predictions.provider, PredictionProvider::None);
        assert_eq!(unknown_keys(&serde_json::json!({ "edit_predictions": { "provider": "none" } }), true), Vec::<String>::new());
        let warnings = unknown_keys(&serde_json::json!({ "edit_predictions": { "provider": "claude", "claude": {} } }), true);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("edit_predictions.provider"), "{warnings:?}");

        let err = jsonc::parse::<SettingsContent>(r#"{ "edit_predictions": { "provider": "copilot" } }"#).unwrap_err();
        assert!(err.message.contains(r#""none", "claude" or "lsp""#), "{err}");
    }

    #[test]
    fn wrong_values_say_what_was_expected_and_where() {
        let err = jsonc::parse::<SettingsContent>("{\n  \"tab_size\": 0\n}").unwrap_err();
        assert_eq!((err.line, err.column), (2, 15), "{err}");
        assert!(err.message.starts_with("expected a number"), "{err}");
        assert!(err.message.contains("a number of columns from 1 to 16"), "{err}");
        let err = jsonc::parse::<SettingsContent>("{ \"soft_wrap\": \"sometimes\" }").unwrap_err();
        assert!(err.message.contains(r#"expected "none" or "editor_width""#), "{err}");
        let err = jsonc::parse::<SettingsContent>("{ \"theme\": { \"mode\": \"dim\" } }").unwrap_err();
        assert!(err.message.contains("\"mode\""), "{err}");
    }
}

//! What Xcode's Edit Scheme sheet (⌘<) edits for the Run and Test actions:
//! build configuration, launch arguments, environment variables, options
//! and diagnostics; and what Product ▸ Profile reads from the Profile action.
//!
//! An Xcode scheme keeps them in its `.xcscheme` (shared in
//! `xcshareddata/xcschemes`, or the user's in `xcuserdata`), which is read
//! and written in place: only the attributes and elements that change are
//! rewritten. Swift packages have no scheme files, and neither do the
//! schemes `xcodebuild` makes up for projects without any; their options
//! live in zcode's per-project session instead.
//!
//! The options reach every launch zcode makes itself (simulator, Mac
//! executables, packages, lldb-dap) through [`Launch`], and builds through
//! [`Diagnostics::xcodebuild_args`] and [`Diagnostics::swiftpm_args`].
//! `xcodebuild` reads a scheme file's Test action itself, and writes it (or
//! the test plan's settings) into the `.xctestrun` the simulator test runner
//! uses.

pub mod xml;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::project::ProjectKind;
use xml::{Edits, Element, block_text};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Argument {
    /// Split into words like a shell would, as Xcode does: one entry may be
    /// `-com.apple.CoreData.SQLDebug 1`.
    pub value: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variable {
    pub key: String,
    pub value: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Diagnostics {
    pub address_sanitizer: bool,
    pub thread_sanitizer: bool,
    pub undefined_behavior_sanitizer: bool,
    /// On by default; applies to runs under the debugger, as in Xcode.
    pub main_thread_checker: bool,
    pub malloc_scribble: bool,
    pub malloc_guard_edges: bool,
    pub zombie_objects: bool,
    pub malloc_stack_logging: bool,
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self {
            address_sanitizer: false,
            thread_sanitizer: false,
            undefined_behavior_sanitizer: false,
            main_thread_checker: true,
            malloc_scribble: false,
            malloc_guard_edges: false,
            zombie_objects: false,
            malloc_stack_logging: false,
        }
    }
}

/// The Memory Management checkboxes: `AdditionalOption` keys in the scheme,
/// environment variables at launch.
const MEMORY_OPTIONS: &[(&str, &str)] = &[
    ("MallocScribble", ""),
    ("MallocGuardEdges", ""),
    ("NSZombieEnabled", "YES"),
    ("MallocStackLogging", ""),
];

impl Diagnostics {
    fn memory_flag(&mut self, key: &str) -> Option<&mut bool> {
        match key {
            "MallocScribble" => Some(&mut self.malloc_scribble),
            "MallocGuardEdges" => Some(&mut self.malloc_guard_edges),
            "NSZombieEnabled" => Some(&mut self.zombie_objects),
            "MallocStackLogging" => Some(&mut self.malloc_stack_logging),
            _ => None,
        }
    }

    fn memory_options(&self) -> Vec<&'static str> {
        let mut copy = self.clone();
        MEMORY_OPTIONS
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| copy.memory_flag(key).is_some_and(|on| *on))
            .collect()
    }

    /// The environment the Memory Management options set at launch.
    pub fn environment(&self) -> Vec<(String, String)> {
        self.memory_options()
            .into_iter()
            .map(|key| (key.to_string(), if key == "NSZombieEnabled" { "YES" } else { "1" }.to_string()))
            .collect()
    }

    /// `xcodebuild` flags for the sanitizers that are on.
    pub fn xcodebuild_args(&self) -> Vec<String> {
        [
            (self.address_sanitizer, "-enableAddressSanitizer"),
            (self.thread_sanitizer, "-enableThreadSanitizer"),
            (self.undefined_behavior_sanitizer, "-enableUndefinedBehaviorSanitizer"),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .flat_map(|(_, flag)| [flag.to_string(), "YES".to_string()])
        .collect()
    }

    /// `swift build` / `swift test` flags for the sanitizers that are on.
    pub fn swiftpm_args(&self) -> Vec<String> {
        [
            (self.address_sanitizer, "--sanitize=address"),
            (self.thread_sanitizer, "--sanitize=thread"),
            (self.undefined_behavior_sanitizer, "--sanitize=undefined"),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, flag)| flag.to_string())
        .collect()
    }
}

/// One scheme action's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActionOptions {
    pub configuration: String,
    pub arguments: Vec<Argument>,
    pub environment: Vec<Variable>,
    pub diagnostics: Diagnostics,
}

impl Default for ActionOptions {
    fn default() -> Self {
        Self { configuration: "Debug".into(), arguments: vec![], environment: vec![], diagnostics: Diagnostics::default() }
    }
}

/// The Profile action, read but not edited: what Product ▸ Profile builds
/// and records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileOptions {
    /// Release unless the scheme says otherwise, as in Xcode.
    pub configuration: String,
    /// Arguments ▸ Use the Run action's arguments and environment variables
    /// (on in the schemes Xcode makes).
    pub uses_run_arguments: bool,
    pub arguments: Vec<Argument>,
    pub environment: Vec<Variable>,
    /// Info ▸ Instrument (`savedToolIdentifier`): a template's name or
    /// path; empty is Ask on Launch.
    pub template: String,
}

impl Default for ProfileOptions {
    fn default() -> Self {
        Self {
            configuration: "Release".into(),
            uses_run_arguments: true,
            arguments: vec![],
            environment: vec![],
            template: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SchemeOptions {
    pub run: ActionOptions,
    pub test: ActionOptions,
    pub profile: ProfileOptions,
    /// Run ▸ Info ▸ Debug executable: whether ⌘R may run under the debugger.
    pub debug_executable: bool,
    /// Run ▸ Options ▸ Working Directory (Mac executables and packages).
    pub use_custom_working_directory: bool,
    pub custom_working_directory: String,
    /// Run ▸ Options ▸ Persistent State: launch without state restoration.
    pub ignore_persistent_state: bool,
    /// Test ▸ Arguments ▸ Use the Run action's arguments and environment
    /// variables.
    pub test_uses_run_arguments: bool,
}

impl Default for SchemeOptions {
    fn default() -> Self {
        Self {
            run: ActionOptions::default(),
            test: ActionOptions::default(),
            profile: ProfileOptions::default(),
            debug_executable: true,
            use_custom_working_directory: false,
            custom_working_directory: String::new(),
            ignore_persistent_state: false,
            test_uses_run_arguments: true,
        }
    }
}

/// Arguments, environment and working directory for one launch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    pub arguments: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub working_directory: Option<PathBuf>,
}

impl Launch {
    /// Adds a library to `DYLD_INSERT_LIBRARIES`, after any already there.
    pub fn insert_library(&mut self, library: &Path) {
        let library = library.display().to_string();
        match self.environment.iter_mut().find(|(k, _)| k == "DYLD_INSERT_LIBRARIES") {
            Some((_, value)) if !value.is_empty() => *value = format!("{value}:{library}"),
            Some((_, value)) => *value = library,
            None => self.environment.push(("DYLD_INSERT_LIBRARIES".into(), library)),
        }
    }
}

fn launch_lists(arguments: &[Argument], environment: &[Variable], settings: Option<&HashMap<String, String>>) -> Launch {
    let expand = |s: &str| match settings {
        Some(settings) => expand_macros(s, settings),
        None => s.to_string(),
    };
    Launch {
        arguments: arguments
            .iter()
            .filter(|a| a.enabled)
            .flat_map(|a| split_arguments(&expand(&a.value)))
            .collect(),
        environment: environment
            .iter()
            .filter(|v| v.enabled && !v.key.trim().is_empty())
            .map(|v| (v.key.trim().to_string(), expand(&v.value)))
            .collect(),
        working_directory: None,
    }
}

impl SchemeOptions {
    /// What the Run action launches the executable with. `settings` (the
    /// product's build settings) expand `$(MACRO)`s, as Xcode does.
    pub fn run_launch(&self, settings: Option<&HashMap<String, String>>) -> Launch {
        let mut launch = launch_lists(&self.run.arguments, &self.run.environment, settings);
        if self.ignore_persistent_state {
            launch.arguments.extend(["-ApplePersistenceIgnoreState".to_string(), "YES".to_string()]);
        }
        for (key, value) in self.run.diagnostics.environment() {
            if !launch.environment.iter().any(|(k, _)| *k == key) {
                launch.environment.push((key, value));
            }
        }
        let directory = self.custom_working_directory.trim();
        if self.use_custom_working_directory && !directory.is_empty() {
            let expanded = settings.map_or(directory.to_string(), |s| expand_macros(directory, s));
            launch.working_directory = Some(PathBuf::from(expanded));
        }
        launch
    }

    /// What Product ▸ Profile launches the product with: the Profile
    /// action's arguments and environment, or the Run action's (without its
    /// diagnostics, which Instruments doesn't take either), and the Run
    /// action's working directory.
    pub fn profile_launch(&self, settings: Option<&HashMap<String, String>>) -> Launch {
        let mut launch = if self.profile.uses_run_arguments {
            launch_lists(&self.run.arguments, &self.run.environment, settings)
        } else {
            launch_lists(&self.profile.arguments, &self.profile.environment, settings)
        };
        launch.working_directory = self.run_launch(settings).working_directory;
        launch
    }

    /// The Test action's own arguments and environment, or the Run
    /// action's when it uses those.
    pub fn test_lists(&self) -> (&[Argument], &[Variable]) {
        if self.test_uses_run_arguments {
            (&self.run.arguments, &self.run.environment)
        } else {
            (&self.test.arguments, &self.test.environment)
        }
    }

    /// What test processes get, for runners that don't read the scheme
    /// file themselves (packages, and schemes without one).
    pub fn test_launch(&self) -> Launch {
        let (arguments, environment) = self.test_lists();
        let mut launch = launch_lists(arguments, environment, None);
        for (key, value) in self.test.diagnostics.environment() {
            if !launch.environment.iter().any(|(k, _)| *k == key) {
                launch.environment.push((key, value));
            }
        }
        launch
    }
}

/// Splits an argument entry into words: whitespace separates them, quotes
/// group them and a backslash escapes the next character.
pub fn split_arguments(text: &str) -> Vec<String> {
    let mut words = vec![];
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') | (None, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
                in_word = true;
            }
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// Expands `$(NAME)` and `${NAME}` from build settings; unknown names expand
/// to nothing, as in Xcode.
pub fn expand_macros(text: &str, settings: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let close = match rest.as_bytes().get(1) {
            Some(b'(') => ')',
            Some(b'{') => '}',
            _ => {
                out.push('$');
                rest = &rest[1..];
                continue;
            }
        };
        let Some(end) = rest.find(close) else {
            out.push_str(rest);
            return out;
        };
        let name = &rest[2..end];
        if let Some(value) = settings.get(name) {
            out.push_str(value);
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Where a scheme's options live

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    File(PathBuf),
    /// zcode's session (Swift packages, schemes without a file).
    Session,
}

/// A scheme's options as last read.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub scheme: String,
    pub options: SchemeOptions,
    pub source: Source,
    pub mtime: Option<SystemTime>,
    /// The Run action's product (`App.app`).
    pub executable: Option<String>,
    /// The Test action lists test plans: they set the test arguments,
    /// environment and diagnostics instead of the scheme.
    pub uses_test_plans: bool,
    /// Why the scheme file couldn't be read.
    pub error: Option<String>,
}

impl Loaded {
    pub fn is_shared(&self) -> bool {
        matches!(&self.source, Source::File(path) if path.components().any(|c| c.as_os_str() == "xcshareddata"))
    }

    /// Whether the scheme file changed on disk since it was read.
    pub fn is_stale(&self) -> bool {
        match &self.source {
            Source::File(path) => file_mtime(path) != self.mtime,
            Source::Session => false,
        }
    }
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The scheme file for `scheme`: shared first, then the user's.
pub fn scheme_file(kind: &ProjectKind, scheme: &str) -> Option<PathBuf> {
    let containers: Vec<PathBuf> = match kind {
        ProjectKind::XcodeWorkspace(path) => {
            std::iter::once(path.clone()).chain(crate::project::xcode_projects(kind)).collect()
        }
        ProjectKind::XcodeProject(path) => vec![path.clone()],
        _ => return None,
    };
    let file = format!("{scheme}.xcscheme");
    for container in &containers {
        let shared = container.join("xcshareddata/xcschemes").join(&file);
        if shared.is_file() {
            return Some(shared);
        }
        if let Ok(users) = std::fs::read_dir(container.join("xcuserdata")) {
            for user in users.flatten() {
                let candidate = user.path().join("xcschemes").join(&file);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Reads a scheme's options (blocking): from its file when it has one,
/// otherwise `stored` (from the session) or the defaults.
pub fn load(kind: &ProjectKind, scheme: &str, stored: Option<&SchemeOptions>) -> Loaded {
    let session = |error: Option<String>| Loaded {
        scheme: scheme.to_string(),
        options: stored.cloned().unwrap_or_default(),
        source: Source::Session,
        mtime: None,
        executable: matches!(kind, ProjectKind::SwiftPackage).then(|| scheme.to_string()),
        uses_test_plans: false,
        error,
    };
    let Some(path) = scheme_file(kind, scheme) else { return session(None) };
    let mtime = file_mtime(&path);
    let parsed = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))
        .and_then(|text| read(&text));
    match parsed {
        Ok(read) => Loaded {
            scheme: scheme.to_string(),
            options: read.options,
            source: Source::File(path),
            mtime,
            executable: read.executable,
            uses_test_plans: read.uses_test_plans,
            error: None,
        },
        Err(err) => Loaded { source: Source::File(path), mtime, ..session(Some(format!("{err:#}"))) },
    }
}

/// Writes `options` where `loaded` came from: edits the scheme file in place
/// (only what differs from the file as it is now). Session-backed options
/// are the caller's to store. Returns the file's new modification time.
pub fn save(loaded: &Loaded, options: &SchemeOptions) -> Result<Option<SystemTime>> {
    let Source::File(path) = &loaded.source else { return Ok(None) };
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let updated = write(&text, options)?;
    if updated != text {
        std::fs::write(path, updated).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(file_mtime(path))
}

// ---------------------------------------------------------------------------
// The `.xcscheme` format

pub struct ReadScheme {
    pub options: SchemeOptions,
    pub executable: Option<String>,
    pub uses_test_plans: bool,
}

const DEBUGGER_LLDB: &str = "Xcode.DebuggerFoundation.Debugger.LLDB";
const LAUNCHER_LLDB: &str = "Xcode.DebuggerFoundation.Launcher.LLDB";
const LAUNCHER_POSIX_SPAWN: &str = "Xcode.IDEFoundation.Launcher.PosixSpawn";

fn flag(element: &Element, name: &str, default: bool) -> bool {
    element.attribute(name).map_or(default, |v| v == "YES")
}

fn read_arguments(action: &Element) -> Vec<Argument> {
    action
        .child("CommandLineArguments")
        .map(|list| {
            list.children_named("CommandLineArgument")
                .map(|a| Argument {
                    value: a.attribute("argument").unwrap_or("").to_string(),
                    enabled: flag(a, "isEnabled", true),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn read_variables(action: &Element, list: &str, entry: &str) -> Vec<Variable> {
    action
        .child(list)
        .map(|list| {
            list.children_named(entry)
                .map(|v| Variable {
                    key: v.attribute("key").unwrap_or("").to_string(),
                    value: v.attribute("value").unwrap_or("").to_string(),
                    enabled: flag(v, "isEnabled", true),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn read_action(action: &Element) -> ActionOptions {
    let mut diagnostics = Diagnostics {
        address_sanitizer: flag(action, "enableAddressSanitizer", false),
        thread_sanitizer: flag(action, "enableThreadSanitizer", false),
        undefined_behavior_sanitizer: flag(action, "enableUBSanitizer", false),
        main_thread_checker: !flag(action, "disableMainThreadChecker", false),
        ..Diagnostics::default()
    };
    for option in read_variables(action, "AdditionalOptions", "AdditionalOption") {
        if let Some(on) = diagnostics.memory_flag(&option.key) {
            *on = option.enabled;
        }
    }
    ActionOptions {
        configuration: action.attribute("buildConfiguration").unwrap_or("Debug").to_string(),
        arguments: read_arguments(action),
        environment: read_variables(action, "EnvironmentVariables", "EnvironmentVariable"),
        diagnostics,
    }
}

pub fn read(text: &str) -> Result<ReadScheme> {
    let root = xml::parse(text)?;
    let mut options = SchemeOptions::default();
    let mut executable = None;
    let mut uses_test_plans = false;
    if let Some(launch) = root.child("LaunchAction") {
        options.run = read_action(launch);
        options.debug_executable = launch.attribute("selectedDebuggerIdentifier").is_none_or(|d| !d.is_empty());
        options.use_custom_working_directory = flag(launch, "useCustomWorkingDirectory", false);
        options.custom_working_directory = launch.attribute("customWorkingDirectory").unwrap_or("").to_string();
        options.ignore_persistent_state = flag(launch, "ignoresPersistentStateOnLaunch", false);
        executable = ["BuildableProductRunnable", "MacroExpansion"]
            .iter()
            .filter_map(|name| launch.child(name)?.child("BuildableReference")?.attribute("BuildableName"))
            .next()
            .or_else(|| launch.child("PathRunnable")?.attribute("FilePath"))
            .map(str::to_string);
    }
    if let Some(profile) = root.child("ProfileAction") {
        let action = read_action(profile);
        options.profile = ProfileOptions {
            configuration: profile.attribute("buildConfiguration").unwrap_or("Release").to_string(),
            uses_run_arguments: flag(profile, "shouldUseLaunchSchemeArgsEnv", true),
            arguments: action.arguments,
            environment: action.environment,
            template: profile.attribute("savedToolIdentifier").unwrap_or("").to_string(),
        };
    }
    if let Some(test) = root.child("TestAction") {
        options.test = read_action(test);
        options.test_uses_run_arguments = flag(test, "shouldUseLaunchSchemeArgsEnv", false);
        uses_test_plans = test.child("TestPlans").is_some_and(|p| !p.children.is_empty());
    }
    Ok(ReadScheme { options, executable, uses_test_plans })
}

/// Where Xcode puts an action's children, so new ones land in its order.
const CHILD_ORDER: &[&str] = &[
    "PreActions",
    "PostActions",
    "BuildableProductRunnable",
    "PathRunnable",
    "RemoteRunnable",
    "MacroExpansion",
    "TestPlans",
    "Testables",
    "CommandLineArguments",
    "EnvironmentVariables",
    "AdditionalOptions",
];

/// Replaces, removes or inserts the child `name` of `action` so that it
/// holds `entries` (made by `entry`), leaving it alone when it already does.
fn set_list<T: PartialEq>(
    edits: &mut Edits,
    text: &str,
    action: &Element,
    name: &str,
    current: &[T],
    entries: &[T],
    entry: &dyn Fn(&str, &T) -> String,
) {
    if current == entries {
        return;
    }
    let block = |indent: &str| {
        let children: Vec<Box<dyn Fn(&str) -> String + '_>> = entries
            .iter()
            .map(|e| Box::new(move |indent: &str| entry(indent, e)) as Box<dyn Fn(&str) -> String + '_>)
            .collect();
        let refs: Vec<&dyn Fn(&str) -> String> = children.iter().map(|c| c.as_ref()).collect();
        block_text(indent, name, &[], &refs)
    };
    match action.child(name) {
        Some(existing) if entries.is_empty() => edits.replace_element(text, existing, None),
        Some(existing) => edits.replace_element(text, existing, Some(&block)),
        None if entries.is_empty() => {}
        None => {
            let rank = |n: &str| CHILD_ORDER.iter().position(|o| *o == n);
            let mine = rank(name).unwrap_or(CHILD_ORDER.len());
            let after = action.children.iter().rev().find(|c| rank(&c.name).is_some_and(|r| r < mine));
            edits.insert_child(text, action, after, &block);
        }
    }
}

/// Sets a YES/NO attribute, leaving it out when it is missing and `value`
/// is what a missing attribute means.
fn set_flag(edits: &mut Edits, text: &str, element: &Element, name: &str, value: bool, default: bool) {
    if flag(element, name, default) == value {
        return;
    }
    edits.set_attribute(text, element, name, if value { "YES" } else { "NO" });
}

fn yes_no(on: bool) -> String {
    if on { "YES" } else { "NO" }.to_string()
}

fn write_action(edits: &mut Edits, text: &str, action: &Element, new: &ActionOptions, lists: bool) {
    let old = read_action(action);
    if old.configuration != new.configuration {
        edits.set_attribute(text, action, "buildConfiguration", &new.configuration);
    }
    if !lists {
        return;
    }
    let (d, was) = (&new.diagnostics, &old.diagnostics);
    if d.address_sanitizer != was.address_sanitizer {
        set_flag(edits, text, action, "enableAddressSanitizer", d.address_sanitizer, false);
    }
    if d.thread_sanitizer != was.thread_sanitizer {
        set_flag(edits, text, action, "enableThreadSanitizer", d.thread_sanitizer, false);
    }
    if d.undefined_behavior_sanitizer != was.undefined_behavior_sanitizer {
        set_flag(edits, text, action, "enableUBSanitizer", d.undefined_behavior_sanitizer, false);
    }
    if d.main_thread_checker != was.main_thread_checker {
        set_flag(edits, text, action, "disableMainThreadChecker", !d.main_thread_checker, false);
    }
    set_list(edits, text, action, "CommandLineArguments", &old.arguments, &new.arguments, &|indent, a: &Argument| {
        block_text(indent, "CommandLineArgument", &[("argument", a.value.clone()), ("isEnabled", yes_no(a.enabled))], &[])
    });
    set_list(edits, text, action, "EnvironmentVariables", &old.environment, &new.environment, &|indent, v: &Variable| {
        block_text(
            indent,
            "EnvironmentVariable",
            &[("key", v.key.clone()), ("value", v.value.clone()), ("isEnabled", yes_no(v.enabled))],
            &[],
        )
    });
    // Memory Management options, keeping any other additional options.
    let current = read_variables(action, "AdditionalOptions", "AdditionalOption");
    let wanted = d.memory_options();
    let mut options: Vec<Variable> = current
        .iter()
        .filter(|o| !MEMORY_OPTIONS.iter().any(|(k, _)| *k == o.key) || wanted.contains(&o.key.as_str()))
        .cloned()
        .map(|mut o| {
            if wanted.contains(&o.key.as_str()) {
                o.enabled = true;
            }
            o
        })
        .collect();
    for (key, value) in MEMORY_OPTIONS {
        if wanted.contains(key) && !options.iter().any(|o| o.key == *key) {
            options.push(Variable { key: key.to_string(), value: value.to_string(), enabled: true });
        }
    }
    set_list(edits, text, action, "AdditionalOptions", &current, &options, &|indent, o: &Variable| {
        block_text(
            indent,
            "AdditionalOption",
            &[("key", o.key.clone()), ("value", o.value.clone()), ("isEnabled", yes_no(o.enabled))],
            &[],
        )
    });
}

/// `text` (an `.xcscheme`) with `options` written into its Run and Test
/// actions; everything else stays as it was, byte for byte.
pub fn write(text: &str, options: &SchemeOptions) -> Result<String> {
    let root = xml::parse(text)?;
    let current = read(text)?;
    let mut edits = Edits::default();
    if let Some(launch) = root.child("LaunchAction") {
        write_action(&mut edits, text, launch, &options.run, true);
        let old = &current.options;
        if old.debug_executable != options.debug_executable {
            let (debugger, launcher) =
                if options.debug_executable { (DEBUGGER_LLDB, LAUNCHER_LLDB) } else { ("", LAUNCHER_POSIX_SPAWN) };
            edits.set_attribute(text, launch, "selectedDebuggerIdentifier", debugger);
            edits.set_attribute(text, launch, "selectedLauncherIdentifier", launcher);
        }
        if old.use_custom_working_directory != options.use_custom_working_directory {
            set_flag(&mut edits, text, launch, "useCustomWorkingDirectory", options.use_custom_working_directory, false);
        }
        if old.custom_working_directory != options.custom_working_directory {
            match options.custom_working_directory.as_str() {
                "" => edits.remove_attribute(text, launch, "customWorkingDirectory"),
                path => edits.set_attribute(text, launch, "customWorkingDirectory", path),
            }
        }
        if old.ignore_persistent_state != options.ignore_persistent_state {
            set_flag(&mut edits, text, launch, "ignoresPersistentStateOnLaunch", options.ignore_persistent_state, false);
        }
    }
    if let Some(test) = root.child("TestAction") {
        // With test plans, the plan sets the arguments, environment and
        // diagnostics; the scheme's are left as they are.
        write_action(&mut edits, text, test, &options.test, !current.uses_test_plans);
        if !current.uses_test_plans && current.options.test_uses_run_arguments != options.test_uses_run_arguments {
            set_flag(&mut edits, text, test, "shouldUseLaunchSchemeArgsEnv", options.test_uses_run_arguments, false);
        }
    }
    if edits.is_empty() {
        return Ok(text.to_string());
    }
    let updated = edits.apply(text)?;
    xml::parse(&updated).context("the edited scheme doesn't parse")?;
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/samples/HelloZ/HelloZ.xcodeproj/xcshareddata/xcschemes/HelloZ.xcscheme"
        ))
        .unwrap()
    }

    #[test]
    fn reads_the_sample_scheme() {
        let read = read(&sample()).unwrap();
        assert_eq!(read.executable.as_deref(), Some("HelloZ.app"));
        assert!(read.uses_test_plans);
        let o = read.options;
        assert_eq!((o.run.configuration.as_str(), o.test.configuration.as_str()), ("Debug", "Debug"));
        assert!(o.debug_executable && o.test_uses_run_arguments && o.run.diagnostics.main_thread_checker);
        assert!(o.run.arguments.is_empty() && o.run.environment.is_empty());
        assert!(!o.use_custom_working_directory && !o.ignore_persistent_state);
    }

    #[test]
    fn reads_the_profile_action() {
        let o = read(&sample()).unwrap().options;
        assert_eq!(o.profile, ProfileOptions::default(), "Release, the Run action's arguments, Ask on Launch");
        let text = sample().replace(
            "savedToolIdentifier = \"\"",
            "savedToolIdentifier = \"Time Profiler\"",
        );
        let text = text.replacen("buildConfiguration = \"Release\"\n      shouldUseLaunchSchemeArgsEnv = \"YES\"", "buildConfiguration = \"Profiling\"\n      shouldUseLaunchSchemeArgsEnv = \"NO\"", 1);
        let text = text.replacen(
            "</BuildableProductRunnable>\n   </ProfileAction>",
            "</BuildableProductRunnable>\n      <CommandLineArguments>\n         <CommandLineArgument\n            argument = \"-profiling YES\"\n            isEnabled = \"YES\">\n         </CommandLineArgument>\n      </CommandLineArguments>\n   </ProfileAction>",
            1,
        );
        let mut o = read(&text).unwrap().options;
        assert_eq!(o.profile.configuration, "Profiling");
        assert_eq!(o.profile.template, "Time Profiler");
        assert!(!o.profile.uses_run_arguments);
        o.run.arguments = vec![Argument { value: "-run".into(), enabled: true }];
        o.run.diagnostics.zombie_objects = true;
        assert_eq!(o.profile_launch(None).arguments, ["-profiling", "YES"]);
        o.profile.uses_run_arguments = true;
        let launch = o.profile_launch(None);
        assert_eq!(launch.arguments, ["-run"]);
        assert!(launch.environment.is_empty(), "no diagnostics under Instruments: {launch:?}");
        // Read only: saving leaves the Profile action as it is.
        let mut changed = read(&text).unwrap().options;
        changed.profile.template = "Leaks".into();
        assert_eq!(write(&text, &changed).unwrap(), text);
    }

    #[test]
    fn unchanged_options_leave_the_file_alone() {
        let text = sample();
        let options = read(&text).unwrap().options;
        assert_eq!(write(&text, &options).unwrap(), text);
    }

    #[test]
    fn writes_run_options_in_xcode_format() {
        let text = sample();
        let mut options = read(&text).unwrap().options;
        options.run.configuration = "Release".into();
        options.run.arguments = vec![
            Argument { value: "-verbose".into(), enabled: true },
            Argument { value: "-off".into(), enabled: false },
        ];
        options.run.environment = vec![Variable { key: "API_URL".into(), value: "http://x/?a=1&b=\"2\"".into(), enabled: true }];
        options.run.diagnostics.zombie_objects = true;
        options.run.diagnostics.address_sanitizer = true;
        options.run.diagnostics.main_thread_checker = false;
        options.debug_executable = false;
        options.ignore_persistent_state = true;
        let out = write(&text, &options).unwrap();
        let expected = r#"            </BuildableReference>
      </BuildableProductRunnable>
      <CommandLineArguments>
         <CommandLineArgument
            argument = "-verbose"
            isEnabled = "YES">
         </CommandLineArgument>
         <CommandLineArgument
            argument = "-off"
            isEnabled = "NO">
         </CommandLineArgument>
      </CommandLineArguments>
      <EnvironmentVariables>
         <EnvironmentVariable
            key = "API_URL"
            value = "http://x/?a=1&amp;b=&quot;2&quot;"
            isEnabled = "YES">
         </EnvironmentVariable>
      </EnvironmentVariables>
      <AdditionalOptions>
         <AdditionalOption
            key = "NSZombieEnabled"
            value = "YES"
            isEnabled = "YES">
         </AdditionalOption>
      </AdditionalOptions>
   </LaunchAction>"#;
        assert!(out.contains(expected), "{out}");
        assert!(out.contains("   <LaunchAction\n      buildConfiguration = \"Release\"\n      selectedDebuggerIdentifier = \"\"\n      selectedLauncherIdentifier = \"Xcode.IDEFoundation.Launcher.PosixSpawn\""), "{out}");
        assert!(out.contains("ignoresPersistentStateOnLaunch = \"YES\""));
        assert!(out.contains("allowLocationSimulation = \"YES\"\n      enableAddressSanitizer = \"YES\"\n      disableMainThreadChecker = \"YES\">"), "{out}");
        // The Test action and everything else are as they were.
        let test = |s: &str| s[s.find("<TestAction").unwrap()..s.find("</TestAction>").unwrap()].to_string();
        assert_eq!(test(&out), test(&text));
        assert_eq!(read(&out).unwrap().options, options);

        // Turning everything back restores the file, except attributes
        // Xcode writes either way.
        let mut back = read(&text).unwrap().options;
        back.debug_executable = true;
        let restored = write(&out, &back).unwrap();
        assert_eq!(read(&restored).unwrap().options, back);
        assert!(!restored.contains("CommandLineArguments") && !restored.contains("AdditionalOptions"));
        assert!(restored.contains("enableAddressSanitizer = \"NO\""));
    }

    #[test]
    fn test_action_with_and_without_plans() {
        let text = sample();
        let mut options = read(&text).unwrap().options;
        options.test.configuration = "Release".into();
        options.test.environment = vec![Variable { key: "T".into(), value: "1".into(), enabled: true }];
        options.test_uses_run_arguments = false;
        // The plan owns the test environment: only the configuration changes.
        let out = write(&text, &options).unwrap();
        assert!(!out.contains("<EnvironmentVariables>") && out.contains("shouldUseLaunchSchemeArgsEnv = \"YES\""));
        assert!(out.contains("   <TestAction\n      buildConfiguration = \"Release\""), "{out}");

        let start = text.find("      <TestPlans>").unwrap();
        let end = text.find("</TestPlans>\n").unwrap() + "</TestPlans>\n".len();
        let without_plans = format!("{}{}", &text[..start], &text[end..]);
        let out = write(&without_plans, &options).unwrap();
        assert!(out.contains(
            "      shouldUseLaunchSchemeArgsEnv = \"NO\">\n      <EnvironmentVariables>\n         <EnvironmentVariable\n            key = \"T\""
        ), "{out}");
        let read_back = read(&out).unwrap();
        assert!(!read_back.uses_test_plans);
        assert_eq!(read_back.options.test, options.test);
        assert_eq!(options.test_launch().environment, [("T".to_string(), "1".to_string())]);
    }

    #[test]
    fn keeps_other_additional_options() {
        let text = r#"<Scheme>
   <LaunchAction
      buildConfiguration = "Debug">
      <AdditionalOptions>
         <AdditionalOption
            key = "PrefersMallocStackLoggingLite"
            value = ""
            isEnabled = "YES">
         </AdditionalOption>
         <AdditionalOption
            key = "MallocScribble"
            value = ""
            isEnabled = "YES">
         </AdditionalOption>
      </AdditionalOptions>
   </LaunchAction>
</Scheme>
"#;
        let mut options = read(text).unwrap().options;
        assert!(options.run.diagnostics.malloc_scribble);
        options.run.diagnostics.malloc_scribble = false;
        options.run.diagnostics.malloc_guard_edges = true;
        let out = write(text, &options).unwrap();
        assert!(out.contains("PrefersMallocStackLoggingLite") && out.contains("MallocGuardEdges") && !out.contains("MallocScribble"), "{out}");
        assert_eq!(options.run_launch(None).environment, [("MallocGuardEdges".to_string(), "1".to_string())]);
    }

    #[test]
    fn launch_arguments_and_environment() {
        let mut options = SchemeOptions::default();
        options.run.arguments = vec![
            Argument { value: "-com.apple.CoreData.SQLDebug 1".into(), enabled: true },
            Argument { value: "--name \"two words\" it\\'s".into(), enabled: true },
            Argument { value: "-skipped".into(), enabled: false },
            Argument { value: "$(PRODUCT_NAME).log".into(), enabled: true },
        ];
        options.run.environment = vec![
            Variable { key: "ROOT".into(), value: "${SRCROOT}/x$(NOPE)".into(), enabled: true },
            Variable { key: "OFF".into(), value: "1".into(), enabled: false },
            Variable { key: " ".into(), value: "blank".into(), enabled: true },
        ];
        options.run.diagnostics.zombie_objects = true;
        options.use_custom_working_directory = true;
        options.custom_working_directory = "$(SRCROOT)/run".into();
        let settings: HashMap<String, String> =
            [("SRCROOT", "/src"), ("PRODUCT_NAME", "App")].into_iter().map(|(k, v)| (k.into(), v.into())).collect();
        let launch = options.run_launch(Some(&settings));
        assert_eq!(launch.arguments, ["-com.apple.CoreData.SQLDebug", "1", "--name", "two words", "it's", "App.log"]);
        assert_eq!(
            launch.environment,
            [("ROOT".to_string(), "/src/x".to_string()), ("NSZombieEnabled".to_string(), "YES".to_string())]
        );
        assert_eq!(launch.working_directory, Some(PathBuf::from("/src/run")));
        // Tests use the Run action's lists by default, with their own diagnostics.
        assert_eq!(options.test_launch().arguments.len(), 6);
        assert!(options.test_launch().environment.iter().all(|(k, _)| k != "NSZombieEnabled"));

        let mut launch = Launch::default();
        launch.insert_library(Path::new("/a.dylib"));
        launch.insert_library(Path::new("/b.dylib"));
        assert_eq!(launch.environment, [("DYLD_INSERT_LIBRARIES".to_string(), "/a.dylib:/b.dylib".to_string())]);
    }

    #[test]
    fn sanitizer_flags() {
        let d = Diagnostics { address_sanitizer: true, undefined_behavior_sanitizer: true, ..Diagnostics::default() };
        assert_eq!(d.xcodebuild_args(), ["-enableAddressSanitizer", "YES", "-enableUndefinedBehaviorSanitizer", "YES"]);
        assert_eq!(d.swiftpm_args(), ["--sanitize=address", "--sanitize=undefined"]);
    }

    #[test]
    fn session_options_round_trip() {
        let mut options = SchemeOptions::default();
        options.run.environment.push(Variable { key: "A".into(), value: "b".into(), enabled: false });
        let json = serde_json::to_string(&options).unwrap();
        assert_eq!(serde_json::from_str::<SchemeOptions>(&json).unwrap(), options);
        // Older or partial session entries fill in the defaults.
        let partial: SchemeOptions = serde_json::from_str(r#"{"run":{"configuration":"Release"}}"#).unwrap();
        assert_eq!(partial.run.configuration, "Release");
        assert!(partial.debug_executable && partial.run.diagnostics.main_thread_checker);
    }
}

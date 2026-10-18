//! File-based configuration, as in Zed: `~/.config/zcode/settings.json`
//! (a project's `.zcode/settings.json` over it), `keymap.json` for key
//! bindings and `themes/*.json`, all JSON with comments, read again whenever
//! they change. A file that doesn't parse keeps the settings it gave last
//! and says where it went wrong. Xcode's themes and key bindings import
//! into them (`xcode_theme.rs`, `xcode_keys.rs`).
//!
//! Startup reads nothing on the main thread: `preload` reads the user's
//! files (and the theme they name) on a thread of its own as soon as the
//! process starts, and the first window takes what it read. A project's file
//! is read when its window opens; files are then polled about once a second
//! off the main thread (`watch`).

pub mod content;
pub mod jsonc;
pub mod keymap;
pub mod themes;
pub mod xcode_keys;
pub mod xcode_theme;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gpui_kit::{App, BorrowAppContext as _, Global};
use serde_json::Value;

pub use content::{
    ClaudeSettings, DEFAULT_FONT_SIZE, EditPredictionSettings, Formatter, LspBinary, LspServerSettings, PredictionProvider, Secret, Settings,
    SettingsContent, SourcekitBuildSettings, ThemeMode, ThemeSelection,
};

pub fn settings_path() -> PathBuf {
    crate::paths::config_dir().join("settings.json")
}

pub fn keymap_path() -> PathBuf {
    crate::paths::config_dir().join("keymap.json")
}

pub fn themes_dir() -> PathBuf {
    crate::paths::config_dir().join("themes")
}

pub fn project_settings_path(root: &Path) -> PathBuf {
    root.join(".zcode").join("settings.json")
}

/// What a new settings file starts as.
pub const SETTINGS_TEMPLATE: &str = "// Your zcode settings: JSON with comments, in Zed's format. They apply as\n// soon as the file is saved; zcode ▸ Open Default Settings lists them all.\n{\n}\n";
pub const PROJECT_SETTINGS_TEMPLATE: &str = "// This project's zcode settings, over yours (zcode ▸ Settings…). zcode ▸\n// Open Default Settings lists them all.\n{\n}\n";

/// When a file was last changed, and its size: enough to tell it changed.
pub type Stamp = Option<(SystemTime, u64)>;

pub fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// One file: its last good content and what's wrong with it now.
#[derive(Clone, Debug, Default)]
pub struct FileState<T> {
    pub path: PathBuf,
    pub stamp: Stamp,
    pub text: Option<String>,
    pub content: T,
    /// Why the file's current text isn't used (`content` is from before).
    pub error: Option<String>,
    /// Keys it has that zcode doesn't know, bindings it couldn't make.
    pub warnings: Vec<String>,
}

/// What can be read from a file, and the warnings to show about it.
pub trait Content: Default + Clone {
    fn read(text: &str, project: bool) -> Result<(Self, Vec<String>), jsonc::ParseError>;
}

impl Content for SettingsContent {
    fn read(text: &str, project: bool) -> Result<(Self, Vec<String>), jsonc::ParseError> {
        let content: SettingsContent = jsonc::parse(text)?;
        let value = jsonc::parse_value(text)?;
        if !value.is_object() && !value.is_null() {
            return Err(jsonc::ParseError { line: 1, column: 1, message: "expected an object ({ … })".into() });
        }
        let warnings = content::unknown_keys(&value, project);
        Ok((content, warnings))
    }
}

impl<T: Content> FileState<T> {
    pub fn new(path: PathBuf) -> Self {
        Self { path, stamp: None, text: None, content: T::default(), error: None, warnings: vec![] }
    }

    /// Reads the file (off the main thread when it can be).
    pub fn read(path: PathBuf, project: bool) -> Self {
        let mut state = Self::new(path);
        let stamp = stamp(&state.path);
        let text = std::fs::read_to_string(&state.path).ok();
        state.update(stamp, text, project);
        state
    }

    /// Takes the file's new text; returns whether anything changed.
    pub fn update(&mut self, stamp: Stamp, text: Option<String>, project: bool) -> bool {
        self.stamp = stamp;
        if text == self.text && (text.is_some() || self.error.is_none()) {
            return false;
        }
        self.text = text;
        match self.text.as_deref().map(|text| T::read(text, project)).unwrap_or_else(|| Ok((T::default(), vec![]))) {
            Ok((content, warnings)) => {
                self.content = content;
                self.warnings = warnings;
                self.error = None;
            }
            Err(err) => {
                // Whatever it said about the text before doesn't matter now.
                self.warnings.clear();
                self.error = Some(err.to_string());
            }
        }
        true
    }

    /// The problems to show, each saying which file.
    pub fn messages(&self) -> Vec<Problem> {
        let name = label(&self.path);
        let mut messages: Vec<Problem> = self.error.iter().map(|e| Problem { error: true, message: format!("{name}: {e}") }).collect();
        messages.extend(self.warnings.iter().map(|w| Problem { error: false, message: format!("{name}: {w}") }));
        messages
    }
}

/// A file as messages name it: `settings.json`, `themes/Dusk.json` in the
/// configuration folder, the path elsewhere.
pub fn label(path: &Path) -> String {
    match path.strip_prefix(crate::paths::config_dir()) {
        Ok(rest) => rest.display().to_string(),
        Err(_) => crate::paths::display_path(path),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub error: bool,
    pub message: String,
}

// ---------------------------------------------------------------------------
// The store: every file read, and the settings they make.

pub struct SettingsStore {
    pub user: FileState<SettingsContent>,
    pub keymap: FileState<keymap::KeymapContent>,
    pub projects: HashMap<PathBuf, FileState<SettingsContent>>,
    pub themes: themes::ThemeStore,
    user_settings: Settings,
    /// Problems with the keymap's bindings found while binding them.
    pub keymap_problems: Vec<String>,
    /// Bumped by every change, so windows can tell what they've applied.
    pub generation: u64,
}

impl Global for SettingsStore {}

impl Settings {
    /// The user's settings (no project's): what app-wide things follow.
    pub fn get(cx: &App) -> Settings {
        cx.try_global::<SettingsStore>().map(|s| s.user_settings.clone()).unwrap_or_default()
    }

    /// The settings for a project's window: the user's, then the project's.
    pub fn for_project(root: &Path, cx: &App) -> Settings {
        let Some(store) = cx.try_global::<SettingsStore>() else { return Settings::default() };
        match store.projects.get(root) {
            Some(project) => store.user_settings.clone().merged(&project.content, true),
            None => store.user_settings.clone(),
        }
    }
}

impl SettingsStore {
    fn resolve(&mut self) {
        self.user_settings = Settings::default().merged(&self.user.content, false);
        self.generation += 1;
    }

    /// Everything wrong with the files a window uses.
    pub fn problems(&self, root: Option<&Path>) -> Vec<Problem> {
        let mut problems = self.user.messages();
        if let Some(project) = root.and_then(|root| self.projects.get(root)) {
            problems.extend(project.messages());
        }
        problems.extend(self.keymap.messages());
        let keymap = label(&self.keymap.path);
        problems.extend(self.keymap_problems.iter().map(|p| Problem { error: false, message: format!("{keymap}: {p}") }));
        problems.extend(self.themes.problems());
        problems
    }
}

// ---------------------------------------------------------------------------
// Startup

/// What `preload`'s thread read.
pub struct Preloaded {
    user: FileState<SettingsContent>,
    keymap: FileState<keymap::KeymapContent>,
    themes: themes::Loaded,
}

impl Preloaded {
    fn read() -> Self {
        migrate_session(&settings_path(), &crate::paths::support_dir().join("session.json"));
        let user = FileState::<SettingsContent>::read(settings_path(), false);
        let keymap = FileState::read(keymap_path(), false);
        let selection = Settings::default().merged(&user.content, false).theme;
        let themes = themes::Loaded::read(&themes_dir(), &selection.names());
        Self { user, keymap, themes }
    }
}

static PRELOAD: parking_lot::Mutex<Option<std::thread::JoinHandle<Preloaded>>> = parking_lot::Mutex::new(None);

/// Starts reading the settings files on a thread of their own, at the very
/// start of the process, so the first window doesn't wait for them.
pub fn preload() {
    let thread = std::thread::Builder::new().name("zcode-settings".into()).spawn(Preloaded::read).ok();
    *PRELOAD.lock() = thread;
}

/// Makes the store from what `preload` read (reading it here if it didn't
/// run): settings, key bindings over the defaults, the theme.
pub fn init(cx: &mut App) {
    let preloaded = PRELOAD.lock().take().and_then(|thread| thread.join().ok()).unwrap_or_else(Preloaded::read);
    let mut store = SettingsStore {
        user: preloaded.user,
        keymap: preloaded.keymap,
        projects: HashMap::new(),
        themes: themes::ThemeStore::new(preloaded.themes),
        user_settings: Settings::default(),
        keymap_problems: vec![],
        generation: 0,
    };
    store.resolve();
    cx.set_global(store);
}

/// Starts polling the files for changes, a while after startup.
pub fn watch(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(WATCH_INTERVAL_MS)).await;
            let watched = cx.update(|cx| watched_files(cx));
            let changed = cx
                .background_executor()
                .spawn(async move {
                    watched
                        .into_iter()
                        .filter_map(|(path, was)| {
                            let now = stamp(&path);
                            (now != was).then(|| (path.clone(), now, std::fs::read_to_string(&path).ok()))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            if !changed.is_empty() {
                cx.update(|cx| apply_changes(changed, cx));
            }
        }
    })
    .detach();
}

pub const WATCH_INTERVAL_MS: u64 = 1000;

fn watched_files(cx: &App) -> Vec<(PathBuf, Stamp)> {
    let store = cx.global::<SettingsStore>();
    let mut files = vec![(store.user.path.clone(), store.user.stamp), (store.keymap.path.clone(), store.keymap.stamp)];
    files.extend(store.projects.values().map(|p| (p.path.clone(), p.stamp)));
    files.extend(store.themes.watched());
    files
}

/// Reads every file again now (the control channel's `settings reload`).
pub fn reload(cx: &mut App) {
    let changed = watched_files(cx)
        .into_iter()
        .filter_map(|(path, was)| {
            let now = stamp(&path);
            (now != was).then(|| (path.clone(), now, std::fs::read_to_string(&path).ok()))
        })
        .collect();
    apply_changes(changed, cx);
}

fn apply_changes(changed: Vec<(PathBuf, Stamp, Option<String>)>, cx: &mut App) {
    let mut keymap_changed = false;
    let mut theme_files_changed = false;
    let mut any = false;
    cx.update_global::<SettingsStore, _>(|store, _| {
        for (path, stamp, text) in changed {
            if path == store.user.path {
                any |= store.user.update(stamp, text, false);
            } else if path == store.keymap.path {
                let updated = store.keymap.update(stamp, text, false);
                keymap_changed |= updated;
                any |= updated;
            } else if let Some(project) = store.projects.values_mut().find(|p| p.path == path) {
                any |= project.update(stamp, text, true);
            } else {
                theme_files_changed = true;
            }
        }
        if any {
            store.resolve();
        }
    });
    if keymap_changed {
        keymap::rebind(cx);
    }
    // A theme's file, the folder, or the theme the settings name changed.
    themes::refresh(theme_files_changed, cx);
    if any || keymap_changed {
        crate::actions::refresh_menus(cx);
    }
}

/// Reads a project's settings file as its window opens (one file; usually
/// it doesn't exist).
pub fn open_project(root: &Path, cx: &mut App) {
    if !cx.has_global::<SettingsStore>() || cx.global::<SettingsStore>().projects.contains_key(root) {
        return;
    }
    let state = FileState::read(project_settings_path(root), true);
    cx.update_global::<SettingsStore, _>(|store, _| {
        store.projects.insert(root.to_path_buf(), state);
        store.generation += 1;
    });
}

// ---------------------------------------------------------------------------
// Changing a setting from zcode (menus, the theme picker, imports).

/// Sets `path` (`["git", "inline_blame", "enabled"]`) to `value` in the
/// project's file when it sets that already, otherwise in the user's, and
/// applies it at once.
pub fn set(root: Option<&Path>, path: &[&str], value: Value, cx: &mut App) -> Result<(), String> {
    let store = cx.global::<SettingsStore>();
    let project = root.and_then(|root| store.projects.get(root)).filter(|project| {
        let set_there = project.text.as_deref().and_then(|t| jsonc::parse_value(t).ok());
        set_there.is_some_and(|v| v.pointer(&format!("/{}", path.join("/"))).is_some())
    });
    let (file, template, is_project) = match project {
        Some(project) => (project.path.clone(), PROJECT_SETTINGS_TEMPLATE, true),
        None => (store.user.path.clone(), SETTINGS_TEMPLATE, false),
    };
    let text = std::fs::read_to_string(&file).unwrap_or_else(|_| template.to_string());
    let edited = jsonc::set(&text, path, &value)
        .map_err(|err| format!("Couldn’t change {}: fix {err} first", crate::paths::display_path(&file)))?;
    write_file(&file, &edited)?;
    let now = stamp(&file);
    cx.update_global::<SettingsStore, _>(|store, _| {
        let state = if is_project { store.projects.values_mut().find(|p| p.path == file) } else { Some(&mut store.user) };
        if let Some(state) = state {
            state.update(now, Some(edited), is_project);
        }
        store.resolve();
    });
    themes::refresh(false, cx);
    crate::actions::refresh_menus(cx);
    Ok(())
}

pub fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Couldn’t create {}: {e}", crate::paths::display_path(dir)))?;
    }
    std::fs::write(path, text).map_err(|e| format!("Couldn’t write {}: {e}", crate::paths::display_path(path)))
}

/// Creates a settings file from its template if there isn't one, for
/// Settings… to open.
pub fn ensure_file(path: &Path, template: &str) -> Result<(), String> {
    if path.exists() { Ok(()) } else { write_file(path, template) }
}

/// The settings zcode kept in the session before this file: moved into
/// settings.json once (values that differ from the defaults), then taken
/// out of the session. A settings file that doesn't parse is left alone,
/// and the values wait in the session for the next launch.
fn migrate_session(settings: &Path, session: &Path) {
    const MOVED: &[(&str, &[&str])] = &[
        ("font_size", &["buffer_font_size"]),
        ("inlay_hints", &["inlay_hints", "enabled"]),
        ("vim_mode", &["vim_mode"]),
        ("inline_blame", &["git", "inline_blame", "enabled"]),
        ("show_authors", &["git", "show_authors"]),
        ("side_by_side_comparison", &["git", "side_by_side_comparison"]),
    ];
    let Ok(bytes) = std::fs::read(session) else { return };
    let Ok(Value::Object(mut session_value)) = serde_json::from_slice::<Value>(&bytes) else { return };
    if !MOVED.iter().any(|(key, _)| session_value.contains_key(*key)) {
        return;
    }
    let original = std::fs::read_to_string(settings).ok();
    let mut text = original.clone().unwrap_or_else(|| SETTINGS_TEMPLATE.to_string());
    let Ok(current) = jsonc::parse_value(&text) else { return };
    let defaults = jsonc::parse_value(content::DEFAULT_SETTINGS).unwrap_or_default();
    let mut moved = 0;
    for (key, path) in MOVED {
        let Some(value) = session_value.get(*key).filter(|v| !v.is_null()) else { continue };
        let pointer = format!("/{}", path.join("/"));
        let numbers_equal = |a: &Value, b: &Value| a.as_f64().zip(b.as_f64()).is_some_and(|(a, b)| a == b);
        let is_default = defaults.pointer(&pointer).is_some_and(|d| d == value || numbers_equal(d, value));
        if is_default || current.pointer(&pointer).is_some() {
            continue;
        }
        match jsonc::set(&text, path, value) {
            Ok(edited) => text = edited,
            Err(_) => return,
        }
        moved += 1;
    }
    if moved > 0 && write_file(settings, &text).is_err() {
        return;
    }
    for (key, _) in MOVED {
        session_value.remove(*key);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&Value::Object(session_value)) {
        std::fs::write(session, bytes).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_toggles_move_into_the_settings_file_once() {
        let dir = std::env::temp_dir().join(format!("zcode-settings-migrate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = dir.join("settings.json");
        let session = dir.join("session.json");
        std::fs::write(&session, r#"{ "recent": ["/a"], "font_size": 15.0, "inline_blame": true, "inlay_hints": true, "vim_mode": false }"#).unwrap();
        migrate_session(&settings, &session);
        let written = std::fs::read_to_string(&settings).unwrap();
        assert!(written.starts_with("// Your zcode settings"), "{written}");
        assert_eq!(jsonc::parse_value(&written).unwrap(), serde_json::json!({ "buffer_font_size": 15.0, "inlay_hints": { "enabled": true } }));
        let session_now: Value = serde_json::from_slice(&std::fs::read(&session).unwrap()).unwrap();
        assert_eq!(session_now, serde_json::json!({ "recent": ["/a"] }));
        // Once: a second launch changes nothing, and the file wins over a session
        // an older zcode wrote again.
        std::fs::write(&session, r#"{ "font_size": 20.0 }"#).unwrap();
        migrate_session(&settings, &session);
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), written);
        // A broken settings file is left for the user to fix; the session keeps the values.
        std::fs::write(&settings, "{ \"tab_size\": ").unwrap();
        std::fs::write(&session, r#"{ "vim_mode": true }"#).unwrap();
        migrate_session(&settings, &session);
        assert_eq!(std::fs::read_to_string(&session).unwrap(), r#"{ "vim_mode": true }"#);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_that_stops_parsing_keeps_what_it_gave() {
        let mut state = FileState::<SettingsContent>::new(PathBuf::from("/tmp/settings.json"));
        assert!(state.update(None, Some(r#"{ "tab_size": 2, "tab_sise": 3 }"#.into()), false));
        assert_eq!(state.content.tab_size, Some(2));
        assert_eq!(state.warnings, ["unknown setting “tab_sise”"]);
        assert!(state.update(None, Some("{ \"tab_size\": 3,, }".into()), false));
        assert_eq!(state.content.tab_size, Some(2));
        assert!(state.error.as_deref().is_some_and(|e| e.starts_with("line 1, column")), "{:?}", state.error);
        assert!(!state.update(None, Some("{ \"tab_size\": 3,, }".into()), false), "the same text again is no change");
        assert!(state.messages()[0].error && state.messages()[0].message.starts_with("/tmp/settings.json: line 1"));
        assert!(state.warnings.is_empty(), "the old text's warnings go with it");
        assert!(state.update(None, None, false));
        assert_eq!(state.content, SettingsContent::default());
    }
}

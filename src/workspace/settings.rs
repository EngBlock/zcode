//! The settings in a project window: the user's with the project's over
//! them (`crate::settings`), applied to its editors, terminals and git
//! views whenever a file changes; the Editor menu's toggles, which write
//! the setting; the files' problems as notifications; and the zcode menu's
//! commands (Settings…, Key Bindings…, Select Theme…, the Xcode imports,
//! the default files).

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;
use serde_json::{Value, json};
use zcode_editor::{EditorState, TabSize};

use super::{Item, Workspace};
use crate::actions::*;
use crate::fuzzy::fuzzy_match;
use crate::picker::{Picker, PickerDelegate, PickerItem};
use crate::settings::{self, Problem, Settings, SettingsStore, keymap, themes, xcode_keys, xcode_theme};

/// What a window last applied.
pub(super) struct SettingsState {
    pub current: Settings,
    generation: u64,
    problems: Vec<Problem>,
    theme: Option<String>,
}

impl SettingsState {
    pub fn new(current: Settings) -> Self {
        Self { current, generation: 0, problems: vec![], theme: None }
    }
}

/// How the window's editors are made, from its settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct EditorConfig {
    pub soft_wrap: bool,
    pub tab_size: usize,
    pub hard_tabs: bool,
    pub line_numbers: bool,
    pub indent_guides: bool,
    pub show_whitespaces: bool,
}

impl EditorConfig {
    pub fn tab(&self) -> TabSize {
        TabSize { tab_size: self.tab_size, hard_tabs: self.hard_tabs }
    }

    /// Applies to an editor made before the settings changed.
    pub fn apply(&self, editor: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
        let config = *self;
        editor.update(cx, |state, cx| {
            state.set_tab_size(config.tab(), cx);
            state.set_line_number(config.line_numbers, window, cx);
            state.set_indent_guides(config.indent_guides, window, cx);
            state.set_show_whitespaces(config.show_whitespaces, window, cx);
        });
    }
}

/// Problems with the settings files, one notification each.
struct SettingsProblem;

impl Workspace {
    pub(super) fn editor_config(&self) -> EditorConfig {
        let s = &self.settings.current;
        EditorConfig {
            soft_wrap: s.soft_wrap,
            tab_size: s.tab_size as usize,
            hard_tabs: s.hard_tabs,
            line_numbers: s.line_numbers,
            indent_guides: s.indent_guides,
            show_whitespaces: s.show_whitespaces,
        }
    }

    /// The store changed (a file was saved, a toggle written, a theme
    /// read): applies what changed for this window.
    pub(super) fn settings_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = cx.try_global::<SettingsStore>() else { return };
        if store.generation == self.settings.generation {
            return;
        }
        self.settings.generation = store.generation;
        let theme = store.themes.applied.clone();
        let problems = store.problems(Some(&self.project.root));
        let new = Settings::for_project(&self.project.root, cx);
        let old = std::mem::replace(&mut self.settings.current, new.clone());
        if old != new {
            self.apply_settings(&old, window, cx);
            // The menu bar shows the front window's.
            if cx.active_window().is_none_or(|w| w == window.window_handle()) {
                crate::actions::set_menu_settings(new, cx);
            }
        }
        if theme != self.settings.theme {
            // Semantic token colors come from the theme.
            self.settings.theme = theme;
            self.refresh_semantic_tokens(cx);
        }
        if problems != self.settings.problems {
            self.show_settings_problems(problems, window, cx);
        }
        cx.notify();
    }

    fn apply_settings(&mut self, old: &Settings, window: &mut Window, cx: &mut Context<Self>) {
        let new = self.settings.current.clone();
        self.font_size = new.buffer_font_size;
        let config = self.editor_config();
        let old_config = EditorConfig {
            soft_wrap: old.soft_wrap,
            tab_size: old.tab_size as usize,
            hard_tabs: old.hard_tabs,
            line_numbers: old.line_numbers,
            indent_guides: old.indent_guides,
            show_whitespaces: old.show_whitespaces,
        };
        if config != old_config {
            for editor in self.all_editors(cx) {
                config.apply(&editor, window, cx);
            }
        }
        if new.soft_wrap != self.soft_wrap {
            self.soft_wrap = new.soft_wrap;
            self.apply_soft_wrap(window, cx);
        }
        if (&new.agent_servers, &new.default_agent, new.always_allow_tool_actions) != (&old.agent_servers, &old.default_agent, old.always_allow_tool_actions) {
            self.agent_settings_changed(cx);
        }
        if new.inlay_hints != self.inlay_hints.enabled {
            self.set_inlay_hints(new.inlay_hints, cx);
        }
        if new.inline_blame != self.git.inline_blame {
            self.set_inline_blame(new.inline_blame, cx);
        }
        if new.show_authors != self.git.show_authors {
            self.set_authors(new.show_authors, cx);
        }
        if new.side_by_side_comparison != self.git.side_by_side {
            self.set_comparison(new.side_by_side_comparison, cx);
        }
        if new.vim_mode != old.vim_mode {
            self.apply_vim_mode(cx);
        }
        self.apply_prediction_settings(cx);
    }

    /// Every editor of the window: files' buffers and every tab's views.
    fn all_editors(&self, cx: &App) -> Vec<Entity<EditorState>> {
        let mut editors: Vec<Entity<EditorState>> = self.buffers.iter().map(|b| b.editor.clone()).collect();
        for pane in &self.panes {
            for item in &pane.items {
                match item {
                    Item::Editor(item) => editors.push(item.editor.clone()),
                    Item::MultiBuffer(item) => editors.push(item.host.clone()),
                    Item::Diff(diff) => {
                        let diff = diff.view.read(cx);
                        editors.extend([diff.current.clone(), diff.base.clone()]);
                    }
                    Item::Terminal(_) | Item::ProjectSettings(_) => {}
                }
            }
        }
        editors.sort_by_key(|e| e.entity_id());
        editors.dedup_by_key(|e| e.entity_id());
        editors
    }

    fn show_settings_problems(&mut self, problems: Vec<Problem>, window: &mut Window, cx: &mut Context<Self>) {
        window.remove_notification::<SettingsProblem>(cx);
        for (ix, problem) in problems.iter().enumerate() {
            let note = if problem.error { Notification::error(problem.message.clone()) } else { Notification::warning(problem.message.clone()) };
            window.push_notification(note.id1::<SettingsProblem>(ix).autohide(false), cx);
        }
        for problem in &problems {
            log::warn!("{}", problem.message);
        }
        self.settings.problems = problems;
    }

    /// Writes a setting (the Editor menu's toggles, ⌘+), in the project's
    /// file when it sets that, otherwise in the user's.
    pub(super) fn set_setting(&mut self, path: &[&str], value: Value, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root.clone();
        if let Err(err) = settings::set(Some(&root), path, value, cx) {
            self.notify_error(err, window, cx);
        }
    }

    /// Turns a setting the other way: `on` reads it from the settings as
    /// they are now (written a moment ago, maybe not applied here yet).
    pub(super) fn toggle_setting(&mut self, path: &[&str], on: impl Fn(&Settings) -> bool, window: &mut Window, cx: &mut Context<Self>) {
        let now = on(&self.latest_settings(cx));
        self.set_setting(path, json!(!now), window, cx);
    }

    /// The settings files' values now, before this window has applied them.
    pub(super) fn latest_settings(&self, cx: &App) -> Settings {
        Settings::for_project(&self.project.root, cx)
    }

    // ---------------------------------------------------------------------
    // The zcode menu

    /// Opens a settings file in a tab, creating it from its template.
    fn open_config_file(&mut self, path: PathBuf, template: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(err) = settings::ensure_file(&path, template) {
            self.notify_error(err, window, cx);
            return;
        }
        self.open_file(path, None, true, window, cx);
    }

    pub(super) fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.open_config_file(settings::settings_path(), settings::SETTINGS_TEMPLATE, window, cx);
    }

    pub(super) fn open_keymap(&mut self, _: &OpenKeymap, window: &mut Window, cx: &mut Context<Self>) {
        self.open_config_file(settings::keymap_path(), keymap::KEYMAP_TEMPLATE, window, cx);
    }

    pub(super) fn open_project_settings_file(&mut self, _: &OpenProjectSettingsFile, window: &mut Window, cx: &mut Context<Self>) {
        let path = settings::project_settings_path(&self.project.root);
        self.open_config_file(path, settings::PROJECT_SETTINGS_TEMPLATE, window, cx);
    }

    pub(super) fn open_default_settings(&mut self, _: &OpenDefaultSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.open_generated("default-settings.json", settings::content::DEFAULT_SETTINGS.to_string(), window, cx);
    }

    pub(super) fn open_default_keymap(&mut self, _: &OpenDefaultKeymap, window: &mut Window, cx: &mut Context<Self>) {
        let text = keymap::default_keymap_text(cx);
        self.open_generated("default-keymap.json", text, window, cx);
    }

    /// A file zcode writes to show (read-only), in its cache.
    fn open_generated(&mut self, name: &str, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let path = generated_dir().join(name);
        if let Err(err) = settings::write_file(&path, &text) {
            self.notify_error(err, window, cx);
            return;
        }
        // An open tab shows the new text.
        if self.buffer(&path).is_some() {
            self.reload_from_disk(&path, cx);
        }
        self.open_file(path, None, true, window, cx);
    }

    pub(super) fn select_theme(&mut self, _: &SelectTheme, window: &mut Window, cx: &mut Context<Self>) {
        let (themes, errors) = themes::all(cx);
        for error in errors {
            self.notify_error(error, window, cx);
        }
        let delegate = ThemeDelegate::new(cx.weak_entity(), themes, window, cx);
        let picker = cx.new(|cx| Picker::new(delegate, window, cx));
        self.show_modal(picker, window, cx);
    }

    /// Chooses a theme as Select Theme… does.
    pub(super) fn choose_theme(&mut self, name: &str, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        let current = Settings::get(cx).theme;
        let system_dark = matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
        let value = themes::chosen(&current, name, dark, system_dark);
        if let Err(err) = settings::set(None, &["theme"], value, cx) {
            self.notify_error(err, window, cx);
        }
    }

    pub(super) fn import_xcode_theme(&mut self, _: &ImportXcodeTheme, window: &mut Window, cx: &mut Context<Self>) {
        let found = xcode_theme::find();
        let delegate = ImportDelegate::new(
            cx.weak_entity(),
            "Import Xcode Theme",
            found.iter().map(|t| (t.name.clone(), t.path.clone(), if t.built_in { "Xcode" } else { "Custom" })).collect(),
            ImportKind::Theme,
        );
        let picker = cx.new(|cx| Picker::new(delegate, window, cx));
        self.show_modal(picker, window, cx);
    }

    pub(super) fn import_xcode_key_bindings(&mut self, _: &ImportXcodeKeyBindings, window: &mut Window, cx: &mut Context<Self>) {
        let found = xcode_keys::find();
        let delegate = ImportDelegate::new(
            cx.weak_entity(),
            "Import Xcode Key Bindings",
            found.iter().map(|p| (p.file_stem().unwrap_or_default().to_string_lossy().to_string(), p.clone(), "Key Bindings")).collect(),
            ImportKind::KeyBindings,
        );
        let picker = cx.new(|cx| Picker::new(delegate, window, cx));
        self.show_modal(picker, window, cx);
    }

    /// Imports an Xcode theme and shows it.
    pub(super) fn import_theme_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        let xcode_name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let name = xcode_theme::theme_name(&xcode_name);
        let installed = |family: &str| themes::font_installed(family, cx);
        let converted = xcode_theme::convert(path, &name, &installed).map_err(|e| format!("Couldn’t import {}: {e:#}", path.display()))?;
        themes::write_set(&converted.set, &name)?;
        self.choose_theme(&name, converted.dark, window, cx);
        let mut message = format!("Imported Xcode’s “{xcode_name}” as the theme “{name}”.");
        for note in &converted.notes {
            message.push_str(&format!(" Note: {note}."));
        }
        window.push_notification(Notification::success(message.clone()), cx);
        Ok(message)
    }

    /// Imports an Xcode key bindings set into keymap.json.
    pub(super) fn import_key_bindings_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        let defaults: Vec<(String, String)> = keymap::defaults(cx)
            .iter()
            .map(|b| (b.keystrokes().iter().map(|k| k.unparse()).collect::<Vec<_>>().join(" "), b.action().name().to_string()))
            .collect();
        let imported = xcode_keys::convert(path, &defaults).map_err(|e| format!("Couldn’t import {}: {e:#}", path.display()))?;
        let marker = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let keymap_path = settings::keymap_path();
        let text = std::fs::read_to_string(&keymap_path).unwrap_or_else(|_| keymap::KEYMAP_TEMPLATE.to_string());
        let merged = xcode_keys::merge_into(&text, &imported, &marker)
            .map_err(|e| format!("Couldn’t import into {}: fix {e} first", crate::paths::display_path(&keymap_path)))?;
        settings::write_file(&keymap_path, &merged)?;
        settings::reload(cx);
        let mut message = format!("Imported {} key bindings from Xcode’s {marker}.", imported.bound);
        if !imported.skipped.is_empty() {
            message.push_str(&format!(" zcode has no command for {}: {}.", imported.skipped.len(), imported.skipped.join(", ")));
        }
        window.push_notification(Notification::success(message.clone()), cx);
        Ok(message)
    }

    // ---------------------------------------------------------------------
    // The control channel

    /// `settings <command>`: paths, set <path.to.key> <json>, reload,
    /// theme <name>, import-theme <path>, import-keys <path>, open
    /// settings|keymap|project|default-settings|default-keymap.
    pub fn settings_command(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        let (command, rest) = arg.split_once(' ').unwrap_or((arg, ""));
        let rest = rest.trim();
        match command {
            "paths" => Ok(json!({
                "config_dir": crate::paths::config_dir(),
                "settings": settings::settings_path(),
                "keymap": settings::keymap_path(),
                "themes": settings::themes_dir(),
                "project": settings::project_settings_path(&self.project.root),
                "session": crate::paths::support_dir().join("session.json"),
            })
            .to_string()),
            "set" => {
                let (key, value) = rest.split_once(' ').ok_or("usage: settings set <a.b.c> <json>")?;
                let value: Value = serde_json::from_str(value).map_err(|e| e.to_string())?;
                let path: Vec<&str> = key.split('.').collect();
                let root = self.project.root.clone();
                settings::set(Some(&root), &path, value, cx)?;
                Ok(String::new())
            }
            "reload" => {
                settings::reload(cx);
                Ok(String::new())
            }
            "theme" => {
                let (all, _) = themes::all(cx);
                let (name, dark) = all.into_iter().find(|(name, _)| name == rest).ok_or(format!("no theme {rest}"))?;
                self.choose_theme(&name, dark, window, cx);
                Ok(String::new())
            }
            "import-theme" => self.import_theme_file(Path::new(rest), window, cx),
            "import-keys" => self.import_key_bindings_file(Path::new(rest), window, cx),
            "xcode-themes" => Ok(json!({ "themes": xcode_theme::find().iter().map(|t| json!({ "name": t.name, "built_in": t.built_in })).collect::<Vec<_>>() }).to_string()),
            "xcode-key-sets" => Ok(json!({ "sets": xcode_keys::find() }).to_string()),
            "open" => {
                match rest {
                    "settings" => self.open_settings(&OpenSettings, window, cx),
                    "keymap" => self.open_keymap(&OpenKeymap, window, cx),
                    "project" => self.open_project_settings_file(&OpenProjectSettingsFile, window, cx),
                    "default-settings" => self.open_default_settings(&OpenDefaultSettings, window, cx),
                    "default-keymap" => self.open_default_keymap(&OpenDefaultKeymap, window, cx),
                    other => return Err(format!("unknown settings file {other}")),
                }
                Ok(String::new())
            }
            other => Err(format!("unknown settings command {other}")),
        }
    }

    /// For `dump`.
    pub(super) fn settings_summary(&self, window: &Window, cx: &App) -> Value {
        let s = &self.settings.current;
        let theme = gpui_kit::component::Theme::global(cx);
        let editor = self.active_editor().map(|e| {
            let state = e.read(cx);
            let tab = state.current_tab_size();
            json!({
                "tab_size": tab.map(|t| t.tab_size),
                "hard_tabs": tab.map(|t| t.hard_tabs),
                "soft_wrap": state.is_soft_wrapped(),
                "line_numbers": state.shows_line_numbers(),
                "show_whitespaces": state.shows_whitespaces(),
            })
        });
        json!({
            "buffer_font_size": s.buffer_font_size,
            "buffer_font_family": s.buffer_font_family,
            "tab_size": s.tab_size,
            "hard_tabs": s.hard_tabs,
            "soft_wrap": s.soft_wrap,
            "show_whitespaces": s.show_whitespaces,
            "line_numbers": s.line_numbers,
            "indent_guides": s.indent_guides,
            "format_on_save": s.format_on_save,
            "remove_trailing_whitespace_on_save": s.remove_trailing_whitespace_on_save,
            "ensure_final_newline_on_save": s.ensure_final_newline_on_save,
            "inlay_hints": s.inlay_hints,
            "vim_mode": s.vim_mode,
            "inline_blame": s.inline_blame,
            "show_authors": s.show_authors,
            "side_by_side_comparison": s.side_by_side_comparison,
            "terminal_shell": s.terminal_shell,
            "theme": {
                "name": cx.try_global::<SettingsStore>().and_then(|st| st.themes.applied.clone()),
                "mode": if theme.mode.is_dark() { "dark" } else { "light" },
                "appearance_dark": matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark),
                "editor_background": theme.highlight_theme.style.editor_background.map(hex),
                "keyword": theme.highlight_theme.style.syntax.keyword.and_then(|k| HighlightStyle::from(k).color).map(hex),
                "selection": hex(theme.selection),
                "mono_font": theme.mono_font_family.to_string(),
            },
            "active_editor": editor,
            "problems": self.settings.problems.iter().map(|p| json!({ "error": p.error, "message": p.message })).collect::<Vec<_>>(),
            "menu_checked": crate::actions::menu_checkmarks(cx),
            // The focused element's key contexts, outermost first (for keymap.json).
            "key_context": window.context_stack().iter().map(|c| format!("{c:?}")).collect::<Vec<_>>(),
            "user_bindings": cx.try_global::<SettingsStore>().map(|st| st.keymap.content.blocks.iter().map(|b| b.bindings.len()).sum::<usize>()),
        })
    }
}

/// `#rrggbbaa`, as theme files write colors.
fn hex(color: Hsla) -> String {
    let c = gpui_kit::Rgba::from(color);
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b), byte(c.a))
}

/// Where zcode writes the files it shows read-only (Open Default Settings).
pub fn generated_dir() -> PathBuf {
    crate::paths::cache_dir().join("generated")
}

pub fn is_generated(path: &Path) -> bool {
    path.starts_with(generated_dir())
}

/// Without a project window (the welcome window): the settings files open
/// in a window of their own.
pub fn register_global_settings_actions(cx: &mut App) {
    fn open(path: PathBuf, template: &'static str, cx: &mut App) {
        match settings::ensure_file(&path, template) {
            Ok(()) => crate::app::open_project(path, cx),
            Err(err) => log::warn!("{err}"),
        }
    }
    cx.on_action(|_: &OpenSettings, cx| open(settings::settings_path(), settings::SETTINGS_TEMPLATE, cx));
    cx.on_action(|_: &OpenKeymap, cx| open(settings::keymap_path(), keymap::KEYMAP_TEMPLATE, cx));
}

// ---------------------------------------------------------------------------
// Select Theme…

struct ThemeDelegate {
    workspace: WeakEntity<Workspace>,
    themes: Vec<(String, bool)>,
    labels: Vec<String>,
    current: Option<String>,
    items: Vec<PickerItem>,
    matches: Vec<usize>,
}

impl ThemeDelegate {
    fn new(workspace: WeakEntity<Workspace>, themes: Vec<(String, bool)>, _: &Window, cx: &App) -> Self {
        let current = cx.try_global::<SettingsStore>().and_then(|s| s.themes.applied.clone());
        Self { workspace, labels: themes.iter().map(|(n, _)| n.clone()).collect(), themes, current, items: vec![], matches: vec![] }
    }
}

impl PickerDelegate for ThemeDelegate {
    fn placeholder(&self) -> SharedString {
        format!("Select Theme (more in {})", crate::paths::display_path(&settings::themes_dir())).into()
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        let found = fuzzy_match(&query, &self.labels, false, 200);
        self.matches = found.iter().map(|m| m.index).collect();
        self.items = found
            .into_iter()
            .map(|m| {
                let (name, dark) = &self.themes[m.index];
                PickerItem {
                    label: name.clone().into(),
                    detail: Some(if *dark { "Dark" } else { "Light" }.into()),
                    icon: Some(if *dark { IconName::Moon } else { IconName::Sun }),
                    positions: m.positions,
                    checked: self.current.as_ref() == Some(name),
                    ..Default::default()
                }
            })
            .collect();
    }

    fn initial_selection(&self) -> usize {
        self.items.iter().position(|i| i.checked).unwrap_or(0)
    }

    fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let (name, dark) = self.themes[self.matches[index]].clone();
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace.update(cx, |ws, cx| ws.choose_theme(&name, dark, window, cx)).ok();
        });
        true
    }
}

// ---------------------------------------------------------------------------
// Import Xcode Theme… / Key Bindings…

#[derive(Clone, Copy)]
enum ImportKind {
    Theme,
    KeyBindings,
}

struct ImportDelegate {
    workspace: WeakEntity<Workspace>,
    title: &'static str,
    found: Vec<(String, PathBuf, &'static str)>,
    labels: Vec<String>,
    kind: ImportKind,
    items: Vec<PickerItem>,
    matches: Vec<usize>,
}

impl ImportDelegate {
    fn new(workspace: WeakEntity<Workspace>, title: &'static str, found: Vec<(String, PathBuf, &'static str)>, kind: ImportKind) -> Self {
        Self { workspace, title, labels: found.iter().map(|(n, ..)| n.clone()).collect(), found, kind, items: vec![], matches: vec![] }
    }
}

impl PickerDelegate for ImportDelegate {
    fn placeholder(&self) -> SharedString {
        self.title.into()
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn empty_message(&self) -> SharedString {
        match self.kind {
            ImportKind::Theme => "No Xcode themes found".into(),
            ImportKind::KeyBindings => format!("No key bindings in {}", crate::paths::display_path(&xcode_keys::bindings_dir())).into(),
        }
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        let found = fuzzy_match(&query, &self.labels, false, 200);
        self.matches = found.iter().map(|m| m.index).collect();
        self.items = found
            .into_iter()
            .map(|m| {
                let (name, path, detail) = &self.found[m.index];
                PickerItem {
                    label: name.clone().into(),
                    detail: Some(format!("{detail} · {}", crate::paths::display_path(path)).into()),
                    icon: Some(match self.kind {
                        ImportKind::Theme => IconName::Palette,
                        ImportKind::KeyBindings => IconName::Keyboard,
                    }),
                    positions: m.positions,
                    ..Default::default()
                }
            })
            .collect();
    }

    fn confirm(&mut self, index: usize, window: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let path = self.found[self.matches[index]].1.clone();
        let (workspace, kind) = (self.workspace.clone(), self.kind);
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |ws, cx| {
                    let result = match kind {
                        ImportKind::Theme => ws.import_theme_file(&path, window, cx),
                        ImportKind::KeyBindings => ws.import_key_bindings_file(&path, window, cx),
                    };
                    if let Err(err) = result {
                        ws.notify_error(err, window, cx);
                    }
                })
                .ok();
        });
        true
    }
}

/// How long a format-on-save waits for the language server before the
/// file is saved as it is.
pub(super) const FORMAT_ON_SAVE_TIMEOUT: Duration = Duration::from_millis(1500);

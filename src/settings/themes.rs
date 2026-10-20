//! Themes as JSON: gpui-component's theme files (a set with `themes`, each
//! with `mode`, UI `colors` and editor `highlight`, whose syntax part is
//! Zed's), and Zed's own theme files (`appearance` and `style`), in
//! `~/.config/zcode/themes`. The `theme` setting picks one, or one for each
//! appearance; `Default Light` and `Default Dark` are built in.
//!
//! Only the themes the settings name are read (at startup on the
//! settings' own thread); Select Theme… reads the folder when it opens.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeRegistry};
use gpui_kit::{App, BorrowAppContext as _, SharedString, Window, WindowAppearance};
use serde_json::{Value, json};

use super::{Problem, Settings, SettingsStore, Stamp, jsonc, stamp};

/// A theme read from a file.
#[derive(Clone, Debug)]
pub struct ThemeFile {
    pub config: ThemeConfig,
    pub path: PathBuf,
}

/// Themes read from the folder, and what was wrong with the files.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    pub themes: Vec<ThemeFile>,
    pub errors: Vec<String>,
    /// The files read and the folder, to tell when they change.
    pub stamps: Vec<(PathBuf, Stamp)>,
    /// The names asked for.
    pub names: Vec<String>,
}

/// The themes in a theme file, converted from Zed's format if it's that.
pub fn parse_file(text: &str) -> Result<Vec<ThemeConfig>, String> {
    let value = jsonc::parse_value(text).map_err(|e| e.to_string())?;
    let themes: Vec<Value> = match value.get("themes") {
        Some(Value::Array(themes)) => themes.clone(),
        Some(_) => return Err("\"themes\" isn't a list".into()),
        None if value.is_object() => vec![value],
        None => return Err("expected a theme ({ \"name\", \"mode\", … }) or a set of them ({ \"themes\": […] })".into()),
    };
    themes
        .into_iter()
        .map(|theme| {
            let mut theme = if theme.get("style").is_some() { from_zed(theme) } else { theme };
            // Editor colors without syntax colors are fine too.
            if let Some(highlight) = theme.get_mut("highlight").and_then(Value::as_object_mut) {
                highlight.entry("syntax").or_insert_with(|| json!({}));
            }
            let config: ThemeConfig = serde_json::from_value(theme).map_err(|e| e.to_string())?;
            if config.name.is_empty() { Err("a theme has no \"name\"".to_string()) } else { Ok(config) }
        })
        .collect()
}

/// A Zed theme (`appearance`, `style`) as gpui-component's: the style is
/// the editor's highlight theme as it is, and its main surfaces and text
/// colors become the UI's.
fn from_zed(theme: Value) -> Value {
    let style = theme.get("style").cloned().unwrap_or_default();
    let mut colors = serde_json::Map::new();
    for (zed, ours) in [
        ("background", "background"),
        ("text", "foreground"),
        ("border", "border"),
        ("border", "input.border"),
        ("text.muted", "muted.foreground"),
        ("surface.background", "muted.background"),
        ("panel.background", "sidebar.background"),
        ("panel.background", "list.background"),
        ("text", "sidebar.foreground"),
        ("border", "sidebar.border"),
        ("elevated_surface.background", "popover.background"),
        ("text", "popover.foreground"),
        ("element.hover", "list.hover.background"),
        ("element.selected", "list.active.background"),
        ("element.hover", "accent.background"),
        ("text", "accent.foreground"),
        ("title_bar.background", "title_bar.background"),
        ("status_bar.background", "status_bar.background"),
        ("tab_bar.background", "tab_bar.background"),
        ("tab.inactive_background", "tab.background"),
        ("tab.active_background", "tab.active.background"),
        ("editor.background", "tab.active.background"),
        ("text.accent", "ring"),
        ("text.accent", "link"),
        ("players", "caret"),
    ] {
        let value = match zed {
            "players" => style.pointer("/players/0/cursor").cloned(),
            key => style.get(key).cloned(),
        };
        if let Some(value) = value.filter(Value::is_string)
            && !colors.contains_key(ours)
        {
            colors.insert(ours.into(), value);
        }
    }
    if let Some(selection) = style.pointer("/players/0/selection").filter(|v| v.is_string()) {
        colors.insert("selection.background".into(), selection.clone());
    }
    json!({
        "name": theme.get("name").cloned().unwrap_or_default(),
        "mode": theme.get("appearance").cloned().unwrap_or_else(|| json!("dark")),
        "colors": colors,
        "highlight": style,
    })
}

impl Loaded {
    /// Reads the themes `names` name that aren't built in: `<name>.json`
    /// first, then every file in the folder until each is found.
    pub fn read(dir: &Path, names: &[&str]) -> Self {
        let mut loaded = Loaded { names: names.iter().map(|n| n.to_string()).collect(), ..Default::default() };
        loaded.stamps.push((dir.to_path_buf(), stamp(dir)));
        let wanted: Vec<&str> = names.iter().copied().filter(|name| !is_built_in(name)).collect();
        if wanted.is_empty() {
            return loaded;
        }
        let mut files: Vec<PathBuf> = wanted.iter().map(|name| dir.join(format!("{name}.json"))).filter(|p| p.exists()).collect();
        let found = |loaded: &Loaded| wanted.iter().all(|name| loaded.themes.iter().any(|t| t.config.name.as_ref() == *name));
        let mut listed = false;
        while !found(&loaded) {
            let Some(path) = files.pop() else {
                if listed {
                    break;
                }
                listed = true;
                files = theme_files(dir).into_iter().filter(|p| !loaded.stamps.iter().any(|(read, _)| read == p)).collect();
                continue;
            };
            loaded.read_file(&path);
        }
        loaded
    }

    fn read_file(&mut self, path: &Path) {
        if self.stamps.iter().any(|(read, _)| read == path) {
            return;
        }
        self.stamps.push((path.to_path_buf(), stamp(path)));
        let text = std::fs::read_to_string(path).unwrap_or_default();
        match parse_file(&text) {
            Ok(themes) => self.themes.extend(themes.into_iter().map(|config| ThemeFile { config, path: path.to_path_buf() })),
            Err(err) => self.errors.push(format!("{}: {err}", super::label(path))),
        }
    }

    /// Every theme in the folder (Select Theme…).
    pub fn read_all(dir: &Path) -> Self {
        let mut loaded = Loaded::default();
        for path in theme_files(dir) {
            loaded.read_file(&path);
        }
        loaded
    }
}

fn theme_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
}

pub fn is_built_in(name: &str) -> bool {
    name == super::content::DEFAULT_LIGHT_THEME || name == super::content::DEFAULT_DARK_THEME
}

/// The themes in use and what they came from.
pub struct ThemeStore {
    loaded: Loaded,
    themes: HashMap<String, Rc<ThemeConfig>>,
    /// Theme names the settings ask for that no file has.
    missing: Vec<String>,
    /// The monospace font before any theme or setting changed it.
    default_mono: Option<SharedString>,
    /// A font `buffer_font_family` or the theme names that isn't installed.
    font_problem: Option<String>,
    /// The theme shown, by name.
    pub applied: Option<String>,
}

impl ThemeStore {
    pub fn new(loaded: Loaded) -> Self {
        let mut store = Self { loaded: Loaded::default(), themes: HashMap::new(), missing: vec![], default_mono: None, font_problem: None, applied: None };
        store.take(loaded);
        store
    }

    fn take(&mut self, loaded: Loaded) {
        // A file that doesn't parse any more keeps its themes as they were.
        let mut themes: HashMap<String, Rc<ThemeConfig>> = loaded.themes.iter().map(|t| (t.config.name.to_string(), Rc::new(t.config.clone()))).collect();
        for name in &loaded.names {
            if !themes.contains_key(name)
                && let Some(old) = self.themes.get(name)
                && loaded.errors.iter().any(|e| self.loaded.themes.iter().any(|t| t.config.name.as_ref() == name && e.starts_with(&super::label(&t.path))))
            {
                themes.insert(name.clone(), old.clone());
            }
        }
        self.missing = loaded.names.iter().filter(|n| !is_built_in(n) && !themes.contains_key(*n)).cloned().collect();
        self.themes = themes;
        self.loaded = loaded;
    }

    pub fn watched(&self) -> Vec<(PathBuf, Stamp)> {
        self.loaded.stamps.clone()
    }

    pub fn problems(&self) -> Vec<Problem> {
        let mut problems: Vec<Problem> = self.loaded.errors.iter().map(|e| Problem { error: true, message: e.clone() }).collect();
        let dir = crate::paths::display_path(&super::themes_dir());
        problems.extend(self.missing.iter().map(|name| Problem { error: true, message: format!("theme “{name}” isn’t built in or in {dir}") }));
        problems.extend(self.font_problem.iter().map(|message| Problem { error: false, message: message.clone() }));
        problems
    }

    fn config(&self, name: &str, cx: &App) -> Option<Rc<ThemeConfig>> {
        if is_built_in(name) {
            return ThemeRegistry::global(cx).themes().get(name).cloned();
        }
        self.themes.get(name).cloned()
    }
}

/// Reads the theme files again when they changed or the settings name
/// other themes, and shows the theme the settings say.
pub fn refresh(files_changed: bool, cx: &mut App) {
    let selection = Settings::get(cx).theme;
    let names: Vec<String> = selection.names().iter().map(|n| n.to_string()).collect();
    let store = &cx.global::<SettingsStore>().themes;
    if files_changed || store.loaded.names != names {
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let loaded = Loaded::read(&super::themes_dir(), &names);
        cx.update_global::<SettingsStore, _>(|store, _| {
            store.themes.take(loaded);
            store.generation += 1;
        });
    }
    apply(cx.window_appearance(), cx);
}

/// For a window whose appearance changed (and as it opens).
pub fn sync(window: &mut Window, cx: &mut App) {
    apply(window.appearance(), cx);
}

fn is_dark(appearance: WindowAppearance) -> bool {
    matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

/// Shows the theme the settings pick for `appearance`, with the editor
/// font they set. Without a store (tests) or a custom theme this is what
/// zcode always did: the default theme of the appearance's mode.
pub fn apply(appearance: WindowAppearance, cx: &mut App) {
    let system_dark = is_dark(appearance);
    let settings = Settings::get(cx);
    let name = settings.theme.name(system_dark).to_string();
    let config = cx.try_global::<SettingsStore>().and_then(|s| s.themes.config(&name, cx));
    let Some(config) = config else {
        // Missing (and said so): the built-in theme of the appearance.
        let mode = if system_dark { ThemeMode::Dark } else { ThemeMode::Light };
        set_theme(None, mode, &settings, cx);
        return;
    };
    let mode = config.mode;
    set_theme(Some((name, config)), mode, &settings, cx);
}

fn set_theme(theme: Option<(String, Rc<ThemeConfig>)>, mode: ThemeMode, settings: &Settings, cx: &mut App) {
    if !cx.has_global::<Theme>() {
        Theme::change(mode, None, cx);
    }
    let default_mono = match cx.try_global::<SettingsStore>().and_then(|s| s.themes.default_mono.clone()) {
        Some(font) => font,
        None => {
            let font = Theme::global(cx).mono_font_family.clone();
            if cx.has_global::<SettingsStore>() {
                cx.update_global::<SettingsStore, _>(|store, _| store.themes.default_mono = Some(font.clone()));
            }
            font
        }
    };
    let registry = ThemeRegistry::global(cx);
    let built_in = if mode.is_dark() { registry.default_dark_theme().clone() } else { registry.default_light_theme().clone() };
    let config = match &theme {
        // A theme without editor colors gets the built-in theme's.
        Some((_, config)) if config.highlight.is_none() => {
            let mut config = (**config).clone();
            config.highlight = built_in.highlight.clone();
            Rc::new(config)
        }
        Some((_, config)) => config.clone(),
        None => built_in,
    };
    let applied = theme.as_ref().map(|(name, _)| name.clone()).unwrap_or_else(|| config.name.to_string());
    let theme_global = Theme::global_mut(cx);
    if mode.is_dark() {
        theme_global.dark_theme = config.clone();
    } else {
        theme_global.light_theme = config.clone();
    }
    Theme::change(mode, None, cx);
    // A font that isn't installed would be drawn in the system's
    // proportional one, under the terminal's monospace grid: the theme's
    // font instead, else the default monospace one, and said.
    let (mono, font_problem) = mono_font(settings.buffer_font_family.as_deref(), config.mono_font_family.as_ref(), &config.name, &default_mono, &|family| font_installed(family, cx));
    if Theme::global(cx).mono_font_family != mono {
        Theme::update(cx, |theme| theme.mono_font_family = mono);
    }
    if cx.has_global::<SettingsStore>() {
        let store = &cx.global::<SettingsStore>().themes;
        let changed = store.applied.as_ref() != Some(&applied) || store.font_problem != font_problem;
        if changed {
            cx.update_global::<SettingsStore, _>(|store, _| {
                store.themes.applied = Some(applied);
                store.themes.font_problem = font_problem;
                store.generation += 1;
            });
        }
    }
}

/// The editor's and terminal's font: `buffer_font_family`, else the theme's,
/// else `default` (installed), skipping those that aren't installed; and
/// what to say about a skipped one.
fn mono_font(setting: Option<&str>, theme_font: Option<&SharedString>, theme: &str, default: &SharedString, installed: &dyn Fn(&str) -> bool) -> (SharedString, Option<String>) {
    let theme_installed = theme_font.filter(|family| installed(family));
    let fallback = theme_installed.unwrap_or(default).clone();
    match (setting, theme_font) {
        (Some(family), _) if installed(family) => (family.into(), None),
        (Some(family), _) => {
            let settings = super::label(&super::settings_path());
            (fallback.clone(), Some(format!("{settings}: buffer_font_family: no font “{family}” is installed; using {fallback}")))
        }
        (None, Some(family)) if theme_installed.is_none() => {
            (fallback.clone(), Some(format!("theme “{theme}”: its font “{family}” isn’t installed; using {fallback}")))
        }
        (None, _) => (fallback, None),
    }
}

/// Whether a font family can be drawn here (a theme or the settings may
/// name one that isn't installed).
pub fn font_installed(family: &str, cx: &App) -> bool {
    let text_system = cx.text_system();
    text_system
        .get_font_for_id(text_system.resolve_font(&gpui_kit::font(family.to_string())))
        .is_some_and(|font| font.family.eq_ignore_ascii_case(family))
}

/// Every theme Select Theme… offers: the built-in ones, then the folder's.
pub fn all(cx: &App) -> (Vec<(String, bool)>, Vec<String>) {
    let registry = ThemeRegistry::global(cx);
    let mut themes: Vec<(String, bool)> = [registry.default_light_theme(), registry.default_dark_theme()]
        .iter()
        .map(|t| (t.name.to_string(), t.mode.is_dark()))
        .collect();
    let loaded = Loaded::read_all(&super::themes_dir());
    let mut found: Vec<(String, bool)> = loaded.themes.iter().map(|t| (t.config.name.to_string(), t.config.mode.is_dark())).collect();
    found.sort_by_key(|(name, _)| name.to_lowercase());
    found.dedup_by(|a, b| a.0 == b.0);
    themes.extend(found.into_iter().filter(|(name, _)| !is_built_in(name)));
    (themes, loaded.errors)
}

/// The `theme` setting after choosing `name` (a `dark` theme or not): the
/// name alone, or, when the setting has one for each appearance, `name` for
/// the appearance it's made for. Its mode stays unless the theme wouldn't
/// show with it (a dark theme while the system is light).
pub fn chosen(current: &super::ThemeSelection, name: &str, dark: bool, system_dark: bool) -> Value {
    use super::ThemeMode as Mode;
    match current {
        super::ThemeSelection::Static(_) => json!(name),
        super::ThemeSelection::Dynamic { mode, light, dark: dark_theme } => {
            let (light, dark_theme) = if dark { (light.as_str(), name) } else { (name, dark_theme.as_str()) };
            let shows = match mode {
                Mode::System => system_dark == dark,
                Mode::Light => !dark,
                Mode::Dark => dark,
            };
            let mode = match (shows, mode) {
                (true, Mode::System) => "system",
                (_, _) if dark => "dark",
                _ => "light",
            };
            json!({ "mode": mode, "light": light, "dark": dark_theme })
        }
    }
}

/// Writes a theme file to `themes/<file>.json`.
pub fn write_set(set: &Value, file_name: &str) -> Result<PathBuf, String> {
    let file_name: String = file_name.chars().map(|c| if c == '/' || c == ':' { '-' } else { c }).collect();
    let path = super::themes_dir().join(format!("{file_name}.json"));
    let text = serde_json::to_string_pretty(set).map_err(|e| e.to_string())?;
    super::write_file(&path, &format!("{text}\n"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_that_arent_installed_are_said_and_skipped() {
        let installed = |family: &str| ["Menlo", "Courier New"].contains(&family);
        let menlo = SharedString::from("Menlo");
        let courier = SharedString::from("Courier New");
        assert_eq!(mono_font(Some("Courier New"), None, "Default Dark", &menlo, &installed), (courier.clone(), None));
        assert_eq!(mono_font(None, Some(&courier), "Harbor", &menlo, &installed), (courier.clone(), None));
        assert_eq!(mono_font(None, None, "Default Dark", &menlo, &installed), (menlo.clone(), None));
        let (font, problem) = mono_font(Some("NoSuchFontXYZ"), Some(&courier), "Harbor", &menlo, &installed);
        assert_eq!(font, courier);
        assert!(problem.unwrap().ends_with("settings.json: buffer_font_family: no font “NoSuchFontXYZ” is installed; using Courier New"));
        let (font, problem) = mono_font(Some("NoSuchFontXYZ"), Some(&"Gone Mono".into()), "Harbor", &menlo, &installed);
        assert_eq!((font, problem.unwrap().ends_with("using Menlo")), (menlo.clone(), true));
        let (font, problem) = mono_font(None, Some(&"Gone Mono".into()), "Harbor", &menlo, &installed);
        assert_eq!((font, problem.as_deref()), (menlo, Some("theme “Harbor”: its font “Gone Mono” isn’t installed; using Menlo")));
    }

    #[test]
    fn theme_files_in_both_formats() {
        let ours = r##"{
            // A set, as gpui-component writes them.
            "name": "Mine",
            "themes": [{ "name": "Mine Dark", "mode": "dark", "colors": { "background": "#101010" },
                         "highlight": { "editor.background": "#101010", "syntax": { "keyword": { "color": "#ff0000", "font_weight": 700 } } } }]
        }"##;
        let themes = parse_file(ours).unwrap();
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name.as_ref(), "Mine Dark");
        assert!(themes[0].mode.is_dark());
        assert!(themes[0].highlight.as_ref().unwrap().syntax.keyword.is_some());
        let zed = r##"{ "name": "Zedish", "themes": [{ "name": "Zedish Light", "appearance": "light",
            "style": { "background": "#fafafa", "text": "#111111", "editor.background": "#ffffff",
                       "players": [{ "cursor": "#0000ff", "selection": "#0000ff33" }],
                       "syntax": { "string": { "color": "#00aa00" } } } }] }"##;
        let themes = parse_file(zed).unwrap();
        assert!(!themes[0].mode.is_dark());
        assert_eq!(themes[0].colors.background.as_deref(), Some("#fafafa"));
        assert_eq!(themes[0].colors.foreground.as_deref(), Some("#111111"));
        assert_eq!(themes[0].colors.caret.as_deref(), Some("#0000ff"));
        assert!(themes[0].highlight.as_ref().unwrap().syntax.string.is_some());
        assert!(parse_file("[1]").is_err());
        assert!(parse_file(r#"{ "themes": [{ "mode": "dark" }] }"#).unwrap_err().contains("no \"name\""));
    }

    #[test]
    fn only_the_named_themes_are_read() {
        let dir = std::env::temp_dir().join(format!("zcode-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Dusk.json"), r#"{ "name": "Dusk", "mode": "dark" }"#).unwrap();
        std::fs::write(dir.join("set.json"), r#"{ "themes": [{ "name": "Pale", "mode": "light" }, { "name": "Deep", "mode": "dark" }] }"#).unwrap();
        std::fs::write(dir.join("broken.json"), "{ \"name\": ").unwrap();
        let loaded = Loaded::read(&dir, &["Dusk", "Default Light"]);
        assert_eq!(loaded.themes.iter().map(|t| t.config.name.to_string()).collect::<Vec<_>>(), ["Dusk"]);
        assert!(loaded.errors.is_empty(), "the other files weren't read: {:?}", loaded.errors);
        let loaded = Loaded::read(&dir, &["Pale", "Nowhere"]);
        assert!(loaded.themes.iter().any(|t| t.config.name.as_ref() == "Pale"));
        assert_eq!(loaded.errors.len(), 1, "{:?}", loaded.errors);
        let store = ThemeStore::new(loaded);
        assert_eq!(store.missing, ["Nowhere"]);
        assert_eq!(Loaded::read_all(&dir).themes.len(), 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn choosing_a_theme_keeps_the_settings_form() {
        use crate::settings::ThemeSelection;
        assert_eq!(chosen(&ThemeSelection::Static("A".into()), "Dusk", true, false), json!("Dusk"));
        assert_eq!(
            chosen(&ThemeSelection::default(), "Dusk", true, true),
            json!({ "mode": "system", "light": "Default Light", "dark": "Dusk" })
        );
        assert_eq!(
            chosen(&ThemeSelection::default(), "Dusk", true, false),
            json!({ "mode": "dark", "light": "Default Light", "dark": "Dusk" })
        );
    }
}

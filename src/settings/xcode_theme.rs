//! Xcode's font and color themes (`.xccolortheme`, a property list) as
//! zcode themes: the source editor's background, text, current line,
//! selection, insertion point and invisibles, its syntax colors and their
//! fonts' weights and slants, and its font. Xcode colors the editor (and
//! console) only, so the rest of the window keeps the light or dark theme
//! the editor's background calls for.
//!
//! Themes come from Xcode's folder of custom ones,
//! `~/Library/Developer/Xcode/UserData/FontAndColorThemes`, and those it
//! ships in `DVTUserInterfaceKit.framework`; both are only read.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::component::ThemeConfig;
use serde_json::{Map, Value, json};

use crate::plist;

/// An Xcode theme on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct XcodeTheme {
    pub name: String,
    pub path: PathBuf,
    /// One Xcode ships, rather than one the user made.
    pub built_in: bool,
}

/// Xcode's user data folder (`ZCODE_XCODE_USER_DATA` stands in for it in
/// tests).
pub fn user_data_dir() -> PathBuf {
    match std::env::var_os("ZCODE_XCODE_USER_DATA") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Developer/Xcode/UserData"),
    }
}

/// The themes Xcode ships, next to the selected developer folder.
fn built_in_dir() -> Option<PathBuf> {
    let developer = crate::toolchain::developer_dir()?;
    Some(developer.parent()?.join("SharedFrameworks/DVTUserInterfaceKit.framework/Versions/A/Resources/FontAndColorThemes"))
}

/// The user's themes, then Xcode's own.
pub fn find() -> Vec<XcodeTheme> {
    let list = |dir: &Path, built_in: bool| -> Vec<XcodeTheme> {
        let mut themes: Vec<XcodeTheme> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "xccolortheme"))
            .map(|path| XcodeTheme { name: path.file_stem().unwrap_or_default().to_string_lossy().into(), path, built_in })
            .collect();
        themes.sort_by_key(|t| t.name.to_lowercase());
        themes
    };
    let mut themes = list(&user_data_dir().join("FontAndColorThemes"), false);
    if let Some(dir) = built_in_dir() {
        themes.extend(list(&dir, true));
    }
    themes
}

/// A property list file's top-level dictionary: XML, binary or old-style.
pub fn read_dict(path: &Path) -> Result<Vec<plist::Entry>> {
    let bytes = std::fs::read(path).with_context(|| format!("couldn’t read {}", path.display()))?;
    let value = if plist::is_binary(&bytes) {
        plist::binary::read(&bytes)?
    } else {
        let text = std::str::from_utf8(&bytes).context("not UTF-8")?;
        if plist::openstep::is_openstep(text) { plist::openstep::parse(text)? } else { plist::parse_xml(text)?.value }
    };
    match value {
        plist::Value::Dict(entries) => Ok(entries),
        _ => Err(anyhow!("not a dictionary")),
    }
}

fn get<'a>(entries: &'a [plist::Entry], key: &str) -> Option<&'a plist::Value> {
    entries.iter().find(|e| e.key == key).map(|e| &e.value.value)
}

/// Xcode's `"r g b a"` (sRGB components from 0 to 1) as `#rrggbbaa`.
fn color(value: Option<&plist::Value>) -> Option<(String, f32)> {
    let plist::Value::String(text) = value? else { return None };
    let parts: Vec<f32> = text.split_whitespace().filter_map(|p| p.parse().ok()).collect();
    let [r, g, b, a] = match parts.as_slice() {
        [r, g, b, a] => [*r, *g, *b, *a],
        [r, g, b] => [*r, *g, *b, 1.],
        _ => return None,
    };
    let byte = |c: f32| (c.clamp(0., 1.) * 255.).round() as u8;
    let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    Some((format!("#{:02x}{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b), byte(a)), luminance))
}

/// `"SFMono-Bold - 12.0"`: the font's family as zcode names it, its weight
/// and whether it's italic.
fn font(value: Option<&plist::Value>) -> Option<(String, Option<u16>, bool)> {
    let plist::Value::String(text) = value? else { return None };
    let postscript = text.rsplit_once(" - ").map_or(text.as_str(), |(name, _)| name).trim();
    let (family, style) = postscript.split_once('-').unwrap_or((postscript, "Regular"));
    let family = match family {
        "SFMono" => "SF Mono",
        "SourceCodePro" => "Source Code Pro",
        "JetBrainsMono" => "JetBrains Mono",
        "FiraCode" => "Fira Code",
        "FiraMono" => "Fira Mono",
        "CourierNewPSMT" => "Courier New",
        "IBMPlexMono" => "IBM Plex Mono",
        family => family,
    };
    let style = style.to_lowercase();
    let weight = if style.contains("heavy") || style.contains("black") {
        Some(800)
    } else if style.contains("semibold") {
        Some(600)
    } else if style.contains("bold") {
        Some(700)
    } else {
        None
    };
    Some((family.to_string(), weight, style.contains("italic") || style.contains("oblique")))
}

/// Where each of Xcode's syntax colors goes among the editor's highlight
/// names. Tree-sitter can't tell a project's names from the SDK's, so the
/// SDK's ("Other …") colors are used for references; declarations aren't
/// told apart either.
const SYNTAX: &[(&str, &[&str])] = &[
    ("xcode.syntax.keyword", &["keyword", "boolean"]),
    ("xcode.syntax.comment", &["comment"]),
    ("xcode.syntax.comment.doc", &["comment.doc"]),
    ("xcode.syntax.string", &["string", "string.escape", "string.regex"]),
    ("xcode.syntax.character", &["string.special", "string.special.symbol"]),
    ("xcode.syntax.number", &["number"]),
    ("xcode.syntax.preprocessor", &["preproc"]),
    ("xcode.syntax.attribute", &["attribute"]),
    ("xcode.syntax.url", &["link_uri", "link_text"]),
    ("xcode.syntax.identifier.type.system", &["type", "constructor"]),
    ("xcode.syntax.identifier.function.system", &["function"]),
    ("xcode.syntax.identifier.variable.system", &["property"]),
    ("xcode.syntax.identifier.constant.system", &["constant", "enum", "variant"]),
    ("xcode.syntax.identifier.macro.system", &["embedded"]),
    ("xcode.syntax.markup.code", &["text.literal", "text.code.span"]),
    ("xcode.syntax.mark", &["title"]),
];

/// A converted theme: the theme file's JSON (a set of one), and what
/// converting left out, for the import's message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Converted {
    pub set: Value,
    pub dark: bool,
    pub notes: Vec<String>,
}

/// An Xcode theme as a zcode theme set of one theme, named `name`.
/// `font_installed` says whether a font family can be used here.
pub fn convert(path: &Path, name: &str, font_installed: &dyn Fn(&str) -> bool) -> Result<Converted> {
    let entries = read_dict(path)?;
    if get(&entries, "DVTSourceTextSyntaxColors").is_none() {
        return Err(anyhow!("{} isn’t an Xcode font and color theme", path.display()));
    }
    let syntax_colors = match get(&entries, "DVTSourceTextSyntaxColors") {
        Some(plist::Value::Dict(colors)) => colors.clone(),
        _ => vec![],
    };
    let syntax_fonts = match get(&entries, "DVTSourceTextSyntaxFonts") {
        Some(plist::Value::Dict(fonts)) => fonts.clone(),
        _ => vec![],
    };
    let (background, luminance) = color(get(&entries, "DVTSourceTextBackground")).unwrap_or(("#ffffffff".into(), 1.));
    let dark = luminance < 0.5;
    let plain = color(get(&syntax_colors, "xcode.syntax.plain"));

    let mut highlight = Map::new();
    highlight.insert("editor.background".into(), json!(background));
    highlight.insert("editor.gutter.background".into(), json!(background));
    if let Some((plain, _)) = &plain {
        highlight.insert("editor.foreground".into(), json!(plain));
        // Xcode's line numbers are the plain text, dimmed.
        highlight.insert("editor.line_number".into(), json!(format!("{}80", &plain[..7])));
        highlight.insert("editor.active_line_number".into(), json!(plain));
    }
    for (xcode, ours) in [("DVTSourceTextCurrentLineHighlightColor", "editor.active_line.background"), ("DVTSourceTextInvisiblesColor", "editor.invisible")] {
        if let Some((value, _)) = color(get(&entries, xcode)) {
            highlight.insert(ours.into(), json!(value));
        }
    }
    let mut syntax = Map::new();
    for (xcode, names) in SYNTAX {
        let Some((value, _)) = color(get(&syntax_colors, xcode)) else { continue };
        let (_, weight, italic) = font(get(&syntax_fonts, xcode)).unwrap_or_default();
        let mut style = Map::new();
        style.insert("color".into(), json!(value));
        if let Some(weight) = weight {
            style.insert("font_weight".into(), json!(weight));
        }
        if italic {
            style.insert("font_style".into(), json!("italic"));
        }
        for name in *names {
            syntax.insert((*name).into(), Value::Object(style.clone()));
        }
    }
    highlight.insert("syntax".into(), Value::Object(syntax));

    let mut colors = Map::new();
    for (xcode, ours) in [("DVTSourceTextSelectionColor", "selection.background"), ("DVTSourceTextInsertionPointColor", "caret")] {
        if let Some((value, _)) = color(get(&entries, xcode)) {
            colors.insert(ours.into(), json!(value));
        }
    }

    let mut notes = vec![];
    let mut theme = json!({
        "name": name,
        "mode": if dark { "dark" } else { "light" },
        "colors": colors,
        "highlight": highlight,
    });
    if let Some((family, _, _)) = font(get(&syntax_fonts, "xcode.syntax.plain")) {
        if font_installed(&family) {
            theme["mono_font.family"] = json!(family);
        } else {
            notes.push(format!("its font, {family}, isn’t installed for other apps, so the editor keeps its own"));
        }
    }
    // What's written must read back.
    serde_json::from_value::<ThemeConfig>(theme.clone()).context("converting the theme")?;
    let source = path.file_name().unwrap_or_default().to_string_lossy();
    let set = json!({ "name": name, "author": format!("Imported from Xcode’s {source}"), "themes": [theme] });
    Ok(Converted { set, dark, notes })
}

/// The name an imported theme gets: Xcode's, unless it's one of zcode's
/// own built-in names.
pub fn theme_name(xcode_name: &str) -> String {
    if super::themes::is_built_in(xcode_name) { format!("{xcode_name} (Xcode)") } else { xcode_name.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>DVTSourceTextBackground</key>
	<string>0.1 0.1 0.12 1</string>
	<key>DVTSourceTextCurrentLineHighlightColor</key>
	<string>0.15 0.15 0.17 1</string>
	<key>DVTSourceTextSelectionColor</key>
	<string>0.3 0.35 0.45 1</string>
	<key>DVTSourceTextInsertionPointColor</key>
	<string>1 1 1 1</string>
	<key>DVTSourceTextSyntaxColors</key>
	<dict>
		<key>xcode.syntax.plain</key>
		<string>1 1 1 0.85</string>
		<key>xcode.syntax.keyword</key>
		<string>1 0 0.5 1</string>
		<key>xcode.syntax.string</key>
		<string>1 0.4 0.35 1</string>
		<key>xcode.syntax.comment</key>
		<string>0.5 0.55 0.6 1</string>
	</dict>
	<key>DVTSourceTextSyntaxFonts</key>
	<dict>
		<key>xcode.syntax.plain</key>
		<string>Menlo-Regular - 12.0</string>
		<key>xcode.syntax.keyword</key>
		<string>Menlo-Bold - 12.0</string>
		<key>xcode.syntax.comment</key>
		<string>Menlo-Italic - 12.0</string>
	</dict>
</dict>
</plist>
"#;

    #[test]
    fn an_xcode_theme_becomes_a_zcode_theme() {
        let path = std::env::temp_dir().join(format!("zcode-xccolortheme-{}.xccolortheme", std::process::id()));
        std::fs::write(&path, FIXTURE).unwrap();
        let converted = convert(&path, "Night", &|family| family == "Menlo").unwrap();
        assert!(converted.notes.is_empty(), "{:?}", converted.notes);
        assert!(converted.dark);
        let theme = &super::super::themes::parse_file(&converted.set.to_string()).unwrap()[0];
        assert_eq!(theme.name.as_ref(), "Night");
        assert!(theme.mode.is_dark());
        assert_eq!(theme.mono_font_family.as_deref(), Some("Menlo"));
        assert_eq!(theme.colors.selection.as_deref(), Some("#4d5973ff"));
        assert_eq!(theme.colors.caret.as_deref(), Some("#ffffffff"));
        let json = &converted.set;
        assert!(!json.to_string().contains("null"), "only what the theme sets is written: {json}");
        let highlight = &json["themes"][0]["highlight"];
        assert_eq!(highlight["editor.background"], "#1a1a1fff");
        assert_eq!(highlight["editor.foreground"], "#ffffffd9");
        assert_eq!(highlight["syntax"]["keyword"]["font_weight"], 700);
        assert_eq!(highlight["syntax"]["boolean"]["color"], "#ff0080ff");
        assert_eq!(highlight["syntax"]["comment"]["font_style"], "italic");
        let without_font = convert(&path, "Night", &|_| false).unwrap();
        assert!(without_font.notes[0].contains("Menlo"));
        assert_eq!(without_font.set["themes"][0].get("mono_font.family"), None);
        std::fs::write(&path, "<plist><dict></dict></plist>").unwrap();
        assert!(convert(&path, "x", &|_| true).is_err());
        std::fs::remove_file(&path).ok();
        assert_eq!(theme_name("Default Dark"), "Default Dark (Xcode)");
        assert_eq!(theme_name("Default (Dark)"), "Default (Dark)");
    }

    /// Every theme Xcode ships converts (needs Xcode; only reads it).
    #[test]
    #[ignore]
    fn xcodes_own_themes_convert() {
        let themes = find();
        let built_in: Vec<_> = themes.iter().filter(|t| t.built_in).collect();
        assert!(built_in.len() >= 10, "{themes:?}");
        for theme in built_in {
            let converted = convert(&theme.path, &theme.name, &|_| true).unwrap_or_else(|e| panic!("{}: {e:#}", theme.name));
            let config = &super::super::themes::parse_file(&converted.set.to_string()).unwrap()[0];
            assert_eq!(config.mode.is_dark(), converted.dark);
            assert!(config.highlight.as_ref().unwrap().syntax.keyword.is_some(), "{}", theme.name);
        }
        let dark = themes.iter().find(|t| t.name == "Default (Dark)").unwrap();
        assert!(convert(&dark.path, "d", &|_| true).unwrap().dark);
        let light = themes.iter().find(|t| t.name == "Default (Light)").unwrap();
        assert!(!convert(&light.path, "l", &|_| true).unwrap().dark);
    }
}

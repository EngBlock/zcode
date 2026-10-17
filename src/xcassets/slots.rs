//! A set's slots as Xcode's asset catalog editor lays them out: an entry of
//! the set's array each, in rows by device and appearance (a row header
//! like "Universal" or "iPhone · Dark") with a caption under each well
//! ("2x", "20pt 3x", "Any Appearance").

use serde_json::Value;

use super::SetKind;
use super::color::ColorValue;

#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    /// The entry's index in the set's array.
    pub index: usize,
    /// The row it's in.
    pub row: String,
    /// The caption under it.
    pub label: String,
    pub filename: Option<String>,
    pub idiom: String,
    pub scale: Option<String>,
    /// `luminosity` / `contrast` values, e.g. `["dark"]`.
    pub appearances: Vec<String>,
    /// Color sets: the color.
    pub color: Option<ColorValue>,
}

impl Slot {
    /// Row and caption in one, for the control channel and errors.
    pub fn title(&self) -> String {
        format!("{} {}", self.row, self.label)
    }
}

pub fn idiom_title(idiom: &str) -> String {
    match idiom {
        "universal" => "Universal",
        "iphone" => "iPhone",
        "ipad" => "iPad",
        "mac" => "Mac",
        "tv" => "Apple TV",
        "watch" => "Apple Watch",
        "car" => "CarPlay",
        "vision" | "reality" => "Apple Vision",
        "ios-marketing" | "watch-marketing" => "App Store",
        "mac-catalyst" => "Mac Catalyst",
        other => return other.to_string(),
    }
    .to_string()
}

fn platform_title(platform: &str) -> String {
    match platform {
        "ios" => "iOS",
        "macos" | "osx" => "macOS",
        "watchos" => "watchOS",
        "tvos" => "tvOS",
        "visionos" | "xros" => "visionOS",
        other => return other.to_string(),
    }
    .to_string()
}

fn appearance_title(value: &str) -> String {
    match value {
        "light" => "Light",
        "dark" => "Dark",
        "tinted" => "Tinted",
        "high" => "High Contrast",
        other => return other.to_string(),
    }
    .to_string()
}

fn appearances(entry: &Value) -> Vec<String> {
    entry
        .get("appearances")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|a| a.get("value").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_default()
}

fn str_field<'a>(entry: &'a Value, key: &str) -> Option<&'a str> {
    entry.get(key).and_then(Value::as_str)
}

/// "20x20" → "20pt", "83.5x83.5" → "83.5pt", "1024x1024" → "1024pt".
fn size_title(size: &str) -> String {
    match size.split_once('x') {
        Some((w, h)) if w == h => format!("{w}pt"),
        _ => size.to_string(),
    }
}

/// What tells entries of one row apart from another's: device, size
/// classes, gamut, language direction…, then the appearance.
fn row_title(kind: SetKind, entry: &Value, any_appearance: bool) -> String {
    let idiom = str_field(entry, "idiom").unwrap_or("universal");
    let mut parts = vec![];
    match (kind, str_field(entry, "platform")) {
        (SetKind::AppIcon, Some(platform)) if idiom == "universal" => parts.push(platform_title(platform)),
        _ => parts.push(idiom_title(idiom)),
    }
    if let Some(subtype) = str_field(entry, "subtype") {
        parts.push(subtype.to_string());
    }
    for key in ["width-class", "height-class"] {
        if let Some(class) = str_field(entry, key) {
            parts.push(format!("{} {}", if key == "width-class" { "Width" } else { "Height" }, class));
        }
    }
    if let Some(gamut) = str_field(entry, "display-gamut") {
        parts.push(if gamut == "display-P3" { "Display P3".into() } else { "sRGB".into() });
    }
    if let Some(direction) = str_field(entry, "language-direction") {
        parts.push(direction.replace("-to-", " to ").to_string());
    }
    if let Some(locale) = str_field(entry, "locale") {
        parts.push(locale.to_string());
    }
    // Image wells are rows per appearance; colors and icons put the
    // appearance under each well instead.
    if kind == SetKind::Image {
        let looks = appearances(entry);
        if !looks.is_empty() {
            parts.extend(looks.iter().map(|a| appearance_title(a)));
        } else if any_appearance {
            parts.push("Any Appearance".into());
        }
    }
    parts.join(" · ")
}

fn caption(kind: SetKind, entry: &Value, any_appearance: bool) -> String {
    let looks: Vec<String> = appearances(entry).iter().map(|a| appearance_title(a)).collect();
    let look = if looks.is_empty() { any_appearance.then(|| "Any Appearance".to_string()) } else { Some(looks.join(", ")) };
    match kind {
        SetKind::Image => str_field(entry, "scale").map(str::to_string).unwrap_or_else(|| "Single Scale".into()),
        SetKind::AppIcon => {
            let size = str_field(entry, "size").map(size_title);
            let scale = str_field(entry, "scale").map(str::to_string);
            let mut parts: Vec<String> = [size, scale].into_iter().flatten().collect();
            if let Some(role) = str_field(entry, "role") {
                parts.push(role.to_string());
            }
            if let Some(look) = look.filter(|_| any_appearance) {
                parts.push(look);
            }
            if parts.is_empty() { "Any".into() } else { parts.join(" ") }
        }
        SetKind::Color => look.unwrap_or_else(|| "Any Appearance".into()),
        SetKind::Data | SetKind::Symbol | SetKind::Other => {
            look.unwrap_or_else(|| idiom_title(str_field(entry, "idiom").unwrap_or("universal")))
        }
    }
}

/// The slots of a set's entries, in their order (rows in order of first
/// appearance, as Xcode shows them).
pub fn slots(kind: SetKind, entries: &[Value]) -> Vec<Slot> {
    let any_appearance = entries.iter().any(|e| !appearances(e).is_empty());
    let row_titles: Vec<String> = entries.iter().map(|e| row_title(kind, e, any_appearance)).collect();
    // A caption names the appearance when its row has several.
    let row_has_appearances =
        |row: &str| entries.iter().zip(&row_titles).any(|(e, r)| r == row && !appearances(e).is_empty());
    let slots: Vec<Slot> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| Slot {
            index,
            row: row_titles[index].clone(),
            label: caption(kind, entry, row_has_appearances(&row_titles[index])),
            filename: str_field(entry, "filename").map(str::to_string),
            idiom: str_field(entry, "idiom").unwrap_or("universal").to_string(),
            scale: str_field(entry, "scale").map(str::to_string),
            appearances: appearances(entry),
            color: (kind == SetKind::Color).then(|| ColorValue::of(entry)),
        })
        .collect();
    // Rows keep the order they first come in; wells keep theirs in a row.
    let mut rows: Vec<String> = vec![];
    for slot in &slots {
        if !rows.contains(&slot.row) {
            rows.push(slot.row.clone());
        }
    }
    let mut ordered = vec![];
    for row in rows {
        ordered.extend(slots.iter().filter(|s| s.row == row).cloned());
    }
    ordered
}

/// Rows of slots, with their headers.
pub fn rows(slots: &[Slot]) -> Vec<(String, Vec<&Slot>)> {
    let mut out: Vec<(String, Vec<&Slot>)> = vec![];
    for slot in slots {
        match out.iter_mut().find(|(row, _)| *row == slot.row) {
            Some((_, row)) => row.push(slot),
            None => out.push((slot.row.clone(), vec![slot])),
        }
    }
    out
}

/// Finds a slot by index (`3`), or by caption, row and caption
/// (`"Universal 2x"`, `"Dark"`), or file name.
pub fn find<'a>(slots: &'a [Slot], query: &str) -> Option<&'a Slot> {
    if let Ok(ix) = query.parse::<usize>() {
        return slots.iter().find(|s| s.index == ix);
    }
    slots
        .iter()
        .find(|s| s.title().eq_ignore_ascii_case(query))
        .or_else(|| slots.iter().find(|s| s.label.eq_ignore_ascii_case(query)))
        .or_else(|| slots.iter().find(|s| s.filename.as_deref() == Some(query)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xcassets::{Catalog, tests::fixture_catalog};

    fn set_slots(name: &str) -> Vec<Slot> {
        let catalog = Catalog::load(&fixture_catalog()).unwrap();
        let node = catalog.root.sets().into_iter().find(|n| n.name == name).unwrap();
        node.set().unwrap().slots()
    }

    #[test]
    fn image_wells_by_appearance_and_scale() {
        let slots = set_slots("Logo");
        let laid_out: Vec<(String, Vec<String>)> =
            rows(&slots).into_iter().map(|(row, s)| (row, s.iter().map(|s| s.label.clone()).collect())).collect();
        assert_eq!(
            laid_out,
            vec![
                ("Universal · Any Appearance".to_string(), vec!["1x".to_string(), "2x".into(), "3x".into()]),
                ("Universal · Dark".to_string(), vec!["1x".to_string(), "2x".into(), "3x".into()]),
            ]
        );
        assert_eq!(slots[1].filename.as_deref(), Some("logo@2x.png"));
        assert_eq!(find(&slots, "Universal · Dark 2x").map(|s| s.index), Some(4));
        assert_eq!(find(&slots, "4").map(|s| s.title()), Some("Universal · Dark 2x".to_string()));
    }

    #[test]
    fn app_icons_by_platform_with_appearances() {
        let slots = set_slots("AppIcon");
        let titles: Vec<String> = slots.iter().map(Slot::title).collect();
        assert_eq!(
            titles,
            [
                "iOS 1024pt Any Appearance",
                "iOS 1024pt Dark",
                "iOS 1024pt Tinted",
                "Mac 16pt 1x",
                "Mac 16pt 2x",
            ]
        );
    }

    #[test]
    fn colors_by_appearance() {
        let slots = set_slots("Brand");
        assert_eq!(slots.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["Any Appearance", "Dark"]);
        assert_eq!(slots[0].color.as_ref().map(ColorValue::hex).as_deref(), Some("#2E4A73"));
        assert_eq!(slots[1].color.as_ref().map(ColorValue::hex).as_deref(), Some("#7A98BF"));
        // A single color is captioned as Xcode does, without appearances.
        let single = slots_of_color(r#"[{ "idiom": "universal" }]"#);
        assert_eq!(single[0].title(), "Universal Any Appearance");
    }

    fn slots_of_color(entries: &str) -> Vec<Slot> {
        slots(SetKind::Color, &serde_json::from_str::<Vec<Value>>(entries).unwrap())
    }
}

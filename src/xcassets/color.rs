//! A color set's colors: the `color` object of a `colors` entry, its color
//! space and components as Xcode stores them (strings in one of the
//! attributes inspector's input methods: floating point `"0.451"`, 8-bit
//! `"115"` or 8-bit hexadecimal `"0x73"`), what a swatch shows, and edits
//! that keep each component's input method.

use serde_json::{Map, Value, json};

/// The attributes inspector's Input Method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMethod {
    /// Floating point (0–1), `"0.451"`.
    Float,
    /// 8-bit (0–255), `"115"`.
    Integer,
    /// 8-bit hexadecimal, `"0x73"`.
    Hex,
}

impl InputMethod {
    pub fn title(self) -> &'static str {
        match self {
            InputMethod::Float => "Floating point (0.0–1.0)",
            InputMethod::Integer => "8-bit (0–255)",
            InputMethod::Hex => "8-bit Hexadecimal",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            InputMethod::Float => "float",
            InputMethod::Integer => "8-bit",
            InputMethod::Hex => "hex",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [InputMethod::Float, InputMethod::Integer, InputMethod::Hex].into_iter().find(|m| m.id() == id)
    }

    /// How a stored component is written.
    pub fn of(raw: &Value) -> Self {
        match raw.as_str().map(str::trim) {
            Some(s) if s.starts_with("0x") || s.starts_with("0X") => InputMethod::Hex,
            Some(s) if !s.contains('.') && s.parse::<u32>().is_ok_and(|n| n > 1) => InputMethod::Integer,
            _ => InputMethod::Float,
        }
    }

    /// `value` (0–1) written this way.
    pub fn format(self, value: f64) -> String {
        match self {
            InputMethod::Float => format!("{value:.3}"),
            InputMethod::Integer => format!("{}", (value.clamp(0., 1.) * 255.).round() as u32),
            InputMethod::Hex => format!("0x{:02X}", (value.clamp(0., 1.) * 255.).round() as u32),
        }
    }

    /// What's typed in a component's field, as 0–1.
    pub fn parse(self, text: &str) -> Option<f64> {
        let text = text.trim();
        match self {
            InputMethod::Float => text.parse::<f64>().ok().filter(|v| v.is_finite()),
            InputMethod::Integer => text.parse::<u32>().ok().filter(|n| *n <= 255).map(|n| n as f64 / 255.),
            InputMethod::Hex => {
                let digits = text.trim_start_matches("0x").trim_start_matches("0X").trim_start_matches('#');
                u32::from_str_radix(digits, 16).ok().filter(|n| *n <= 255).map(|n| n as f64 / 255.)
            }
        }
    }
}

/// A stored component, as 0–1 (alpha in floating point however it's
/// written: Xcode keeps it that way).
pub fn component_value(raw: &Value) -> Option<f64> {
    match raw {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let s = s.trim();
            if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                return u32::from_str_radix(hex, 16).ok().map(|n| n as f64 / 255.);
            }
            let v: f64 = s.parse().ok()?;
            Some(if !s.contains('.') && v > 1. { v / 255. } else { v })
        }
        _ => None,
    }
}

/// Xcode's Content menu: its color spaces.
pub const COLOR_SPACES: &[(&str, &str)] = &[
    ("srgb", "sRGB"),
    ("display-p3", "Display P3"),
    ("extended-srgb", "Extended Range sRGB"),
    ("extended-linear-srgb", "Extended Range Linear sRGB"),
    ("gray-gamma-22", "Gray Gamma 2.2"),
    ("extended-gray", "Extended Range Gray"),
];

pub fn color_space_title(id: &str) -> String {
    COLOR_SPACES.iter().find(|(i, _)| *i == id).map(|(_, t)| t.to_string()).unwrap_or_else(|| id.to_string())
}

pub fn is_gray(space: &str) -> bool {
    space.contains("gray")
}

/// The components a color space has, in the inspector's order.
pub fn component_keys(space: &str) -> &'static [&'static str] {
    if is_gray(space) { &["white", "alpha"] } else { &["red", "green", "blue", "alpha"] }
}

pub fn component_title(key: &str) -> &'static str {
    match key {
        "red" => "Red",
        "green" => "Green",
        "blue" => "Blue",
        "white" => "White",
        "alpha" => "Opacity",
        _ => "Value",
    }
}

/// What a `colors` entry's `color` says.
#[derive(Clone, Debug, PartialEq)]
pub enum ColorValue {
    /// No color yet (a new color set's slot): Xcode shows white.
    Unset,
    /// A system color by name (`"reference" : "systemBlueColor"`).
    Reference(String),
    Components { space: String, components: Vec<(String, Value)> },
}

impl ColorValue {
    pub fn of(entry: &Value) -> Self {
        let Some(color) = entry.get("color").and_then(Value::as_object) else { return ColorValue::Unset };
        if let Some(name) = color.get("reference").and_then(Value::as_str) {
            return ColorValue::Reference(name.to_string());
        }
        let space = color.get("color-space").and_then(Value::as_str).unwrap_or("srgb").to_string();
        let components = color
            .get("components")
            .and_then(Value::as_object)
            .map(|c| c.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        ColorValue::Components { space, components }
    }

    pub fn component(&self, key: &str) -> Option<f64> {
        match self {
            ColorValue::Components { components, .. } => components.iter().find(|(k, _)| k == key).and_then(|(_, v)| component_value(v)),
            _ => None,
        }
    }

    /// The input method the components are written in (the first one's;
    /// alpha is always floating point).
    pub fn input_method(&self) -> InputMethod {
        match self {
            ColorValue::Components { components, .. } => {
                components.iter().find(|(k, _)| k != "alpha").map(|(_, v)| InputMethod::of(v)).unwrap_or(InputMethod::Float)
            }
            _ => InputMethod::Float,
        }
    }

    /// Red, green, blue and alpha (0–1) for a swatch. Display P3 and the
    /// extended spaces are shown as if sRGB, clamped; a system color as
    /// its usual light-appearance value.
    pub fn rgba(&self) -> [f32; 4] {
        match self {
            ColorValue::Unset => [1., 1., 1., 1.],
            ColorValue::Reference(name) => system_color(name).unwrap_or([0.5, 0.5, 0.5, 1.]),
            ColorValue::Components { space, .. } => {
                let get = |k: &str, default: f64| self.component(k).unwrap_or(default).clamp(0., 1.) as f32;
                let alpha = get("alpha", 1.);
                if is_gray(space) {
                    let w = get("white", 0.);
                    [w, w, w, alpha]
                } else {
                    [get("red", 0.), get("green", 0.), get("blue", 0.), alpha]
                }
            }
        }
    }

    /// `#RRGGBB` (and `AA` when not opaque), for the inspector and tests.
    pub fn hex(&self) -> String {
        let [r, g, b, a] = self.rgba().map(|c| (c * 255.).round() as u8);
        if a == 255 { format!("#{r:02X}{g:02X}{b:02X}") } else { format!("#{r:02X}{g:02X}{b:02X}{a:02X}") }
    }
}

/// A `colors` entry with `key` set to `value` (0–1), written in the input
/// method the color uses, Xcode's 3-decimal floating point for alpha and
/// for a color that had none. An unset color becomes sRGB white first, as
/// Xcode's inspector starts from.
pub fn set_component(entry: &Value, key: &str, value: f64) -> Value {
    let mut entry = entry.clone();
    let current = ColorValue::of(&entry);
    let method = current.input_method();
    let (space, mut components) = match current {
        ColorValue::Components { space, components } => (space, components),
        ColorValue::Unset | ColorValue::Reference(_) => {
            let space = "srgb".to_string();
            let white = component_keys(&space).iter().map(|k| (k.to_string(), json!("1.000"))).collect();
            (space, white)
        }
    };
    let extended = space.starts_with("extended");
    let value = if extended { value } else { value.clamp(0., 1.) };
    let raw = if key == "alpha" { InputMethod::Float.format(value.clamp(0., 1.)) } else { method.format(value) };
    match components.iter_mut().find(|(k, _)| k == key) {
        Some((_, v)) => *v = json!(raw),
        None => components.push((key.to_string(), json!(raw))),
    }
    write_color(&mut entry, &space, components);
    entry
}

/// A `colors` entry with its components rewritten in `method`.
pub fn set_input_method(entry: &Value, method: InputMethod) -> Value {
    let mut entry = entry.clone();
    let ColorValue::Components { space, components } = ColorValue::of(&entry) else { return entry };
    let components = components
        .into_iter()
        .map(|(k, v)| {
            let value = component_value(&v).unwrap_or(0.);
            let raw = if k == "alpha" { InputMethod::Float.format(value) } else { method.format(value) };
            (k, json!(raw))
        })
        .collect();
    write_color(&mut entry, &space, components);
    entry
}

/// A `colors` entry in another color space: the components carried over
/// (gray from red, green and blue's mean; RGB from white), as Xcode keeps
/// the numbers when the Content menu changes.
pub fn set_color_space(entry: &Value, space: &str) -> Value {
    let mut entry = entry.clone();
    let current = ColorValue::of(&entry);
    let method = current.input_method();
    let [r, g, b, a] = current.rgba().map(f64::from);
    let components = if is_gray(space) {
        vec![("white".to_string(), (r + g + b) / 3.), ("alpha".to_string(), a)]
    } else {
        vec![("red".to_string(), r), ("green".to_string(), g), ("blue".to_string(), b), ("alpha".to_string(), a)]
    };
    let components = components
        .into_iter()
        .map(|(k, v)| {
            let raw = if k == "alpha" { InputMethod::Float.format(v) } else { method.format(v) };
            (k, json!(raw))
        })
        .collect();
    write_color(&mut entry, space, components);
    entry
}

fn write_color(entry: &mut Value, space: &str, components: Vec<(String, Value)>) {
    let mut map = Map::new();
    for (k, v) in components {
        map.insert(k, v);
    }
    if let Some(obj) = entry.as_object_mut() {
        obj.insert("color".into(), json!({ "color-space": space, "components": Value::Object(map) }));
    }
}

/// UIKit's and AppKit's system colors' light-appearance values, for
/// `reference` swatches.
fn system_color(name: &str) -> Option<[f32; 4]> {
    let rgb = |r: u8, g: u8, b: u8| Some([r as f32 / 255., g as f32 / 255., b as f32 / 255., 1.]);
    match name.trim_end_matches("Color") {
        "systemRed" => rgb(255, 59, 48),
        "systemOrange" => rgb(255, 149, 0),
        "systemYellow" => rgb(255, 204, 0),
        "systemGreen" => rgb(52, 199, 89),
        "systemMint" => rgb(0, 199, 190),
        "systemTeal" => rgb(48, 176, 199),
        "systemCyan" => rgb(50, 173, 230),
        "systemBlue" | "link" => rgb(0, 122, 255),
        "systemIndigo" => rgb(88, 86, 214),
        "systemPurple" => rgb(175, 82, 222),
        "systemPink" => rgb(255, 45, 85),
        "systemBrown" => rgb(162, 132, 94),
        "systemGray" => rgb(142, 142, 147),
        "label" | "darkText" => rgb(0, 0, 0),
        "systemBackground" | "lightText" => rgb(255, 255, 255),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(components: Value) -> Value {
        json!({ "color": { "color-space": "srgb", "components": components }, "idiom": "universal" })
    }

    #[test]
    fn components_in_each_input_method() {
        assert_eq!(component_value(&json!("0.451")), Some(0.451));
        assert_eq!(component_value(&json!("255")), Some(1.));
        assert_eq!(component_value(&json!("0x80")).map(|v| (v * 255.).round()), Some(128.));
        assert_eq!(component_value(&json!("1")), Some(1.), "an alpha of 1");
        assert_eq!(component_value(&json!(0.25)), Some(0.25));
        assert_eq!(InputMethod::of(&json!("0.451")), InputMethod::Float);
        assert_eq!(InputMethod::of(&json!("115")), InputMethod::Integer);
        assert_eq!(InputMethod::of(&json!("0x73")), InputMethod::Hex);
        assert_eq!(InputMethod::Hex.format(115. / 255.), "0x73");
        assert_eq!(InputMethod::Integer.parse("115").map(|v| (v * 255.).round()), Some(115.));
        assert_eq!(InputMethod::Hex.parse("#ff"), Some(1.));
        assert_eq!(InputMethod::Integer.parse("300"), None);
    }

    #[test]
    fn a_swatch_from_xcodes_components() {
        let accent = entry(json!({ "alpha": "1.000", "blue": "0.451", "green": "0.290", "red": "0.180" }));
        let color = ColorValue::of(&accent);
        assert_eq!(color.hex(), "#2E4A73");
        assert_eq!(ColorValue::of(&json!({ "idiom": "universal" })), ColorValue::Unset);
        assert_eq!(ColorValue::of(&json!({ "color": { "reference": "systemPurpleColor" } })).hex(), "#AF52DE");
        let gray = json!({ "color": { "color-space": "gray-gamma-22", "components": { "alpha": "0.500", "white": "0.000" } } });
        assert_eq!(ColorValue::of(&gray).hex(), "#00000080");
    }

    #[test]
    fn editing_keeps_the_input_method() {
        let hex = entry(json!({ "alpha": "1.000", "blue": "0x00", "green": "0x00", "red": "0xFF" }));
        let edited = set_component(&hex, "green", 0.5);
        assert_eq!(edited["color"]["components"]["green"], "0x80");
        assert_eq!(edited["color"]["components"]["red"], "0xFF");
        let edited = set_component(&hex, "alpha", 0.25);
        assert_eq!(edited["color"]["components"]["alpha"], "0.250");
        // An unset color starts white in sRGB.
        let unset = set_component(&json!({ "idiom": "universal" }), "red", 0.2);
        assert_eq!(
            unset,
            json!({ "color": { "color-space": "srgb", "components": { "red": "0.200", "green": "1.000", "blue": "1.000", "alpha": "1.000" } }, "idiom": "universal" })
        );
        let ints = set_input_method(&hex, InputMethod::Integer);
        assert_eq!(ints["color"]["components"]["red"], "255");
        assert_eq!(ints["color"]["components"]["alpha"], "1.000");
        let gray = set_color_space(&hex, "gray-gamma-22");
        assert_eq!(gray["color"]["components"], json!({ "white": "0x55", "alpha": "1.000" }));
    }
}

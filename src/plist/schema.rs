//! What Xcode knows about a kind of property list: human-readable key names,
//! value types and allowed values (from its structure definitions, see
//! `definitions.rs`), and lookups that follow a value's path through them.

use super::{Kind, Node, Value};

pub struct Schema {
    pub name: &'static str,
    /// File name patterns (`*` wildcards) the schema applies to.
    pub patterns: &'static [&'static str],
    /// The root row's label ("Information Property List").
    pub root_label: &'static str,
    /// Sorted by name; `_root_` is the top-level value.
    pub definitions: &'static [Definition],
}

pub struct Definition {
    pub name: &'static str,
    /// A primitive type or another definition's name.
    pub class: &'static str,
    /// For arrays: the class of their elements.
    pub element: &'static str,
    pub label: &'static str,
    pub default: &'static str,
    pub keys: &'static [Key],
    pub choices: &'static [Choice],
}

pub struct Key {
    pub name: &'static str,
    pub class: &'static str,
    pub label: &'static str,
}

pub struct Choice {
    pub value: &'static str,
    pub label: &'static str,
}

/// A class resolved to its primitive kind and the definitions on the way.
#[derive(Clone, Copy)]
pub struct Resolved {
    pub kind: Option<Kind>,
    /// The outermost definition (its keys, choices and default).
    pub definition: Option<&'static Definition>,
    /// For arrays: the class of their elements.
    pub element: Option<&'static str>,
}

impl Resolved {
    pub const UNKNOWN: Resolved = Resolved { kind: None, definition: None, element: None };

    pub fn keys(&self) -> &'static [Key] {
        self.definition.map_or(&[], |d| d.keys)
    }

    pub fn choices(&self) -> &'static [Choice] {
        self.definition.map_or(&[], |d| d.choices)
    }

    pub fn key(&self, name: &str) -> Option<&'static Key> {
        self.keys().iter().find(|k| k.name == name)
    }
}

fn primitive(class: &str) -> Option<Kind> {
    Some(match class {
        "String" => Kind::String,
        "Boolean" => Kind::Boolean,
        "Number" => Kind::Number,
        "Dictionary" => Kind::Dictionary,
        "Array" | "StringArray" => Kind::Array,
        "Date" => Kind::Date,
        "Data" => Kind::Data,
        _ => return None,
    })
}

/// Glob match with `*` only, as Xcode's file name patterns use.
fn matches_pattern(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((prefix, rest)) => {
            let Some(tail) = name.strip_prefix(prefix) else { return false };
            if rest.is_empty() {
                return true;
            }
            (0..=tail.len()).filter(|&i| tail.is_char_boundary(i)).any(|i| matches_pattern(rest, &tail[i..]))
        }
    }
}

impl Schema {
    /// The schema for a file, by its name, as Xcode picks one.
    pub fn for_file_name(name: &str) -> Option<&'static Schema> {
        super::definitions::ALL.iter().copied().find(|s| s.patterns.iter().any(|p| matches_pattern(p, name)))
    }

    pub fn definition(&self, name: &str) -> Option<&'static Definition> {
        let definitions: &'static [Definition] = self.definitions;
        definitions.binary_search_by(|d| d.name.cmp(name)).ok().map(|ix| &definitions[ix])
    }

    pub fn resolve(&self, class: &str) -> Resolved {
        let mut out = Resolved::UNKNOWN;
        let mut class = class;
        // Definitions refer to each other by name; follow them to a primitive.
        for _ in 0..8 {
            if class == "StringArray" && self.definition(class).is_none() {
                out.kind = Some(Kind::Array);
                out.element = out.element.or(Some("String"));
                return out;
            }
            match self.definition(class) {
                Some(d) => {
                    out.definition = out.definition.or(Some(d));
                    if !d.element.is_empty() {
                        out.element = out.element.or(Some(d.element));
                    }
                    if d.class == class {
                        break;
                    }
                    class = d.class;
                }
                None => {
                    out.kind = primitive(class);
                    return out;
                }
            }
        }
        out
    }

    pub fn root(&self) -> Resolved {
        self.resolve("_root_")
    }

    /// The class of a value's child: a dictionary's entry by key, or an
    /// array's element.
    pub fn child(&self, parent: &Resolved, key: Option<&str>) -> Resolved {
        match key {
            Some(key) => parent.key(key).map_or(Resolved::UNKNOWN, |k| self.resolve(k.class)),
            None => parent.element.map_or(Resolved::UNKNOWN, |class| self.resolve(class)),
        }
    }

    /// Resolves each step of `path` (child indices) from the root.
    pub fn along(&self, root: &Node, path: &[usize]) -> Vec<Resolved> {
        let mut out = vec![self.root()];
        let mut node = root;
        for &ix in path {
            let parent = *out.last().expect("starts with the root");
            let (key, child) = match &node.value {
                Value::Dict(entries) => match entries.get(ix) {
                    Some(e) => (Some(e.key.as_str()), &e.value),
                    None => break,
                },
                Value::Array(items) => match items.get(ix) {
                    Some(n) => (None, n),
                    None => break,
                },
                _ => break,
            };
            out.push(self.child(&parent, key));
            node = child;
        }
        out
    }
}

/// The value a new entry of this class starts with, as Xcode adds it.
pub fn default_value(resolved: &Resolved) -> Value {
    let default = resolved.definition.map_or("", |d| d.default);
    match resolved.kind.unwrap_or(Kind::String) {
        Kind::Boolean => Value::Bool(default == "YES"),
        Kind::Number => match default.parse::<i128>() {
            Ok(i) => Value::Integer(i),
            Err(_) => default.parse::<f64>().map_or(Value::Integer(0), Value::Real),
        },
        Kind::Dictionary => Value::Dict(vec![]),
        Kind::Array => Value::Array(vec![]),
        Kind::Date => Value::Date(super::now_since_2001()),
        Kind::Data => Value::Data(vec![]),
        Kind::String => Value::String(match resolved.choices().first() {
            Some(choice) if default.is_empty() => choice.value.to_string(),
            _ => default.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plist::definitions::{ENTITLEMENTS, INFO_PLIST, PRIVACY};

    #[test]
    fn picks_schemas_by_file_name() {
        assert_eq!(Schema::for_file_name("Info.plist").map(|s| s.name), Some("Info.plist"));
        assert_eq!(Schema::for_file_name("HelloZ-Info.plist").map(|s| s.name), Some("Info.plist"));
        assert_eq!(Schema::for_file_name("InfoTests.plist").map(|s| s.name), Some("Info.plist"));
        assert_eq!(Schema::for_file_name("App.entitlements").map(|s| s.name), Some(ENTITLEMENTS.name));
        assert_eq!(Schema::for_file_name("PrivacyInfo.xcprivacy").map(|s| s.name), Some(PRIVACY.name));
        assert!(Schema::for_file_name("Settings.plist").is_none());
    }

    #[test]
    fn resolves_keys_to_xcode_names_and_types() {
        let root = INFO_PLIST.root();
        assert_eq!(root.kind, Some(Kind::Dictionary));
        let key = root.key("CFBundleDisplayName").unwrap();
        assert_eq!(key.label, "Bundle display name");
        let camera = root.key("NSCameraUsageDescription").unwrap();
        assert_eq!(camera.label, "Privacy - Camera Usage Description");
        let orientations = INFO_PLIST.child(&root, Some("UISupportedInterfaceOrientations"));
        assert_eq!(orientations.kind, Some(Kind::Array));
        let orientation = INFO_PLIST.child(&orientations, None);
        assert_eq!(orientation.kind, Some(Kind::String));
        assert!(orientation.choices().iter().any(|c| c.value == "UIInterfaceOrientationPortrait" && c.label == "Portrait (bottom home button)"));
        let modes = INFO_PLIST.child(&root, Some("UIBackgroundModes"));
        assert!(INFO_PLIST.child(&modes, None).choices().iter().any(|c| c.value == "audio"));
        assert_eq!(INFO_PLIST.child(&root, Some("LSRequiresIPhoneOS")).kind, Some(Kind::Boolean));
        // StringArray isn't defined in the entitlements schema.
        let groups = ENTITLEMENTS.child(&ENTITLEMENTS.root(), Some("com.apple.security.application-groups"));
        assert_eq!(groups.kind, Some(Kind::Array));
        assert_eq!(ENTITLEMENTS.child(&groups, None).kind, Some(Kind::String));
        assert_eq!(ENTITLEMENTS.root().key("com.apple.security.app-sandbox").unwrap().label, "App Sandbox");
    }

    #[test]
    fn follows_paths_through_nested_definitions() {
        let text = "<plist><dict><key>NSAppTransportSecurity</key><dict><key>NSAllowsArbitraryLoads</key><true/></dict></dict></plist>";
        let root = crate::plist::parse_xml(text).unwrap();
        let along = INFO_PLIST.along(&root, &[0, 0]);
        assert_eq!(along.len(), 3);
        assert_eq!(along[1].definition.map(|d| d.name), Some("NSAppTransportSecurityDictionary"));
        assert_eq!(along[2].kind, Some(Kind::Boolean));
        assert_eq!(along[1].key("NSAllowsArbitraryLoads").unwrap().label, "Allow Arbitrary Loads");
    }

    #[test]
    fn defaults_follow_the_class() {
        let root = INFO_PLIST.root();
        assert_eq!(default_value(&INFO_PLIST.child(&root, Some("UIRequiresFullScreen"))), Value::Bool(false));
        assert_eq!(default_value(&INFO_PLIST.child(&root, Some("CFBundleURLTypes"))), Value::Array(vec![]));
        assert_eq!(default_value(&Resolved::UNKNOWN), Value::String(String::new()));
    }

    #[test]
    fn globs() {
        assert!(matches_pattern("*Info.plist", "Info.plist"));
        assert!(matches_pattern("Info*.plist", "Info-iOS.plist"));
        assert!(!matches_pattern("Info*.plist", "MyInfo.plst"));
        assert!(matches_pattern("*.xcprivacy", "PrivacyInfo.xcprivacy"));
    }
}

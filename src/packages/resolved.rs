//! `Package.resolved`: the versions a package graph resolved to. Versions 1
//! (Xcode 12 and earlier), 2 and 3 of the format are read; pins are removed
//! (so the next resolve updates those packages) keeping the file's
//! formatting, `"key" : value` with two-space indents, as SwiftPM writes it.

use anyhow::{Context as _, Result, anyhow};
use serde_json::Value;

/// One resolved package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    pub identity: String,
    /// `remoteSourceControl`, `localSourceControl` or `registry`.
    pub kind: String,
    /// The repository URL (or path, or registry identity).
    pub location: String,
    pub version: Option<String>,
    pub branch: Option<String>,
    pub revision: Option<String>,
}

impl Pin {
    /// What Xcode shows next to the package: its version, else its branch,
    /// else its commit.
    pub fn state_label(&self) -> String {
        match (&self.version, &self.branch, &self.revision) {
            (Some(v), _, _) => v.clone(),
            (None, Some(b), _) => b.clone(),
            (None, None, Some(r)) => r.chars().take(7).collect(),
            _ => String::new(),
        }
    }
}

/// SwiftPM's identity for a package location: its last path component,
/// lowercased, without `.git`.
pub fn identity(location: &str) -> String {
    display_name(location).to_lowercase()
}

/// The package's name as Xcode first shows it, before reading its manifest:
/// the last path component of its URL or path, without `.git`.
pub fn display_name(location: &str) -> String {
    let trimmed = location.trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':']).next().unwrap_or(trimmed);
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

pub fn parse(text: &str) -> Result<Vec<Pin>> {
    let root: Value = serde_json::from_str(text).context("reading Package.resolved")?;
    let str_of = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    let version = root.get("version").and_then(Value::as_u64).unwrap_or(1);
    if version == 1 {
        let pins = root
            .pointer("/object/pins")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Package.resolved has no pins"))?;
        return Ok(pins
            .iter()
            .filter_map(|pin| {
                let location = str_of(pin, "repositoryURL")?;
                let state = pin.get("state");
                Some(Pin {
                    identity: identity(&location),
                    kind: if location.starts_with('/') { "localSourceControl" } else { "remoteSourceControl" }.into(),
                    version: state.and_then(|s| str_of(s, "version")),
                    branch: state.and_then(|s| str_of(s, "branch")),
                    revision: state.and_then(|s| str_of(s, "revision")),
                    location,
                })
            })
            .collect());
    }
    let pins = root.get("pins").and_then(Value::as_array).ok_or_else(|| anyhow!("Package.resolved has no pins"))?;
    Ok(pins
        .iter()
        .filter_map(|pin| {
            let location = str_of(pin, "location")?;
            let state = pin.get("state");
            Some(Pin {
                identity: str_of(pin, "identity").unwrap_or_else(|| identity(&location)),
                kind: str_of(pin, "kind").unwrap_or_default(),
                version: state.and_then(|s| str_of(s, "version")),
                branch: state.and_then(|s| str_of(s, "branch")),
                revision: state.and_then(|s| str_of(s, "revision")),
                location,
            })
        })
        .collect())
}

/// `text` without the pins of `identities`, or `None` when it has none of
/// them. The resolver re-resolves (updates) packages that have no pin.
pub fn without_pins(text: &str, identities: &[String]) -> Result<Option<String>> {
    let mut root: Value = serde_json::from_str(text).context("reading Package.resolved")?;
    let matches = |pin: &Value| {
        let id = pin
            .get("identity")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| pin.get("repositoryURL").and_then(Value::as_str).map(identity))
            .unwrap_or_default();
        identities.iter().any(|i| i.eq_ignore_ascii_case(&id))
    };
    let pins = match root.get("version").and_then(Value::as_u64).unwrap_or(1) {
        1 => root.pointer_mut("/object/pins"),
        _ => root.get_mut("pins"),
    }
    .and_then(Value::as_array_mut)
    .ok_or_else(|| anyhow!("Package.resolved has no pins"))?;
    let before = pins.len();
    pins.retain(|pin| !matches(pin));
    if pins.len() == before {
        return Ok(None);
    }
    Ok(Some(to_swiftpm_json(&root)))
}

/// Pretty-prints JSON the way SwiftPM writes `Package.resolved`.
pub(crate) fn to_swiftpm_json(value: &Value) -> String {
    fn write(out: &mut String, value: &Value, depth: usize) {
        let pad = |n: usize| "  ".repeat(n);
        match value {
            Value::Object(map) if !map.is_empty() => {
                out.push_str("{\n");
                for (ix, (key, value)) in map.iter().enumerate() {
                    out.push_str(&pad(depth + 1));
                    out.push_str(&serde_json::to_string(key).unwrap_or_default());
                    out.push_str(" : ");
                    write(out, value, depth + 1);
                    if ix + 1 < map.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad(depth));
                out.push('}');
            }
            Value::Array(items) if !items.is_empty() => {
                out.push_str("[\n");
                for (ix, item) in items.iter().enumerate() {
                    out.push_str(&pad(depth + 1));
                    write(out, item, depth + 1);
                    if ix + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad(depth));
                out.push(']');
            }
            // SwiftPM writes an empty list with a blank line inside.
            Value::Array(_) => {
                out.push_str("[\n\n");
                out.push_str(&pad(depth));
                out.push(']');
            }
            other => out.push_str(&serde_json::to_string(other).unwrap_or_default()),
        }
    }
    let mut out = String::new();
    write(&mut out, value, 0);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const V3: &str = r#"{
  "originHash" : "59376cc9f0ef19d5edd0a1661bb304fcfd5a4f79e7d95ec4782f9b2e13abb766",
  "pins" : [
    {
      "identity" : "swift-argument-parser",
      "kind" : "remoteSourceControl",
      "location" : "https://github.com/apple/swift-argument-parser",
      "state" : {
        "revision" : "41982a3656a71c768319979febd796c6fd111d5c",
        "version" : "1.5.0"
      }
    },
    {
      "identity" : "tally",
      "kind" : "localSourceControl",
      "location" : "/tmp/zcode-tally/Tally.git",
      "state" : {
        "branch" : "develop",
        "revision" : "e6d008ffd23557380a135562ee0a7ebb9c773afe"
      }
    }
  ],
  "version" : 3
}
"#;

    const V1: &str = r#"{
  "object": {
    "pins": [
      {
        "package": "Alamofire",
        "repositoryURL": "https://github.com/Alamofire/Alamofire.git",
        "state": {
          "branch": null,
          "revision": "f96b619bcb2383b43d898402283924b80e2c4bae",
          "version": "5.4.3"
        }
      }
    ]
  },
  "version": 1
}
"#;

    #[test]
    fn reads_version_3() {
        let pins = parse(V3).unwrap();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].identity, "swift-argument-parser");
        assert_eq!(pins[0].state_label(), "1.5.0");
        assert_eq!(pins[1].kind, "localSourceControl");
        assert_eq!(pins[1].state_label(), "develop");
    }

    #[test]
    fn reads_version_1() {
        let pins = parse(V1).unwrap();
        assert_eq!(pins[0].identity, "alamofire");
        assert_eq!(pins[0].version.as_deref(), Some("5.4.3"));
        assert_eq!(pins[0].branch, None);
    }

    #[test]
    fn removes_pins_in_swiftpm_format() {
        let out = without_pins(V3, &["Tally".into()]).unwrap().unwrap();
        let expected = V3.replace(
            ",\n    {\n      \"identity\" : \"tally\",\n      \"kind\" : \"localSourceControl\",\n      \"location\" : \"/tmp/zcode-tally/Tally.git\",\n      \"state\" : {\n        \"branch\" : \"develop\",\n        \"revision\" : \"e6d008ffd23557380a135562ee0a7ebb9c773afe\"\n      }\n    }",
            "",
        );
        assert_eq!(out, expected);
        let all = without_pins(&out, &["swift-argument-parser".into()]).unwrap().unwrap();
        assert!(all.contains("\"pins\" : [\n\n  ],"), "{all}");
        assert!(parse(&all).unwrap().is_empty());
        assert_eq!(without_pins(V3, &["missing".into()]).unwrap(), None);
        assert!(without_pins(V1, &["alamofire".into()]).unwrap().is_some());
    }

    #[test]
    fn names_from_locations() {
        assert_eq!(identity("https://github.com/apple/swift-collections.git"), "swift-collections");
        assert_eq!(display_name("git@github.com:Alamofire/Alamofire.git"), "Alamofire");
        assert_eq!(display_name("/Users/me/Packages/ZKit/"), "ZKit");
        assert_eq!(identity("file:///tmp/zcode-tally/Tally.git"), "tally");
    }
}

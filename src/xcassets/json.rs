//! `Contents.json` as Xcode writes it: what `NSJSONSerialization` prints
//! with `NSJSONWritingPrettyPrinted | NSJSONWritingSortedKeys` (two-space
//! indent, `"key" : value`, keys sorted, empty containers on three lines,
//! `/` escaped), and a final newline. A file Xcode wrote, read and written
//! again unchanged, comes out byte for byte.

use serde_json::Value;

/// `value` as Xcode writes a `Contents.json`.
pub fn to_xcode_json(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0);
    out.push('\n');
    out
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn write_value(out: &mut String, value: &Value, level: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push_str("[\n");
            for (ix, item) in items.iter().enumerate() {
                indent(out, level + 1);
                write_value(out, item, level + 1);
                if ix + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            if items.is_empty() {
                out.push('\n');
            }
            indent(out, level);
            out.push(']');
        }
        Value::Object(map) => {
            out.push_str("{\n");
            // serde_json keeps insertion order here (`preserve_order` is on
            // through other crates); Xcode sorts.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (ix, key) in keys.iter().enumerate() {
                indent(out, level + 1);
                write_string(out, key);
                out.push_str(" : ");
                write_value(out, &map[key.as_str()], level + 1);
                if ix + 1 < keys.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            if map.is_empty() {
                out.push('\n');
            }
            indent(out, level);
            out.push('}');
        }
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' => out.push_str("\\/"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A new set's or folder's `Contents.json` body: the `info` block Xcode
/// writes in every one.
pub fn info() -> Value {
    serde_json::json!({ "author": "xcode", "version": 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CATALOG: &str = include_str!("../../samples/HelloClassic/HelloClassic/Assets.xcassets/Contents.json");
    const ACCENT: &str = include_str!("../../samples/HelloClassic/HelloClassic/Assets.xcassets/AccentColor.colorset/Contents.json");

    fn round_trip(text: &str) -> String {
        to_xcode_json(&serde_json::from_str(text).unwrap())
    }

    #[test]
    fn xcode_files_come_back_byte_for_byte() {
        assert_eq!(round_trip(CATALOG), CATALOG);
        assert_eq!(round_trip(ACCENT), ACCENT);
        for fixture in super::super::tests::FIXTURES {
            assert_eq!(round_trip(fixture), *fixture);
        }
    }

    #[test]
    fn keys_are_sorted_whatever_order_they_were_in() {
        let value = json!({ "info": { "version": 1, "author": "xcode" }, "colors": [] });
        assert_eq!(
            to_xcode_json(&value),
            "{\n  \"colors\" : [\n\n  ],\n  \"info\" : {\n    \"author\" : \"xcode\",\n    \"version\" : 1\n  }\n}\n"
        );
    }

    #[test]
    fn strings_are_escaped_as_nsjsonserialization_does() {
        // Checked against NSJSONSerialization (see `matches_nsjsonserialization`).
        let value = json!({ "f": "a/b é \"q\" \\ \t.png", "e": {} });
        assert_eq!(to_xcode_json(&value), "{\n  \"e\" : {\n\n  },\n  \"f\" : \"a\\/b é \\\"q\\\" \\\\ \\t.png\"\n}\n");
    }

    /// Compares with Foundation's own output (needs `swiftc` from Xcode).
    #[test]
    #[ignore]
    fn matches_nsjsonserialization() {
        let dir = std::env::temp_dir().join(format!("zcode-json-oracle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("oracle.swift");
        std::fs::write(
            &source,
            r#"import Foundation
let input = FileHandle.standardInput.readDataToEndOfFile()
let object = try JSONSerialization.jsonObject(with: input, options: [.fragmentsAllowed])
let data = try JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys])
FileHandle.standardOutput.write(data)
"#,
        )
        .unwrap();
        let binary = dir.join("oracle");
        let built = std::process::Command::new("xcrun").args(["swiftc", "-O"]).arg(&source).arg("-o").arg(&binary).status().unwrap();
        assert!(built.success());
        let samples = [
            json!({ "images": [{ "filename": "a b/c.png", "idiom": "universal", "scale": "2x" }, { "idiom": "iphone" }], "info": info() }),
            json!({ "colors": [{ "color": { "color-space": "display-p3", "components": { "alpha": "0.500", "blue": "0x1F", "green": "128", "red": "1.000" } }, "idiom": "universal", "appearances": [{ "appearance": "luminosity", "value": "dark" }] }], "info": info() }),
            json!({ "properties": { "provides-namespace": true, "preserves-vector-representation": false, "localizable": true }, "data": [], "z": {}, "tab\tctl\u{1}": "ü\u{1f}" }),
            json!({ "nested": [[1, 2], [], [{}]], "n": null, "x": 3 }),
        ];
        for value in samples {
            let mut child = std::process::Command::new(&binary)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write as _;
            child.stdin.take().unwrap().write_all(value.to_string().as_bytes()).unwrap();
            let out = child.wait_with_output().unwrap();
            let expected = String::from_utf8(out.stdout).unwrap() + "\n";
            assert_eq!(to_xcode_json(&value), expected, "{value}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}

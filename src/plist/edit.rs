//! Structured edits to an XML property list as byte-range replacements of
//! its text: setting a value or key, adding, removing and moving entries.
//! Everything outside the replaced range stays as it was, so a file Xcode
//! wrote keeps its layout, and each edit is one undoable text change.

use std::ops::Range;

use super::{Node, Value, escape, write_value_in};

#[derive(Clone, Debug, PartialEq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub text: String,
}

impl TextEdit {
    pub fn apply(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len() + self.text.len());
        out.push_str(&text[..self.range.start]);
        out.push_str(&self.text);
        out.push_str(&text[self.range.end..]);
        out
    }
}

/// A parsed property list and its text.
pub struct Document<'a> {
    pub text: &'a str,
    pub root: &'a Node,
    /// One level of indentation: a tab, as Xcode writes, unless the file
    /// uses spaces.
    unit: String,
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |i| i + 1)
}

/// The start of `offset`'s line when only whitespace precedes it there.
fn own_line_start(text: &str, offset: usize) -> Option<usize> {
    let start = line_start(text, offset);
    text[start..offset].chars().all(|c| c == ' ' || c == '\t').then_some(start)
}

/// Just past the newline ending `offset`'s line, when only whitespace
/// follows it there.
fn own_line_end(text: &str, offset: usize) -> Option<usize> {
    let rest = &text[offset..];
    let end = rest.find('\n').map_or(rest.len(), |i| i + 1);
    rest[..end].trim_end_matches('\n').chars().all(|c| c == ' ' || c == '\t' || c == '\r').then_some(offset + end)
}

/// A unique key for a new entry: "New item", then "New item - 2", … as
/// Xcode names them.
pub fn unique_key(parent: &Node, base: &str) -> String {
    let taken = |key: &str| parent.get(key).is_some();
    if !taken(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base} - {n}")).find(|k| !taken(k)).expect("some key is free")
}

impl<'a> Document<'a> {
    pub fn new(text: &'a str, root: &'a Node) -> Self {
        let first_child = match &root.value {
            Value::Dict(entries) => entries.first().map(|e| e.key_span.start),
            Value::Array(items) => items.first().map(|n| n.span.start),
            _ => None,
        };
        let unit = first_child
            .and_then(|at| own_line_start(text, at).map(|start| &text[start..at]))
            .filter(|indent| !indent.is_empty() && !indent.starts_with('\t'))
            .map_or_else(|| "\t".to_string(), str::to_string);
        Self { text, root, unit }
    }

    fn indent(&self, depth: usize) -> String {
        self.unit.repeat(depth)
    }

    fn value_text(&self, value: &Value, depth: usize) -> String {
        let mut out = String::new();
        write_value_in(&mut out, value, depth, &self.unit);
        out
    }

    /// The range a child occupies: its key (in a dictionary) through its value.
    fn child_range(&self, parent: &Node, ix: usize) -> Option<Range<usize>> {
        match &parent.value {
            Value::Dict(entries) => entries.get(ix).map(|e| e.key_span.start..e.value.span.end),
            Value::Array(items) => items.get(ix).map(|n| n.span.clone()),
            _ => None,
        }
    }

    /// An entry's text: `<key>…</key>` (in a dictionary), then the value on
    /// the next line, or right after the key when `inline`.
    fn entry_text(&self, key: Option<&str>, value: &Value, depth: usize, inline: bool) -> String {
        let value = self.value_text(value, depth);
        match key {
            Some(key) if inline => format!("<key>{}</key>{value}", escape(key)),
            Some(key) => format!("<key>{}</key>\n{}{value}", escape(key), self.indent(depth)),
            None => value,
        }
    }

    /// Replaces the value at `path` (keeping its key).
    pub fn set_value(&self, path: &[usize], value: &Value) -> Option<TextEdit> {
        let node = self.root.at(path)?;
        Some(TextEdit { range: node.span.clone(), text: self.value_text(value, path.len()) })
    }

    pub fn set_key(&self, path: &[usize], key: &str) -> Option<TextEdit> {
        let (last, parent_path) = path.split_last()?;
        let Value::Dict(entries) = &self.root.at(parent_path)?.value else { return None };
        let entry = entries.get(*last)?;
        Some(TextEdit { range: entry.key_span.clone(), text: format!("<key>{}</key>", escape(key)) })
    }

    pub fn remove(&self, path: &[usize]) -> Option<TextEdit> {
        let (&ix, parent_path) = path.split_last()?;
        let parent = self.root.at(parent_path)?;
        let range = self.child_range(parent, ix)?;
        if parent.children_len() == 1 {
            // The last one: `<dict/>`, as Xcode writes an empty container.
            let empty = match parent.value {
                Value::Dict(_) => "<dict/>",
                _ => "<array/>",
            };
            return Some(TextEdit { range: parent.span.clone(), text: empty.to_string() });
        }
        let range = match (own_line_start(self.text, range.start), own_line_end(self.text, range.end)) {
            (Some(start), Some(end)) => start..end,
            _ => range,
        };
        Some(TextEdit { range, text: String::new() })
    }

    /// Adds a child at `index` of the container at `parent_path` (a key for
    /// dictionaries, `None` for arrays).
    pub fn insert(&self, parent_path: &[usize], index: usize, key: Option<&str>, value: &Value) -> Option<TextEdit> {
        let parent = self.root.at(parent_path)?;
        let depth = parent_path.len() + 1;
        let count = parent.children_len();
        if count == 0 {
            let child = Node::new(value.clone());
            let container = match (&parent.value, key) {
                (Value::Dict(_), Some(key)) => Value::Dict(vec![super::Entry { key: key.to_string(), key_span: 0..0, value: child }]),
                (Value::Array(_), None) => Value::Array(vec![child]),
                _ => return None,
            };
            return self.set_value(parent_path, &container);
        }
        let index = index.min(count);
        if index < count {
            let at = self.child_range(parent, index)?.start;
            return Some(match own_line_start(self.text, at) {
                Some(line) => TextEdit {
                    range: line..line,
                    text: format!("{}{}\n", self.text[line..at].to_string(), self.entry_text(key, value, depth, false)),
                },
                None => TextEdit { range: at..at, text: self.entry_text(key, value, depth, true) },
            });
        }
        let last = self.child_range(parent, count - 1)?;
        let indent = own_line_start(self.text, last.start).map(|line| self.text[line..last.start].to_string());
        Some(match indent {
            Some(indent) => TextEdit { range: last.end..last.end, text: format!("\n{indent}{}", self.entry_text(key, value, depth, false)) },
            None => TextEdit { range: last.end..last.end, text: self.entry_text(key, value, depth, true) },
        })
    }

    /// Moves a container's child from `from` to `to` (both indices before
    /// the move), keeping each child's text and the space between them.
    pub fn move_child(&self, parent_path: &[usize], from: usize, to: usize) -> Option<TextEdit> {
        let parent = self.root.at(parent_path)?;
        let count = parent.children_len();
        if from >= count || to >= count || from == to {
            return None;
        }
        let (lo, hi) = (from.min(to), from.max(to));
        let ranges: Vec<Range<usize>> = (lo..=hi).map(|ix| self.child_range(parent, ix)).collect::<Option<_>>()?;
        let mut order: Vec<usize> = (0..ranges.len()).collect();
        let moved = order.remove(from - lo);
        order.insert(to - lo, moved);
        let mut text = String::new();
        for (slot, &ix) in order.iter().enumerate() {
            text.push_str(&self.text[ranges[ix].clone()]);
            if let Some(next) = ranges.get(slot + 1) {
                text.push_str(&self.text[ranges[slot].end..next.start]);
            }
        }
        Some(TextEdit { range: ranges[0].start..ranges[ranges.len() - 1].end, text })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plist::{Entry, parse_xml, to_xml};

    const INFO: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>CFBundleDisplayName</key>\n\t<string>Hello</string>\n\t<key>UIBackgroundModes</key>\n\t<array>\n\t\t<string>audio</string>\n\t</array>\n\t<key>LSRequiresIPhoneOS</key>\n\t<true/>\n</dict>\n</plist>\n";

    fn edit(text: &str, f: impl Fn(&Document) -> Option<TextEdit>) -> String {
        let root = parse_xml(text).unwrap();
        let doc = Document::new(text, &root);
        let out = f(&doc).unwrap().apply(text);
        parse_xml(&out).unwrap_or_else(|e| panic!("{e:#}\n{out}"));
        out
    }

    /// Edits must leave the file as Xcode would write the edited value.
    fn assert_like_xcode(out: &str) {
        assert_eq!(to_xml(&parse_xml(out).unwrap().value), out);
    }

    #[test]
    fn sets_values_and_keys_in_place() {
        let out = edit(INFO, |d| d.set_value(&[0], &Value::String("Hi & bye".into())));
        assert!(out.contains("\t<string>Hi &amp; bye</string>\n\t<key>UIBackgroundModes"));
        assert_like_xcode(&out);
        let out = edit(INFO, |d| d.set_key(&[2], "UIRequiresFullScreen"));
        assert!(out.contains("<key>UIRequiresFullScreen</key>\n\t<true/>"));
        let out = edit(INFO, |d| d.set_value(&[1, 0], &Value::String("fetch".into())));
        assert!(out.contains("\t\t<string>fetch</string>\n\t</array>"));
        // A container value is written at its depth.
        let nested = Value::Dict(vec![Entry { key: "a".into(), key_span: 0..0, value: Node::new(Value::Array(vec![Node::new(Value::Integer(1))])) }]);
        let out = edit(INFO, |d| d.set_value(&[0], &nested));
        assert!(out.contains("\t<key>CFBundleDisplayName</key>\n\t<dict>\n\t\t<key>a</key>\n\t\t<array>\n\t\t\t<integer>1</integer>\n\t\t</array>\n\t</dict>\n"), "{out}");
        assert_like_xcode(&out);
    }

    #[test]
    fn inserts_where_asked() {
        let out = edit(INFO, |d| d.insert(&[], 1, Some("New item"), &Value::String(String::new())));
        assert!(out.contains("<string>Hello</string>\n\t<key>New item</key>\n\t<string></string>\n\t<key>UIBackgroundModes</key>"), "{out}");
        assert_like_xcode(&out);
        let out = edit(INFO, |d| d.insert(&[], 3, Some("Last"), &Value::Bool(false)));
        assert!(out.ends_with("<true/>\n\t<key>Last</key>\n\t<false/>\n</dict>\n</plist>\n"), "{out}");
        let out = edit(INFO, |d| d.insert(&[], 0, Some("First"), &Value::Array(vec![])));
        assert!(out.contains("<dict>\n\t<key>First</key>\n\t<array/>\n\t<key>CFBundleDisplayName</key>"), "{out}");
        let out = edit(INFO, |d| d.insert(&[1], 1, None, &Value::String("fetch".into())));
        assert!(out.contains("\t\t<string>audio</string>\n\t\t<string>fetch</string>\n\t</array>"), "{out}");
        assert_like_xcode(&out);
    }

    #[test]
    fn inserts_into_empty_containers() {
        let text = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict/>\n</plist>\n";
        let out = edit(text, |d| d.insert(&[], 0, Some("com.apple.security.app-sandbox"), &Value::Bool(true)));
        assert_eq!(out, text.replace("<dict/>", "<dict>\n\t<key>com.apple.security.app-sandbox</key>\n\t<true/>\n</dict>"));
        let out = edit(&out, |d| d.insert(&[], 1, Some("groups"), &Value::Array(vec![])));
        let out = edit(&out, |d| d.insert(&[1], 0, None, &Value::String("group.a".into())));
        assert!(out.contains("\t<key>groups</key>\n\t<array>\n\t\t<string>group.a</string>\n\t</array>\n</dict>"), "{out}");
        assert_like_xcode(&out);
    }

    #[test]
    fn removes_whole_lines_and_empties_containers() {
        let out = edit(INFO, |d| d.remove(&[0]));
        assert!(out.contains("<dict>\n\t<key>UIBackgroundModes</key>"), "{out}");
        assert_like_xcode(&out);
        let out = edit(INFO, |d| d.remove(&[1, 0]));
        assert!(out.contains("<key>UIBackgroundModes</key>\n\t<array/>\n"), "{out}");
        assert_like_xcode(&out);
        let out = edit(INFO, |d| d.remove(&[2]));
        assert!(out.contains("\t</array>\n</dict>"), "{out}");
    }

    #[test]
    fn moves_children_keeping_their_text() {
        let out = edit(INFO, |d| d.move_child(&[], 2, 0));
        assert!(out.contains("<dict>\n\t<key>LSRequiresIPhoneOS</key>\n\t<true/>\n\t<key>CFBundleDisplayName</key>\n\t<string>Hello</string>\n\t<key>UIBackgroundModes</key>"), "{out}");
        assert_like_xcode(&out);
        let back = edit(&out, |d| d.move_child(&[], 0, 2));
        assert_eq!(back, INFO);
    }

    #[test]
    fn keeps_unusual_layouts() {
        // Spaces and a comment survive edits elsewhere.
        let text = "<plist>\n<dict>\n  <!-- keep -->\n  <key>a</key>\n  <string>x</string>\n</dict>\n</plist>\n";
        let out = edit(text, |d| d.insert(&[], 1, Some("b"), &Value::Array(vec![Node::new(Value::Bool(true))])));
        assert_eq!(out, "<plist>\n<dict>\n  <!-- keep -->\n  <key>a</key>\n  <string>x</string>\n  <key>b</key>\n  <array>\n    <true/>\n  </array>\n</dict>\n</plist>\n");
        // Everything on one line.
        let text = "<plist><dict><key>a</key><string>x</string><key>b</key><true/></dict></plist>";
        let out = edit(text, |d| d.remove(&[0]));
        assert_eq!(out, "<plist><dict><key>b</key><true/></dict></plist>");
        let out = edit(text, |d| d.insert(&[], 1, Some("c"), &Value::Integer(3)));
        assert_eq!(out, "<plist><dict><key>a</key><string>x</string><key>c</key><integer>3</integer><key>b</key><true/></dict></plist>");
    }

    #[test]
    fn unique_keys() {
        let root = parse_xml("<plist><dict><key>New item</key><true/><key>New item - 2</key><true/></dict></plist>").unwrap();
        assert_eq!(unique_key(&root, "New item"), "New item - 3");
        assert_eq!(unique_key(&root, "Other"), "Other");
    }
}

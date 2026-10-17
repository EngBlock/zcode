//! JSON with comments and trailing commas, as Zed's settings and keymap
//! files are written: read with the error's line and column, and edited in
//! place so a value zcode sets (a menu toggle, an import) leaves the rest of
//! the file, comments included, as the user wrote it.

use std::ops::Range;

use serde::de::DeserializeOwned;
use serde_json::Value;

/// Where a file stopped making sense, 1-based as editors show it.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}, column {}: {}", self.line, self.column, self.message)
    }
}

/// The text with its comments and trailing commas blanked out, byte for
/// byte (newlines kept), so positions in it are positions in the file.
pub fn strip(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    let mut i = 0;
    // The last comma outside a string, and whether only blanks followed it.
    let mut pending_comma: Option<usize> = None;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                pending_comma = None;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let end = text[i + 2..].find("*/").map_or(bytes.len(), |e| i + 2 + e + 2);
                for byte in &mut out[i..end] {
                    if *byte != b'\n' {
                        *byte = b' ';
                    }
                }
                i = end;
            }
            b',' => {
                pending_comma = Some(i);
                i += 1;
            }
            b'}' | b']' => {
                if let Some(comma) = pending_comma.take() {
                    out[comma] = b' ';
                }
                i += 1;
            }
            byte if byte.is_ascii_whitespace() => i += 1,
            _ => {
                pending_comma = None;
                i += 1;
            }
        }
    }
    // Only ASCII bytes were replaced (with spaces), so this stays UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

/// Parses a JSONC file into `T`. An empty file is `T`'s default (`{}`).
pub fn parse<T: DeserializeOwned + Default>(text: &str) -> Result<T, ParseError> {
    let stripped = strip(text);
    if stripped.trim().is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str(&stripped).map_err(|err| error_at(err, &stripped))
}

/// `parse` into a JSON value (`null` for an empty file).
pub fn parse_value(text: &str) -> Result<Value, ParseError> {
    let stripped = strip(text);
    if stripped.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&stripped).map_err(|err| error_at(err, &stripped))
}

/// How a setting's own check begins its message: serde_json places those
/// after the value (past the newline that follows it), so they're moved
/// back onto it.
pub const VALUE_ERROR: &str = "\u{1}expected ";
const VALUE_ERROR_MARK: &str = "\u{1}";

fn error_at(err: serde_json::Error, text: &str) -> ParseError {
    let message = err.to_string();
    // serde_json appends " at line L column C"; the fields carry them.
    let message = match message.rfind(" at line ") {
        Some(at) => message[..at].to_string(),
        None => message,
    };
    let (mut line, mut column) = if err.line() == 0 { (text.lines().count().max(1), 1) } else { (err.line(), err.column()) };
    let message = match message.strip_prefix(VALUE_ERROR_MARK) {
        Some(rest) => {
            // Back over the blanks after the value, to its last character.
            let line_start: usize = text.split_inclusive('\n').take(line - 1).map(str::len).sum();
            let mut offset = (line_start + column.saturating_sub(1)).min(text.len());
            while offset > 0 && text.as_bytes()[offset - 1].is_ascii_whitespace() {
                offset -= 1;
            }
            line = text[..offset].matches('\n').count() + 1;
            column = offset - text[..offset].rfind('\n').map_or(0, |n| n + 1);
            rest.to_string()
        }
        None => message,
    };
    ParseError { line, column, message }
}

// ---------------------------------------------------------------------------
// The file's structure with byte ranges, for edits in place.

#[derive(Debug)]
pub enum Node {
    Object { range: Range<usize>, members: Vec<Member> },
    Array { range: Range<usize>, items: Vec<Node> },
    Scalar { range: Range<usize> },
}

#[derive(Debug)]
pub struct Member {
    pub key: String,
    pub key_range: Range<usize>,
    pub value: Node,
}

impl Node {
    pub fn range(&self) -> Range<usize> {
        match self {
            Node::Object { range, .. } | Node::Array { range, .. } | Node::Scalar { range } => range.clone(),
        }
    }
}

/// The structure of a file that `parse_value` accepts.
pub fn tree(text: &str) -> Result<Node, ParseError> {
    parse_value(text)?;
    let stripped = strip(text);
    let mut parser = TreeParser { text: stripped.as_bytes(), at: 0 };
    parser.value().ok_or(ParseError { line: 1, column: 1, message: "expected a value".into() })
}

struct TreeParser<'a> {
    text: &'a [u8],
    at: usize,
}

impl TreeParser<'_> {
    fn skip_blanks(&mut self) {
        while self.at < self.text.len() && (self.text[self.at].is_ascii_whitespace() || self.text[self.at] == b',') {
            self.at += 1;
        }
    }

    fn value(&mut self) -> Option<Node> {
        self.skip_blanks();
        let start = self.at;
        match *self.text.get(self.at)? {
            b'{' => {
                self.at += 1;
                let mut members = vec![];
                loop {
                    self.skip_blanks();
                    if *self.text.get(self.at)? == b'}' {
                        self.at += 1;
                        break;
                    }
                    let key_start = self.at;
                    self.string();
                    let key_range = key_start..self.at;
                    let key = serde_json::from_slice(&self.text[key_range.clone()]).ok()?;
                    self.skip_blanks();
                    if self.text.get(self.at) == Some(&b':') {
                        self.at += 1;
                    }
                    let value = self.value()?;
                    members.push(Member { key, key_range, value });
                }
                Some(Node::Object { range: start..self.at, members })
            }
            b'[' => {
                self.at += 1;
                let mut items = vec![];
                loop {
                    self.skip_blanks();
                    if *self.text.get(self.at)? == b']' {
                        self.at += 1;
                        break;
                    }
                    items.push(self.value()?);
                }
                Some(Node::Array { range: start..self.at, items })
            }
            b'"' => {
                self.string();
                Some(Node::Scalar { range: start..self.at })
            }
            _ => {
                while self.at < self.text.len() && !matches!(self.text[self.at], b',' | b'}' | b']') && !self.text[self.at].is_ascii_whitespace() {
                    self.at += 1;
                }
                Some(Node::Scalar { range: start..self.at })
            }
        }
    }

    fn string(&mut self) {
        self.at += 1;
        while self.at < self.text.len() && self.text[self.at] != b'"' {
            self.at += if self.text[self.at] == b'\\' { 2 } else { 1 };
        }
        self.at += 1;
    }
}

/// The indentation of the line `offset` is on.
fn indentation_at(text: &str, offset: usize) -> &str {
    let line_start = text[..offset].rfind('\n').map_or(0, |n| n + 1);
    let line = &text[line_start..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// `value` as pretty JSON whose lines after the first are indented by
/// `indent`.
pub fn to_json_at(value: &Value, indent: &str) -> String {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into());
    pretty.replace('\n', &format!("\n{indent}"))
}

fn nested(path: &[&str], value: &Value) -> Value {
    path.iter().rev().fold(value.clone(), |inner, key| {
        let mut map = serde_json::Map::new();
        map.insert((*key).to_string(), inner);
        Value::Object(map)
    })
}

/// Sets the value at `path` (object keys from the top), adding the objects
/// on the way that are missing, and keeps everything else as it was. An
/// empty file becomes `{ … }`.
pub fn set(text: &str, path: &[&str], value: &Value) -> Result<String, ParseError> {
    if strip(text).trim().is_empty() {
        return Ok(format!("{}\n", to_json_at(&nested(path, value), "")));
    }
    let root = tree(text)?;
    let mut node = &root;
    let mut depth = 0;
    loop {
        let Node::Object { range, members } = node else {
            // Not an object where the path goes: replace it.
            let range = node.range();
            let indent = indentation_at(text, range.start).to_string();
            return Ok(splice(text, range, &to_json_at(&nested(&path[depth..], value), &indent)));
        };
        let key = path[depth];
        match members.iter().find(|m| m.key == key) {
            Some(member) if depth + 1 == path.len() => {
                let range = member.value.range();
                let indent = indentation_at(text, member.key_range.start).to_string();
                return Ok(splice(text, range, &to_json_at(value, &indent)));
            }
            Some(member) => {
                node = &member.value;
                depth += 1;
            }
            None => {
                let rest = &path[depth + 1..];
                let inner = nested(rest, value);
                return Ok(insert_member(text, range.clone(), members, key, &inner));
            }
        }
    }
}

fn insert_member(text: &str, object: Range<usize>, members: &[Member], key: &str, value: &Value) -> String {
    let key = serde_json::to_string(key).unwrap_or_default();
    match members.last() {
        Some(last) => {
            let indent = indentation_at(text, last.key_range.start).to_string();
            let entry = format!(",\n{indent}{key}: {}", to_json_at(value, &indent));
            let end = last.value.range().end;
            splice(text, end..end, &entry)
        }
        None => {
            let outer = indentation_at(text, object.start).to_string();
            let indent = format!("{outer}  ");
            let entry = format!("\n{indent}{key}: {}\n{outer}", to_json_at(value, &indent));
            // Between the braces, replacing whatever blank space was there.
            splice(text, object.start + 1..object.end - 1, &entry)
        }
    }
}

fn splice(text: &str, range: Range<usize>, with: &str) -> String {
    let mut out = String::with_capacity(text.len() + with.len());
    out.push_str(&text[..range.start]);
    out.push_str(with);
    out.push_str(&text[range.end..]);
    out
}

/// Replaces item `index` of the top-level array with `item`, or appends it
/// (`index` past the end); an empty file becomes `[ item ]`. `comment`
/// goes on the lines above an appended item.
pub fn set_array_item(text: &str, index: usize, item: &Value, comment: Option<&str>) -> Result<String, ParseError> {
    let comment_lines = |indent: &str| {
        comment.map(|c| c.lines().map(|line| format!("{indent}// {line}\n")).collect::<String>()).unwrap_or_default()
    };
    if strip(text).trim().is_empty() {
        return Ok(format!("[\n{}  {}\n]\n", comment_lines("  "), to_json_at(item, "  ")));
    }
    let Node::Array { range, items } = tree(text)? else {
        return Err(ParseError { line: 1, column: 1, message: "expected a list of binding blocks ([ … ])".into() });
    };
    if let Some(existing) = items.get(index) {
        let range = existing.range();
        let indent = indentation_at(text, range.start).to_string();
        return Ok(splice(text, range, &to_json_at(item, &indent)));
    }
    match items.last() {
        Some(last) => {
            let indent = indentation_at(text, last.range().start).to_string();
            let end = last.range().end;
            let entry = format!(",\n{}{indent}{}", comment_lines(&indent), to_json_at(item, &indent));
            Ok(splice(text, end..end, &entry))
        }
        None => {
            let entry = format!("\n{}  {}\n", comment_lines("  "), to_json_at(item, "  "));
            Ok(splice(text, range.start + 1..range.end - 1, &entry))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn comments_and_trailing_commas_are_read_and_errors_have_their_place() {
        let text = "{\n  // the size\n  \"tab_size\": 2, /* two */\n  \"url\": \"http://a//b\",\n}\n";
        let value = parse_value(text).unwrap();
        assert_eq!(value, json!({ "tab_size": 2, "url": "http://a//b" }));
        assert_eq!(strip(text).len(), text.len());
        let err = parse_value("{\n  \"tab_size\": 2\n  \"soft_wrap\": true\n}").unwrap_err();
        assert_eq!((err.line, err.column), (3, 3), "{err}");
        assert!(err.message.starts_with("expected `,`"), "{err}");
        assert_eq!(parse_value("  // nothing\n").unwrap(), Value::Null);
        #[derive(serde::Deserialize, Default, Debug)]
        struct Typed {
            #[allow(dead_code)]
            tab_size: u32,
        }
        let err = parse::<Typed>("{\n  \"tab_size\": \"four\"\n}").unwrap_err();
        assert_eq!(err.line, 2);
        assert!(err.message.contains("invalid type: string \"four\""), "{err}");
    }

    #[test]
    fn values_are_set_in_place_keeping_comments() {
        let text = "{\n  // Fonts\n  \"buffer_font_size\": 13, // points\n  \"inlay_hints\": { \"enabled\": false },\n}\n";
        let edited = set(text, &["buffer_font_size"], &json!(15)).unwrap();
        assert_eq!(edited, text.replace("13", "15"));
        let edited = set(&edited, &["inlay_hints", "enabled"], &json!(true)).unwrap();
        assert!(edited.contains("{ \"enabled\": true }") && edited.contains("// points"), "{edited}");
        let edited = set(&edited, &["git", "inline_blame", "enabled"], &json!(false)).unwrap();
        assert_eq!(
            parse_value(&edited).unwrap(),
            json!({ "buffer_font_size": 15, "inlay_hints": { "enabled": true }, "git": { "inline_blame": { "enabled": false } } })
        );
        assert!(edited.contains("  \"git\": {\n    \"inline_blame\": {\n      \"enabled\": false\n    }\n  }"), "{edited}");
        assert_eq!(set("", &["vim_mode"], &json!(true)).unwrap(), "{\n  \"vim_mode\": true\n}\n");
        assert_eq!(set("{}", &["vim_mode"], &json!(true)).unwrap(), "{\n  \"vim_mode\": true\n}");
        // A scalar where an object should be is replaced.
        let edited = set("{ \"theme\": \"Midnight\" }", &["theme", "dark"], &json!("Dusk")).unwrap();
        assert_eq!(parse_value(&edited).unwrap(), json!({ "theme": { "dark": "Dusk" } }));
        assert!(set("{ \"a\": ", &["a"], &json!(1)).is_err());
    }

    #[test]
    fn array_items_are_replaced_or_appended() {
        let text = "[\n  // mine\n  { \"bindings\": { \"cmd-k\": \"zcode::Build\" } }\n]\n";
        let block = json!({ "bindings": { "f5": "zcode::Run" } });
        let appended = set_array_item(text, 9, &block, Some("Imported")).unwrap();
        assert!(appended.contains("// mine") && appended.contains("  // Imported\n  {"), "{appended}");
        assert_eq!(parse_value(&appended).unwrap().as_array().unwrap().len(), 2);
        let replaced = set_array_item(&appended, 1, &json!({ "bindings": {} }), None).unwrap();
        assert_eq!(parse_value(&replaced).unwrap()[1], json!({ "bindings": {} }));
        let fresh = set_array_item("", 0, &block, None).unwrap();
        assert_eq!(parse_value(&fresh).unwrap(), json!([block]));
        assert!(set_array_item("{}", 0, &block, None).is_err());
    }
}

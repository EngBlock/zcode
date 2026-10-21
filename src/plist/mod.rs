//! Property lists (`Info.plist`, `.entitlements`, `.xcprivacy`, …): a parse
//! of the XML format that keeps every value's byte range, so edits rewrite
//! only what changes (`edit.rs`); the binary format (`binary.rs`); and
//! output in the layout Xcode and CoreFoundation write (tabs, the
//! `<!DOCTYPE>` header, `%.17g` reals, base64 data in 76-column lines).
//!
//! Binary files are edited as XML text and written back as binary.

pub mod binary;
pub mod definitions;
pub mod edit;
pub mod openstep;
pub mod schema;

use std::ops::Range;
use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};

use crate::scheme::xml::{self, Element};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    /// Wide enough for every `<integer>` CoreFoundation writes (`u64` too).
    Integer(i128),
    Real(f64),
    Bool(bool),
    /// Seconds since 2001-01-01T00:00:00Z, as binary plists store dates.
    Date(f64),
    Data(Vec<u8>),
    Array(Vec<Node>),
    Dict(Vec<Entry>),
}

/// A value, and where it is in the XML text it was parsed from (empty
/// ranges for values built in memory or read from a binary file).
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub value: Value,
    /// The value's element, `<string>…</string>`.
    pub span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    /// The `<key>…</key>` element.
    pub key_span: Range<usize>,
    pub value: Node,
}

impl Node {
    pub fn new(value: Value) -> Self {
        Self { value, span: 0..0 }
    }

    pub fn children_len(&self) -> usize {
        match &self.value {
            Value::Array(items) => items.len(),
            Value::Dict(entries) => entries.len(),
            _ => 0,
        }
    }

    /// The child at `ix`, with its key when the parent is a dictionary.
    pub fn child(&self, ix: usize) -> Option<(Option<&str>, &Node)> {
        match &self.value {
            Value::Array(items) => items.get(ix).map(|n| (None, n)),
            Value::Dict(entries) => entries.get(ix).map(|e| (Some(e.key.as_str()), &e.value)),
            _ => None,
        }
    }

    /// The value at `path` (child indices from here).
    pub fn at(&self, path: &[usize]) -> Option<&Node> {
        path.iter().try_fold(self, |node, &ix| node.child(ix).map(|(_, n)| n))
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        match &self.value {
            Value::Dict(entries) => entries.iter().find(|e| e.key == key).map(|e| &e.value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match &self.value {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// An array's strings (other items skipped).
    pub fn strings(&self) -> Vec<String> {
        match &self.value {
            Value::Array(items) => items.iter().filter_map(|n| n.as_str().map(str::to_string)).collect(),
            _ => vec![],
        }
    }
}

/// The types Xcode's editor offers ("Number" covers integers and reals).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Array,
    Dictionary,
    Boolean,
    Data,
    Date,
    Number,
    String,
}

impl Kind {
    /// In Xcode's Value Type menu order.
    pub const ALL: [Kind; 7] = [Kind::Array, Kind::Dictionary, Kind::Boolean, Kind::Data, Kind::Date, Kind::Number, Kind::String];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Array => "Array",
            Kind::Dictionary => "Dictionary",
            Kind::Boolean => "Boolean",
            Kind::Data => "Data",
            Kind::Date => "Date",
            Kind::Number => "Number",
            Kind::String => "String",
        }
    }

    pub fn is_container(self) -> bool {
        matches!(self, Kind::Array | Kind::Dictionary)
    }
}

impl Value {
    pub fn kind(&self) -> Kind {
        match self {
            Value::String(_) => Kind::String,
            Value::Integer(_) | Value::Real(_) => Kind::Number,
            Value::Bool(_) => Kind::Boolean,
            Value::Date(_) => Kind::Date,
            Value::Data(_) => Kind::Data,
            Value::Array(_) => Kind::Array,
            Value::Dict(_) => Kind::Dictionary,
        }
    }

    /// This value converted to another type, as Xcode's Value Type menu
    /// does: text is parsed where it can be, containers keep their items.
    pub fn converted(&self, kind: Kind) -> Value {
        if self.kind() == kind {
            return self.clone();
        }
        let text = match self {
            Value::String(s) => s.trim().to_string(),
            other => other.display(),
        };
        match kind {
            Kind::String => Value::String(match self {
                Value::Array(_) | Value::Dict(_) => String::new(),
                _ => text,
            }),
            Kind::Number => parse_number(&text).unwrap_or(match self {
                Value::Bool(b) => Value::Integer(*b as i128),
                _ => Value::Integer(0),
            }),
            Kind::Boolean => Value::Bool(match self {
                Value::Integer(i) => *i != 0,
                Value::Real(r) => *r != 0.,
                _ => parse_bool(&text).unwrap_or(false),
            }),
            Kind::Date => Value::Date(parse_date(&text).unwrap_or_else(now_since_2001)),
            Kind::Data => Value::Data(match self {
                Value::String(s) => parse_data(s).unwrap_or_else(|| s.as_bytes().to_vec()),
                _ => vec![],
            }),
            Kind::Array => Value::Array(match self {
                Value::Dict(entries) => entries.iter().map(|e| Node::new(e.value.value.clone())).collect(),
                _ => vec![],
            }),
            Kind::Dictionary => Value::Dict(match self {
                Value::Array(items) => items
                    .iter()
                    .enumerate()
                    .map(|(ix, n)| Entry { key: format!("Item {ix}"), key_span: 0..0, value: Node::new(n.value.clone()) })
                    .collect(),
                _ => vec![],
            }),
        }
    }

    /// The value as the editor shows it (and as typing it back parses).
    pub fn display(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            Value::Integer(i) => i.to_string(),
            Value::Real(r) => format_real(*r),
            Value::Bool(b) => if *b { "YES" } else { "NO" }.to_string(),
            Value::Date(d) => format_date(*d),
            Value::Data(bytes) => format_data(bytes),
            Value::Array(items) => items_label(items.len()),
            Value::Dict(entries) => items_label(entries.len()),
        }
    }

    /// Parses what someone typed for a value of this value's type.
    pub fn parse_as(&self, text: &str) -> Result<Value, String> {
        let trimmed = text.trim();
        match self.kind() {
            Kind::String => Ok(Value::String(text.to_string())),
            Kind::Number => parse_number(trimmed).ok_or_else(|| format!("“{trimmed}” isn’t a number.")),
            Kind::Boolean => parse_bool(trimmed).map(Value::Bool).ok_or_else(|| format!("“{trimmed}” isn’t YES or NO.")),
            Kind::Date => parse_date(trimmed).map(Value::Date).ok_or_else(|| format!("“{trimmed}” isn’t a date (YYYY-MM-DDTHH:MM:SSZ).")),
            Kind::Data => parse_data(trimmed).map(Value::Data).ok_or_else(|| format!("“{trimmed}” isn’t hexadecimal data.")),
            Kind::Array | Kind::Dictionary => Err("Containers have no value to edit.".into()),
        }
    }
}

fn items_label(count: usize) -> String {
    if count == 1 { "(1 item)".to_string() } else { format!("({count} items)") }
}

pub fn parse_number(text: &str) -> Option<Value> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(i) = text.strip_prefix('+').unwrap_or(text).parse::<i128>() {
        return Some(Value::Integer(i));
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return i128::from_str_radix(hex, 16).ok().map(Value::Integer);
    }
    match text.to_ascii_lowercase().as_str() {
        "nan" => return Some(Value::Real(f64::NAN)),
        "inf" | "+inf" | "infinity" | "+infinity" => return Some(Value::Real(f64::INFINITY)),
        "-inf" | "-infinity" => return Some(Value::Real(f64::NEG_INFINITY)),
        _ => {}
    }
    text.parse::<f64>().ok().filter(|r| r.is_finite()).map(Value::Real)
}

pub fn parse_bool(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" | "y" => Some(true),
        "no" | "false" | "0" | "n" => Some(false),
        _ => None,
    }
}

/// Hex digits, optionally in `<…>` and grouped with spaces, as Xcode shows data.
pub fn parse_data(text: &str) -> Option<Vec<u8>> {
    let inner = text.trim().trim_start_matches('<').trim_end_matches('>');
    let digits: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
    let digits = digits.strip_prefix("0x").unwrap_or(&digits);
    if digits.len() % 2 != 0 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..digits.len()).step_by(2).map(|i| u8::from_str_radix(&digits[i..i + 2], 16).ok()).collect()
}

/// `<01020304 0506>`: hex in groups of four bytes, like `NSData`'s description.
pub fn format_data(bytes: &[u8]) -> String {
    let mut out = String::from("<");
    for (ix, b) in bytes.iter().enumerate() {
        if ix > 0 && ix % 4 == 0 {
            out.push(' ');
        }
        out.push_str(&format!("{b:02x}"));
    }
    out.push('>');
    out
}

// ---------------------------------------------------------------------------
// Numbers and dates

/// `printf("%.17g")`, which CoreFoundation uses for `<real>`.
pub fn format_real(value: f64) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value.is_infinite() {
        return if value > 0. { "+infinity" } else { "-infinity" }.into();
    }
    if value == 0. {
        return if value.is_sign_negative() { "-0" } else { "0" }.into();
    }
    const PRECISION: i32 = 17;
    let scientific = format!("{:.*e}", (PRECISION - 1) as usize, value);
    let (mantissa, exponent) = scientific.split_once('e').expect("formatted with e");
    let exponent: i32 = exponent.parse().expect("integer exponent");
    let trim = |s: &str| -> String {
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
    };
    if exponent < -4 || exponent >= PRECISION {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim(mantissa), exponent.abs())
    } else {
        let decimals = (PRECISION - 1 - exponent).max(0) as usize;
        trim(&format!("{value:.decimals$}"))
    }
}

/// Seconds between 1970-01-01 and 2001-01-01.
const UNIX_TO_2001: f64 = 978_307_200.;

pub fn now_since_2001() -> f64 {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as f64)
        .unwrap_or(UNIX_TO_2001);
    unix - UNIX_TO_2001
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if month <= 2 { 1 } else { 0 }, month, day)
}

/// The days in a month of the proleptic Gregorian calendar.
fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `2024-01-02T03:04:05Z`, as `<date>` holds it (whole seconds, UTC).
pub fn format_date(since_2001: f64) -> String {
    let unix = (since_2001 + UNIX_TO_2001).floor() as i64;
    let (days, secs) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

/// Parses `YYYY-MM-DDTHH:MM:SSZ` (also with a space for `T`, without the
/// `Z`, or just a date).
pub fn parse_date(text: &str) -> Option<f64> {
    let text = text.trim().trim_end_matches('Z');
    let (date, time) = text.split_once(['T', ' ']).unwrap_or((text, "00:00:00"));
    let mut d = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.trim().splitn(3, ':').map(|p| p.parse::<i64>().ok());
    let (hh, mm) = (t.next()??, t.next().flatten().unwrap_or(0));
    let ss = t.next().flatten().unwrap_or(0);
    if !(1..=12).contains(&m) || !(1..=days_in_month(y, m)).contains(&day) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let unix = days_from_civil(y, m, day) * 86_400 + hh * 3600 + mm * 60 + ss;
    Some(unix as f64 - UNIX_TO_2001)
}

// ---------------------------------------------------------------------------
// Base64

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(BASE64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for b in text.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            c if c.is_ascii_whitespace() => continue,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// XML

pub const XML_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n";

/// Escapes text as CoreFoundation does: only `&`, `<` and `>`.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// An element's text: entities decoded, CDATA sections kept verbatim.
fn element_text(text: &str, element: &Element) -> String {
    if element.self_closing {
        return String::new();
    }
    let mut raw = &text[element.open_end..element.close_start];
    let mut out = String::new();
    while let Some(start) = raw.find("<![CDATA[") {
        out.push_str(&xml::unescape(&raw[..start]));
        let rest = &raw[start + 9..];
        let end = rest.find("]]>").unwrap_or(rest.len());
        out.push_str(&rest[..end]);
        raw = rest.get(end + 3..).unwrap_or("");
    }
    out.push_str(&xml::unescape(raw));
    out
}

fn node_from(text: &str, element: &Element) -> Result<Node> {
    let content = || element_text(text, element);
    let value = match element.name.as_str() {
        "string" => Value::String(content()),
        "integer" => {
            let s = content();
            match parse_number(&s) {
                Some(Value::Integer(i)) => Value::Integer(i),
                _ => bail!("“{}” isn’t an integer (byte {})", s.trim(), element.span.start),
            }
        }
        "real" => match parse_number(&content()) {
            Some(Value::Integer(i)) => Value::Real(i as f64),
            Some(Value::Real(r)) => Value::Real(r),
            _ => bail!("bad <real> at byte {}", element.span.start),
        },
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "date" => Value::Date(parse_date(&content()).ok_or_else(|| anyhow!("bad <date> at byte {}", element.span.start))?),
        "data" => Value::Data(base64_decode(&content()).ok_or_else(|| anyhow!("bad <data> at byte {}", element.span.start))?),
        "array" => Value::Array(element.children.iter().map(|c| node_from(text, c)).collect::<Result<_>>()?),
        "dict" => {
            let mut entries = vec![];
            let mut children = element.children.iter();
            while let Some(key) = children.next() {
                if key.name != "key" {
                    bail!("expected <key> in <dict>, found <{}> at byte {}", key.name, key.span.start);
                }
                let value = children.next().ok_or_else(|| anyhow!("<key> without a value at byte {}", key.span.start))?;
                entries.push(Entry { key: element_text(text, key), key_span: key.span.clone(), value: node_from(text, value)? });
            }
            Value::Dict(entries)
        }
        other => bail!("<{other}> isn’t a property list element (byte {})", element.span.start),
    };
    Ok(Node { value, span: element.span.clone() })
}

/// Parses an XML property list, keeping byte ranges.
pub fn parse_xml(text: &str) -> Result<Node> {
    let root = xml::parse(text)?;
    if root.name != "plist" {
        return node_from(text, &root);
    }
    match root.children.as_slice() {
        [value] => node_from(text, value),
        [] => bail!("the property list is empty"),
        _ => bail!("<plist> has more than one value"),
    }
}

fn indent(out: &mut String, depth: usize, unit: &str) {
    for _ in 0..depth {
        out.push_str(unit);
    }
}

/// Writes a value's element at `depth` (tabs), as CoreFoundation does.
/// The first line isn't indented; nested lines and the closing tag are.
pub fn write_value(out: &mut String, value: &Value, depth: usize) {
    write_value_in(out, value, depth, "\t");
}

/// `write_value` with another indent than a tab (for files indented with
/// spaces).
pub fn write_value_in(out: &mut String, value: &Value, depth: usize, unit: &str) {
    match value {
        Value::String(s) => {
            out.push_str("<string>");
            out.push_str(&escape(s));
            out.push_str("</string>");
        }
        Value::Integer(i) => out.push_str(&format!("<integer>{i}</integer>")),
        Value::Real(r) => out.push_str(&format!("<real>{}</real>", format_real(*r))),
        Value::Bool(true) => out.push_str("<true/>"),
        Value::Bool(false) => out.push_str("<false/>"),
        Value::Date(d) => out.push_str(&format!("<date>{}</date>", format_date(*d))),
        Value::Data(bytes) => {
            out.push_str("<data>\n");
            // CoreFoundation's lines get shorter as they're indented, down to
            // a limit of eight tabs.
            let tabs = depth.min(8);
            let width = 76 - tabs * 8;
            let encoded = base64_encode(bytes);
            for line in encoded.as_bytes().chunks(width) {
                indent(out, tabs, unit);
                out.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
                out.push('\n');
            }
            indent(out, depth, unit);
            out.push_str("</data>");
        }
        Value::Array(items) if items.is_empty() => out.push_str("<array/>"),
        Value::Dict(entries) if entries.is_empty() => out.push_str("<dict/>"),
        Value::Array(items) => {
            out.push_str("<array>\n");
            for item in items {
                indent(out, depth + 1, unit);
                write_value_in(out, &item.value, depth + 1, unit);
                out.push('\n');
            }
            indent(out, depth, unit);
            out.push_str("</array>");
        }
        Value::Dict(entries) => {
            out.push_str("<dict>\n");
            for entry in entries {
                indent(out, depth + 1, unit);
                out.push_str("<key>");
                out.push_str(&escape(&entry.key));
                out.push_str("</key>\n");
                indent(out, depth + 1, unit);
                write_value_in(out, &entry.value.value, depth + 1, unit);
                out.push('\n');
            }
            indent(out, depth, unit);
            out.push_str("</dict>");
        }
    }
}

/// A whole XML property list, byte for byte as Xcode writes it.
pub fn to_xml(value: &Value) -> String {
    let mut out = String::from(XML_HEADER);
    write_value(&mut out, value, 0);
    out.push_str("\n</plist>\n");
    out
}

// ---------------------------------------------------------------------------
// Files

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Xml,
    Binary,
}

/// Files zcode opens in the property list editor.
pub fn is_property_list(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e, "plist" | "entitlements" | "xcprivacy" | "stringsdict"))
}

pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.starts_with(b"bplist00")
}

/// A property list's text for editing: XML as it is, binary converted to
/// XML in Xcode's layout.
pub fn load(bytes: Vec<u8>) -> Result<(Format, String)> {
    if is_binary(&bytes) {
        let value = binary::read(&bytes).context("couldn’t read the binary property list")?;
        return Ok((Format::Binary, to_xml(&value)));
    }
    String::from_utf8(bytes).map(|text| (Format::Xml, text)).map_err(|_| anyhow!("not a UTF-8 text file"))
}

/// The bytes to write for edited text in `format`.
pub fn encode(text: &str, format: Format) -> Result<Vec<u8>> {
    match format {
        Format::Xml => Ok(text.as_bytes().to_vec()),
        Format::Binary => {
            let root = parse_xml(text).context("the property list has errors; fix them in the source code view")?;
            Ok(binary::write(&root.value))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>a</key>\n\t<dict/>\n\t<key>b</key>\n\t<array/>\n\t<key>big</key>\n\t<integer>18446744073709551615</integer>\n\t<key>data</key>\n\t<data>\n\tAAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEy\n\tMzQ1Njc4OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYGFiYw==\n\t</data>\n\t<key>date</key>\n\t<date>2024-01-02T03:04:05Z</date>\n\t<key>edata</key>\n\t<data>\n\t</data>\n\t<key>empty</key>\n\t<string></string>\n\t<key>f</key>\n\t<false/>\n\t<key>i</key>\n\t<integer>-5</integer>\n\t<key>nest</key>\n\t<dict>\n\t\t<key>arr</key>\n\t\t<array>\n\t\t\t<integer>1</integer>\n\t\t\t<string>x</string>\n\t\t\t<dict>\n\t\t\t\t<key>k</key>\n\t\t\t\t<array/>\n\t\t\t</dict>\n\t\t</array>\n\t</dict>\n\t<key>r1</key>\n\t<real>1</real>\n\t<key>r2</key>\n\t<real>0.10000000000000001</real>\n\t<key>r3</key>\n\t<real>1e+20</real>\n\t<key>r4</key>\n\t<real>3.1415899999999999</real>\n\t<key>r5</key>\n\t<real>-2.5000000000000002e-10</real>\n\t<key>s</key>\n\t<string>a&amp;b&lt;c&gt;d\"e'f\ttab\nnl é 😀</string>\n\t<key>t</key>\n\t<true/>\n</dict>\n</plist>\n";

    #[test]
    fn round_trips_core_foundation_output_byte_for_byte() {
        // `SAMPLE` is `plutil -convert xml1` output.
        let root = parse_xml(SAMPLE).unwrap();
        assert_eq!(to_xml(&root.value), SAMPLE);
        assert_eq!(root.get("big").unwrap().value, Value::Integer(18446744073709551615));
        assert_eq!(root.get("data").unwrap().value, Value::Data((0..100).collect()));
        assert_eq!(root.get("s").unwrap().as_str(), Some("a&b<c>d\"e'f\ttab\nnl é 😀"));
        assert_eq!(root.get("r2").unwrap().value, Value::Real(0.1));
        assert_eq!(format_date(match root.get("date").unwrap().value { Value::Date(d) => d, _ => 0. }), "2024-01-02T03:04:05Z");
    }

    #[test]
    fn keeps_spans() {
        let root = parse_xml(SAMPLE).unwrap();
        let Value::Dict(entries) = &root.value else { panic!() };
        assert_eq!(&SAMPLE[entries[0].key_span.clone()], "<key>a</key>");
        assert_eq!(&SAMPLE[entries[0].value.span.clone()], "<dict/>");
        let nest = root.get("nest").unwrap();
        assert!(SAMPLE[nest.span.clone()].starts_with("<dict>\n\t\t<key>arr</key>"));
        assert!(SAMPLE[nest.span.clone()].ends_with("\t</dict>"));
    }

    #[test]
    fn data_lines_shrink_with_depth() {
        let mut value = Value::Data(vec![0; 200]);
        for key in ["u", "t", "s", "r", "q", "p", "o", "m", "n"] {
            value = Value::Dict(vec![Entry { key: key.into(), key_span: 0..0, value: Node::new(value) }]);
        }
        let xml = to_xml(&value);
        assert!(xml.contains("\n\t\t\t\t\t\t\t\t\t<data>\n\t\t\t\t\t\t\t\tAAAAAAAAAAAA\n"), "{xml}");
        let root = to_xml(&Value::Data(vec![0; 200]));
        assert!(root.lines().any(|l| l.len() == 76));
    }

    #[test]
    fn formats_reals_like_printf_g17() {
        for (value, text) in [(1.0, "1"), (0.1, "0.10000000000000001"), (1e20, "1e+20"), (1.5, "1.5"), (-2.5e-10, "-2.5000000000000002e-10"), (123456.0, "123456"), (1e-5, "1.0000000000000001e-05"), (0.0001, "0.0001")] {
            assert_eq!(format_real(value), text, "{value}");
        }
    }

    #[test]
    fn dates_round_trip() {
        for text in ["2001-01-01T00:00:00Z", "1969-07-20T20:17:40Z", "2024-02-29T23:59:59Z", "2100-03-01T12:00:00Z"] {
            assert_eq!(format_date(parse_date(text).unwrap()), text);
        }
        assert_eq!(parse_date("2001-01-01T00:00:00Z"), Some(0.));
        assert_eq!(format_date(parse_date("2024-05-06 07:08").unwrap()), "2024-05-06T07:08:00Z");
        assert!(parse_date("yesterday").is_none());
        // Days a month doesn't have are refused, not rolled into the next.
        for text in ["2026-02-30T00:00:00Z", "2026-02-29", "2100-02-29", "2026-04-31", "2026-13-01", "2026-00-10"] {
            assert!(parse_date(text).is_none(), "{text}");
        }
        assert!(parse_date("2000-02-29").is_some() && parse_date("2026-12-31").is_some());
    }

    #[test]
    fn base64_round_trips() {
        for len in 0..10 {
            let bytes: Vec<u8> = (0..len).map(|i: u8| i.wrapping_mul(37)).collect();
            assert_eq!(base64_decode(&base64_encode(&bytes)).unwrap(), bytes);
        }
        assert_eq!(base64_decode("AQI=\n\t").unwrap(), vec![1, 2]);
    }

    #[test]
    fn reads_cdata_entities_and_comments() {
        let text = "<plist><dict><!-- c --><key>a&#x41;</key><string><![CDATA[<b>]]>&amp;</string></dict></plist>";
        let root = parse_xml(text).unwrap();
        assert_eq!(root.get("aA").unwrap().as_str(), Some("<b>&"));
        assert!(parse_xml("<plist><dict><key>a</key></dict></plist>").is_err());
        assert!(parse_xml("<plist><dict><string>a</string></dict></plist>").is_err());
    }

    #[test]
    fn converts_types_as_xcode_does() {
        assert_eq!(Value::String("12".into()).converted(Kind::Number), Value::Integer(12));
        assert_eq!(Value::String("YES".into()).converted(Kind::Boolean), Value::Bool(true));
        assert_eq!(Value::Integer(0).converted(Kind::Boolean), Value::Bool(false));
        assert_eq!(Value::Bool(true).converted(Kind::String), Value::String("YES".into()));
        let array = Value::Array(vec![Node::new(Value::String("a".into()))]);
        let dict = array.converted(Kind::Dictionary);
        assert_eq!(dict.display(), "(1 item)");
        assert_eq!(dict.converted(Kind::Array), array);
        assert_eq!(Value::Data(vec![1, 2, 3, 4, 5]).display(), "<01020304 05>");
        assert_eq!(Value::Data(vec![]).parse_as("<0102 0a>").unwrap(), Value::Data(vec![1, 2, 10]));
        assert!(Value::Integer(0).parse_as("x").is_err());
    }

    #[test]
    fn loads_and_encodes_binary() {
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>z</key>\n\t<string>last</string>\n\t<key>a</key>\n\t<true/>\n</dict>\n</plist>\n";
        let bytes = encode(xml, Format::Binary).unwrap();
        assert!(is_binary(&bytes));
        let (format, text) = load(bytes).unwrap();
        assert_eq!(format, Format::Binary);
        // Key order survives.
        assert_eq!(text, xml);
    }
}

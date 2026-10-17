//! CSS abbreviations: `m10-20`, `w100p`, `d:f`, `dib`, `c#fc0`, `bd+`,
//! `m10+p5`, `lh1.5!` and the `property:value` fallback.

mod table;

use std::collections::HashMap;
use std::sync::OnceLock;

use self::table::{GLOBAL_KEYWORDS, PROPERTIES, UNITLESS};
use crate::Options;
use crate::error::Error;
use crate::snippet::{SnippetBuf, parse_template};

pub(crate) fn expand(abbreviation: &str, options: &Options) -> Result<String, Error> {
    let parts = split(abbreviation)?;
    let mut buf = SnippetBuf::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            buf.text(&options.newline);
        }
        resolve(part)?.write(&mut buf, i as u32);
    }
    Ok(buf.finish())
}

/// Whether every part of `abbreviation` is a known property abbreviation.
pub(crate) fn is_known(abbreviation: &str) -> bool {
    split(abbreviation).is_ok_and(|parts| {
        parts
            .iter()
            .all(|part| resolve(part).is_ok_and(|d| d.known))
    })
}

/// The abbreviation at the end of the line: a run of abbreviation
/// characters that starts a declaration (only whitespace between it and the
/// last `{`, `;`, `}` or the start of the line) and starts with a letter.
/// That keeps out values (`color: re`), selectors (`.card`, `div p`),
/// at-rules and preprocessor variables (`@media`, `$gap`).
pub(crate) fn extract(line: &str) -> Option<(usize, &str)> {
    let start = line
        .char_indices()
        .rev()
        .take_while(|&(_, c)| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | ':' | '!' | '#' | '.' | '%' | '+')
        })
        .last()
        .map(|(i, _)| i)?;
    let abbreviation = &line[start..];
    let prefix = &line[..start];
    let declaration_start = prefix.rfind(['{', ';', '}']).map_or(0, |i| i + 1);
    if !prefix[declaration_start..].trim().is_empty()
        || !abbreviation.starts_with(|c: char| c.is_ascii_alphabetic())
    {
        return None;
    }
    Some((start, abbreviation))
}

/// Splits `m10+p5` into its parts. A `+` not followed by a letter is part of
/// the name (`bd+`).
fn split(abbreviation: &str) -> Result<Vec<&str>, Error> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (i, c) in abbreviation.char_indices() {
        if c == '+'
            && i > start
            && abbreviation[i + 1..].starts_with(|c: char| c.is_ascii_alphabetic())
        {
            parts.push(&abbreviation[start..i]);
            start = i + 1;
        }
    }
    parts.push(&abbreviation[start..]);
    if parts.iter().any(|part| part.is_empty()) {
        return Err(Error::UnknownAbbreviation(abbreviation.to_owned()));
    }
    Ok(parts)
}

struct Entry {
    property: &'static str,
    /// What the property gets without a value: a keyword or a template.
    default: Option<&'static str>,
    keywords: Vec<&'static str>,
}

struct Table {
    by_key: HashMap<&'static str, Entry>,
    /// Full property names (`color:red`), to the first key that has them.
    by_property: HashMap<&'static str, &'static str>,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut by_key = HashMap::new();
        let mut by_property = HashMap::new();
        for &(key, definition) in PROPERTIES {
            let (property, rest) = definition.split_once(':').unwrap_or((definition, ""));
            let items: Vec<&str> = rest.split('|').filter(|item| !item.is_empty()).collect();
            let entry = Entry {
                property,
                default: items.first().copied(),
                keywords: items
                    .into_iter()
                    .filter(|item| !item.contains(['$', '(', ' ']))
                    .collect(),
            };
            by_property.entry(property).or_insert(key);
            by_key.insert(key, entry);
        }
        Table {
            by_key,
            by_property,
        }
    })
}

fn lookup(name: &str) -> Option<&'static Entry> {
    let table = table();
    table.by_key.get(name).or_else(|| {
        table
            .by_property
            .get(name)
            .and_then(|key| table.by_key.get(key))
    })
}

enum Value {
    /// A template with fields (`${1:1px} ${2:solid} ${3:#000}`).
    Template(&'static str),
    /// A keyword as the placeholder of the value's tabstop.
    Placeholder(&'static str),
    /// An empty tabstop.
    Empty,
    Literal(String),
}

struct Declaration {
    property: String,
    value: Value,
    important: bool,
    /// Resolved through the table rather than the `property:value` fallback.
    known: bool,
}

impl Declaration {
    fn new(entry: &Entry, values: Vec<String>, important: bool) -> Self {
        let value = if !values.is_empty() {
            Value::Literal(values.join(" "))
        } else {
            match entry.default {
                Some(default) if default.contains("${") => Value::Template(default),
                Some(default) => Value::Placeholder(default),
                None => Value::Empty,
            }
        };
        Self {
            property: entry.property.to_owned(),
            value,
            important,
            known: true,
        }
    }

    /// `property: value;`, the fields in `group`.
    fn write(&self, buf: &mut SnippetBuf, group: u32) {
        buf.text(&self.property);
        buf.text(": ");
        match &self.value {
            Value::Template(template) => buf.pieces(&parse_template(template, group)),
            Value::Placeholder(keyword) => buf.field((group, 1), keyword),
            Value::Empty => buf.tabstop(""),
            Value::Literal(value) => buf.text(value),
        }
        if self.important {
            buf.text(" !important");
        }
        buf.text(";");
    }
}

fn resolve(part: &str) -> Result<Declaration, Error> {
    let unknown = || Error::UnknownAbbreviation(part.to_owned());
    let (body, important) = match part.strip_suffix('!') {
        Some(body) => (body, true),
        None => (part, false),
    };
    if body.is_empty() {
        return Err(unknown());
    }
    if let Some(entry) = table().by_key.get(body) {
        // `bd:n` and `bg:n` spell their value out.
        let values = match entry.default {
            Some(value) if body.contains(':') && !value.contains("${") => vec![value.to_owned()],
            _ => Vec::new(),
        };
        return Ok(Declaration::new(entry, values, important));
    }
    if let Some((name, after)) = body.split_once(':') {
        if let Some(entry) = lookup(name) {
            return Ok(Declaration::new(
                entry,
                colon_value(entry, after)?,
                important,
            ));
        }
        let valid_name =
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if !valid_name || after.is_empty() {
            return Err(unknown());
        }
        return Ok(Declaration {
            property: name.to_owned(),
            value: Value::Literal(after.to_owned()),
            important,
            known: false,
        });
    }
    let name_len = body.bytes().take_while(u8::is_ascii_alphabetic).count();
    let (name, rest) = body.split_at(name_len);
    if name.is_empty() {
        return Err(unknown());
    }
    if let Some(entry) = lookup(name) {
        return Ok(Declaration::new(entry, values(entry, rest)?, important));
    }
    // `dib`, `posa`, `tdn`: the longest key the name starts with, and a
    // keyword of its property for the rest.
    for split in (1..name.len()).rev() {
        if let Some(entry) = table().by_key.get(&name[..split])
            && let Some(keyword) = match_keyword(&name[split..], &entry.keywords)
        {
            let mut all = vec![keyword.to_owned()];
            all.extend(values(entry, rest)?);
            return Ok(Declaration::new(entry, all, important));
        }
    }
    Err(unknown())
}

/// The value after `name:`: a keyword abbreviation (`d:ib`) or values
/// (`m:10-20`).
fn colon_value(entry: &Entry, after: &str) -> Result<Vec<String>, Error> {
    if after.starts_with(|c: char| c.is_ascii_alphabetic()) {
        let keyword =
            keyword_or_global(after, entry).map_or_else(|| after.to_owned(), str::to_owned);
        return Ok(vec![keyword]);
    }
    values(entry, after)
}

fn keyword_or_global(abbreviation: &str, entry: &Entry) -> Option<&'static str> {
    match_keyword(abbreviation, &entry.keywords)
        .or_else(|| match_keyword(abbreviation, GLOBAL_KEYWORDS))
}

/// Numbers with units, colors and keywords separated by `-` (`10-20`,
/// `10--20` for a negative second value, `-10`, `0-a`, `#fc0`).
fn values(entry: &Entry, s: &str) -> Result<Vec<String>, Error> {
    let invalid = || Error::UnknownAbbreviation(s.to_owned());
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut after_value = false;
    while i < bytes.len() {
        let b = bytes[i];
        if after_value && b == b'-' {
            i += 1;
            after_value = false;
            if i == bytes.len() {
                return Err(invalid());
            }
            continue;
        }
        if b == b'#' {
            let digits = bytes[i + 1..]
                .iter()
                .take_while(|b| b.is_ascii_hexdigit())
                .count();
            if digits == 0 {
                return Err(invalid());
            }
            out.push(color(&s[i + 1..i + 1 + digits]));
            i += 1 + digits;
        } else if b.is_ascii_digit() || b == b'.' || b == b'-' {
            let start = i;
            if b == b'-' {
                i += 1;
            }
            let int = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
            i += int;
            let mut frac = 0;
            if bytes.get(i) == Some(&b'.') {
                frac = bytes[i + 1..]
                    .iter()
                    .take_while(|b| b.is_ascii_digit())
                    .count();
                i += 1 + frac;
            }
            if int + frac == 0 {
                return Err(invalid());
            }
            let number = &s[start..i];
            let unit_len = bytes[i..]
                .iter()
                .take_while(|b| b.is_ascii_alphabetic() || **b == b'%')
                .count();
            let unit = &s[i..i + unit_len];
            i += unit_len;
            out.push(format_number(number, unit, entry.property));
        } else if b.is_ascii_alphabetic() {
            let len = bytes[i..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric())
                .count();
            let word = &s[i..i + len];
            out.push(keyword_or_global(word, entry).map_or_else(|| word.to_owned(), str::to_owned));
            i += len;
        } else {
            return Err(invalid());
        }
        after_value = true;
    }
    Ok(out)
}

/// A number with its unit: `p` is `%`, `e` `em`, `x` `ex`, `r` `rem`; a bare
/// integer is `px` and a bare decimal `em`, except for zero and unitless
/// properties.
fn format_number(number: &str, unit: &str, property: &str) -> String {
    let unit = match unit {
        "p" => "%",
        "e" => "em",
        "x" => "ex",
        "r" => "rem",
        unit => unit,
    };
    let zero = number.bytes().all(|b| matches!(b, b'0' | b'.' | b'-'));
    let unit = if !unit.is_empty() {
        unit
    } else if zero || UNITLESS.contains(&property) {
        ""
    } else if number.contains('.') {
        "em"
    } else {
        "px"
    };
    format!("{number}{unit}")
}

/// `#f` is `#fff`, `#fc` `#fcfcfc`, `#ffcc00` `#fc0`.
fn color(hex: &str) -> String {
    let hex = hex.to_ascii_lowercase();
    let b = hex.as_bytes();
    let hex = match b.len() {
        1 | 2 => hex.repeat(3),
        6 if b[0] == b[1] && b[2] == b[3] && b[4] == b[5] => {
            [b[0] as char, b[2] as char, b[4] as char].iter().collect()
        }
        _ => hex,
    };
    format!("#{hex}")
}

/// The keyword `abbreviation` stands for: the exact keyword, then the one
/// whose word initials it is (`ib` → `inline-block`), then one it starts
/// (`a` → `absolute`), then, for short abbreviations, one with its letters
/// in order (`nw` → `nowrap`). Ties go to the earlier keyword.
fn match_keyword(abbreviation: &str, keywords: &[&'static str]) -> Option<&'static str> {
    let abbreviation = abbreviation.to_ascii_lowercase();
    let first = abbreviation.chars().next()?;
    let mut best: Option<(u8, &'static str)> = None;
    for &keyword in keywords {
        let score = if keyword == abbreviation {
            4
        } else if initials(keyword) == abbreviation {
            3
        } else if keyword.starts_with(&abbreviation) {
            2
        } else if abbreviation.len() <= 4
            && keyword.starts_with(first)
            && is_subsequence(&abbreviation, keyword)
        {
            1
        } else {
            0
        };
        if score > best.map_or(0, |(s, _)| s) {
            best = Some((score, keyword));
        }
    }
    best.map(|(_, keyword)| keyword)
}

fn initials(keyword: &str) -> String {
    keyword
        .split('-')
        .filter_map(|word| word.chars().next())
        .collect()
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut haystack = haystack.chars();
    needle.chars().all(|c| haystack.any(|h| h == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords() {
        let display = &lookup("d").unwrap().keywords;
        assert_eq!(match_keyword("f", display), Some("flex"));
        assert_eq!(match_keyword("ib", display), Some("inline-block"));
        assert_eq!(match_keyword("if", display), Some("inline-flex"));
        assert_eq!(match_keyword("i", display), Some("inline"));
        assert_eq!(match_keyword("n", display), Some("none"));
        assert_eq!(match_keyword("zz", display), None);
        assert_eq!(
            match_keyword("nw", &lookup("ws").unwrap().keywords),
            Some("nowrap")
        );
    }

    #[test]
    fn colors() {
        assert_eq!(color("f"), "#fff");
        assert_eq!(color("fc"), "#fcfcfc");
        assert_eq!(color("FC0"), "#fc0");
        assert_eq!(color("ffcc00"), "#fc0");
        assert_eq!(color("ffcc01"), "#ffcc01");
    }

    #[test]
    fn every_entry_parses() {
        for (key, _) in PROPERTIES {
            let entry = lookup(key).unwrap();
            assert!(!entry.property.is_empty(), "{key}");
            assert!(is_known(key), "{key}");
            let expanded = expand(key, &Options::default()).unwrap();
            assert!(
                expanded.starts_with(entry.property) && expanded.ends_with(';'),
                "{key}"
            );
            for keyword in &entry.keywords {
                assert!(!keyword.is_empty() && !keyword.contains(':'), "{key}");
            }
        }
    }
}

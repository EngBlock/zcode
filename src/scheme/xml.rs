//! Just enough XML for `.xcscheme` files: a parse that keeps every
//! element's and attribute's byte range, and in-place edits that rewrite
//! only what changes, in the layout Xcode writes (three-space indents, one
//! `key = "value"` attribute per line, explicit closing tags).

use std::ops::Range;

use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub name: String,
    /// Unescaped.
    pub value: String,
    /// From the name to the closing quote.
    pub span: Range<usize>,
    /// Inside the quotes.
    pub value_span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub name: String,
    /// From `<` to the end of the closing tag.
    pub span: Range<usize>,
    /// Just past the start tag's `>`.
    pub open_end: usize,
    /// Where the closing tag starts (`open_end` when self-closing).
    pub close_start: usize,
    pub self_closing: bool,
    pub attributes: Vec<Attribute>,
    pub children: Vec<Element>,
}

impl Element {
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|a| a.name == name).map(|a| a.value.as_str())
    }

    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |c| c.name == name)
    }
}

pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&apos;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
    out
}

pub fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';') else { break };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "quot" => Some('"'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parses the document's root element.
pub fn parse(text: &str) -> Result<Element> {
    let mut parser = Parser { text, pos: 0 };
    parser.skip_misc()?;
    let root = parser.element()?;
    Ok(root)
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.text[self.pos..]
    }

    fn skip_whitespace(&mut self) {
        let skipped = self.rest().len() - self.rest().trim_start().len();
        self.pos += skipped;
    }

    fn skip_past(&mut self, end: &str) -> Result<()> {
        match self.rest().find(end) {
            Some(i) => {
                self.pos += i + end.len();
                Ok(())
            }
            None => bail!("unterminated markup at byte {}", self.pos),
        }
    }

    /// Skips text, comments, declarations and processing instructions up to
    /// the next tag.
    fn skip_misc(&mut self) -> Result<()> {
        loop {
            match self.rest().find('<') {
                None => {
                    self.pos = self.text.len();
                    return Ok(());
                }
                Some(i) => self.pos += i,
            }
            let rest = self.rest();
            if rest.starts_with("<!--") {
                self.skip_past("-->")?;
            } else if rest.starts_with("<?") {
                self.skip_past("?>")?;
            } else if rest.starts_with("<![CDATA[") {
                self.skip_past("]]>")?;
            } else if rest.starts_with("<!") {
                self.skip_past(">")?;
            } else {
                return Ok(());
            }
        }
    }

    fn name(&mut self) -> String {
        let len = self
            .rest()
            .find(|c: char| c.is_whitespace() || matches!(c, '>' | '/' | '='))
            .unwrap_or(self.rest().len());
        let name = self.rest()[..len].to_string();
        self.pos += len;
        name
    }

    fn element(&mut self) -> Result<Element> {
        let start = self.pos;
        if !self.rest().starts_with('<') || self.rest().starts_with("</") {
            bail!("expected an element at byte {start}");
        }
        self.pos += 1;
        let name = self.name();
        if name.is_empty() {
            bail!("missing element name at byte {start}");
        }
        let mut attributes = vec![];
        loop {
            self.skip_whitespace();
            let rest = self.rest();
            if rest.starts_with("/>") {
                self.pos += 2;
                return Ok(Element {
                    name,
                    span: start..self.pos,
                    open_end: self.pos,
                    close_start: self.pos,
                    self_closing: true,
                    attributes,
                    children: vec![],
                });
            }
            if rest.starts_with('>') {
                self.pos += 1;
                break;
            }
            if rest.is_empty() {
                bail!("unterminated <{name}> at byte {start}");
            }
            let attr_start = self.pos;
            let attr_name = self.name();
            if attr_name.is_empty() {
                bail!("bad attribute in <{name}> at byte {attr_start}");
            }
            self.skip_whitespace();
            if !self.rest().starts_with('=') {
                bail!("attribute {attr_name} has no value at byte {}", self.pos);
            }
            self.pos += 1;
            self.skip_whitespace();
            let quote = self.rest().chars().next().filter(|c| matches!(c, '"' | '\''));
            let Some(quote) = quote else { bail!("unquoted value for {attr_name} at byte {}", self.pos) };
            self.pos += 1;
            let value_start = self.pos;
            let Some(len) = self.rest().find(quote) else { bail!("unterminated value for {attr_name}") };
            let value_span = value_start..value_start + len;
            self.pos = value_span.end + 1;
            attributes.push(Attribute {
                name: attr_name,
                value: unescape(&self.text[value_span.clone()]),
                span: attr_start..self.pos,
                value_span,
            });
        }
        let open_end = self.pos;
        let mut children = vec![];
        loop {
            self.skip_misc()?;
            if self.rest().is_empty() {
                bail!("<{name}> at byte {start} is never closed");
            }
            if self.rest().starts_with("</") {
                let close_start = self.pos;
                self.pos += 2;
                let closing = self.name();
                if closing != name {
                    bail!("</{closing}> closes <{name}> at byte {close_start}");
                }
                self.skip_past(">")?;
                return Ok(Element {
                    name,
                    span: start..self.pos,
                    open_end,
                    close_start,
                    self_closing: false,
                    attributes,
                    children,
                });
            }
            children.push(self.element()?);
        }
    }
}

/// The whitespace before `offset` on its line, if only whitespace precedes it.
fn indent_at(text: &str, offset: usize) -> Option<&str> {
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let indent = &text[line_start..offset];
    indent.chars().all(|c| c == ' ' || c == '\t').then_some(indent)
}

/// One step deeper than `indent`, as Xcode indents.
pub fn deeper(indent: &str) -> String {
    format!("{indent}   ")
}

/// Byte-range replacements against one text, applied together.
#[derive(Default)]
pub struct Edits {
    edits: Vec<(Range<usize>, String)>,
}

impl Edits {
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub fn replace(&mut self, range: Range<usize>, text: String) {
        self.edits.push((range, text));
    }

    /// Applies every edit; they must not overlap.
    pub fn apply(mut self, text: &str) -> Result<String> {
        self.edits.sort_by_key(|(range, _)| (range.start, range.end));
        for pair in self.edits.windows(2) {
            if pair[0].0.end > pair[1].0.start {
                bail!("overlapping scheme edits");
            }
        }
        let mut out = String::with_capacity(text.len() + 256);
        let mut at = 0;
        for (range, replacement) in &self.edits {
            out.push_str(&text[at..range.start]);
            out.push_str(replacement);
            at = range.end;
        }
        out.push_str(&text[at..]);
        Ok(out)
    }

    /// Sets an attribute's value, adding it after the element's last
    /// attribute (on its own line, as Xcode does) when missing.
    pub fn set_attribute(&mut self, text: &str, element: &Element, name: &str, value: &str) {
        if let Some(attr) = element.attributes.iter().find(|a| a.name == name) {
            if attr.value != value {
                self.replace(attr.value_span.clone(), escape(value));
            }
            return;
        }
        let indent = match element.attributes.first() {
            Some(first) => indent_at(text, first.span.start).map(str::to_string),
            None => None,
        }
        .unwrap_or_else(|| deeper(indent_at(text, element.span.start).unwrap_or("")));
        let at = element
            .attributes
            .last()
            .map(|a| a.span.end)
            .unwrap_or(element.span.start + 1 + element.name.len());
        self.replace(at..at, format!("\n{indent}{name} = \"{}\"", escape(value)));
    }

    pub fn remove_attribute(&mut self, text: &str, element: &Element, name: &str) {
        if let Some(attr) = element.attributes.iter().find(|a| a.name == name) {
            let start = text[..attr.span.start].trim_end().len();
            self.replace(start..attr.span.end, String::new());
        }
    }

    /// Replaces `old` (a child of some element) with `block`, or removes it
    /// with its line when `block` is `None`. `block` is the element's text
    /// as `block_text` makes it.
    pub fn replace_element(&mut self, text: &str, old: &Element, block: Option<&dyn Fn(&str) -> String>) {
        let indent = indent_at(text, old.span.start);
        match (block, indent) {
            (Some(block), indent) => {
                let replacement = block(indent.unwrap_or(""));
                self.replace(old.span.clone(), replacement);
            }
            (None, Some(indent)) => {
                // Remove the whole line, with the newline before it.
                let start = old.span.start - indent.len();
                let start = if text[..start].ends_with('\n') { start - 1 } else { start };
                self.replace(start..old.span.end, String::new());
            }
            (None, None) => self.replace(old.span.clone(), String::new()),
        }
    }

    /// Inserts a new child into `parent` after `after` (one of its children),
    /// or as its first child.
    pub fn insert_child(&mut self, text: &str, parent: &Element, after: Option<&Element>, block: &dyn Fn(&str) -> String) {
        let parent_indent = indent_at(text, parent.span.start).unwrap_or("").to_string();
        let indent = deeper(&parent_indent);
        if parent.self_closing {
            let end = parent.span.end;
            self.replace(
                end - 2..end,
                format!(">\n{indent}{}\n{parent_indent}</{}>", block(&indent), parent.name),
            );
            return;
        }
        match after {
            Some(sibling) => {
                let indent = indent_at(text, sibling.span.start).map(str::to_string).unwrap_or(indent);
                self.replace(sibling.span.end..sibling.span.end, format!("\n{indent}{}", block(&indent)));
            }
            None => match parent.children.first() {
                Some(first) => {
                    let indent = indent_at(text, first.span.start).map(str::to_string).unwrap_or(indent);
                    let at = first.span.start;
                    self.replace(at..at, format!("{}\n{indent}", block(&indent)));
                }
                None if indent_at(text, parent.close_start).is_some() => {
                    // `>\n   </Parent>`: a new line before the closing tag's.
                    let at = parent.open_end;
                    self.replace(at..at, format!("\n{indent}{}", block(&indent)));
                }
                None => {
                    let at = parent.close_start;
                    self.replace(at..at, format!("\n{indent}{}\n{parent_indent}", block(&indent)));
                }
            },
        }
    }
}

/// An element with attributes and children written the way Xcode writes
/// them, starting at the current position (already indented by `indent`).
/// `children` get the indentation one level deeper.
pub fn block_text(indent: &str, name: &str, attributes: &[(&str, String)], children: &[&dyn Fn(&str) -> String]) -> String {
    let inner = deeper(indent);
    let mut out = format!("<{name}");
    for (key, value) in attributes {
        out.push_str(&format!("\n{inner}{key} = \"{}\"", escape(value)));
    }
    out.push('>');
    for child in children {
        out.push_str(&format!("\n{inner}{}", child(&inner)));
    }
    out.push_str(&format!("\n{indent}</{name}>"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Scheme
   LastUpgradeVersion = "2700"
   version = "1.7">
   <LaunchAction
      buildConfiguration = "Debug"
      launchStyle = "0">
      <BuildableProductRunnable
         runnableDebuggingMode = "0">
      </BuildableProductRunnable>
   </LaunchAction>
   <AnalyzeAction
      buildConfiguration = "Debug">
   </AnalyzeAction>
   <Empty/>
</Scheme>
"#;

    #[test]
    fn parses_spans() {
        let root = parse(SAMPLE).unwrap();
        assert_eq!(root.name, "Scheme");
        assert_eq!(root.attribute("version"), Some("1.7"));
        let launch = root.child("LaunchAction").unwrap();
        assert_eq!(&SAMPLE[launch.span.clone()][..13], "<LaunchAction");
        assert!(SAMPLE[launch.span.clone()].ends_with("</LaunchAction>"));
        assert_eq!(launch.children.len(), 1);
        assert!(root.child("Empty").unwrap().self_closing);
    }

    #[test]
    fn escapes_round_trip() {
        let value = "a \"b\" & <c> 'd'\nnext";
        assert_eq!(unescape(&escape(value)), value);
        assert_eq!(unescape("&#x41;&#66;&bogus;"), "AB&bogus;");
    }

    #[test]
    fn edits_attributes_in_place() {
        let root = parse(SAMPLE).unwrap();
        let launch = root.child("LaunchAction").unwrap();
        let mut edits = Edits::default();
        edits.set_attribute(SAMPLE, launch, "buildConfiguration", "Release");
        edits.set_attribute(SAMPLE, launch, "enableAddressSanitizer", "YES");
        edits.remove_attribute(SAMPLE, launch, "launchStyle");
        let out = edits.apply(SAMPLE).unwrap();
        assert!(out.contains(
            "   <LaunchAction\n      buildConfiguration = \"Release\"\n      enableAddressSanitizer = \"YES\">\n      <BuildableProductRunnable"
        ), "{out}");
        // Everything else is untouched.
        assert!(out.ends_with(&SAMPLE[SAMPLE.find("      <BuildableProductRunnable").unwrap()..]));
    }

    #[test]
    fn inserts_and_removes_children() {
        let root = parse(SAMPLE).unwrap();
        let launch = root.child("LaunchAction").unwrap();
        let runnable = launch.child("BuildableProductRunnable").unwrap();
        let block = |indent: &str| {
            let entry = |indent: &str| block_text(indent, "Entry", &[("key", "A".into())], &[]);
            block_text(indent, "Entries", &[], &[&entry])
        };
        let mut edits = Edits::default();
        edits.insert_child(SAMPLE, launch, Some(runnable), &block);
        let analyze = root.child("AnalyzeAction").unwrap();
        edits.insert_child(SAMPLE, analyze, None, &block);
        let out = edits.apply(SAMPLE).unwrap();
        assert!(out.contains(
            "      </BuildableProductRunnable>\n      <Entries>\n         <Entry\n            key = \"A\">\n         </Entry>\n      </Entries>\n   </LaunchAction>"
        ), "{out}");
        assert!(out.contains("buildConfiguration = \"Debug\">\n      <Entries>"), "{out}");

        // Removing what was added restores the original.
        let root = parse(&out).unwrap();
        let mut edits = Edits::default();
        for action in ["LaunchAction", "AnalyzeAction"] {
            let entries = root.child(action).unwrap().child("Entries").unwrap();
            edits.replace_element(&out, entries, None);
        }
        assert_eq!(edits.apply(&out).unwrap(), SAMPLE);
    }

    #[test]
    fn expands_self_closing_parents() {
        let root = parse(SAMPLE).unwrap();
        let empty = root.child("Empty").unwrap();
        let mut edits = Edits::default();
        edits.insert_child(SAMPLE, empty, None, &|indent| block_text(indent, "A", &[], &[]));
        let out = edits.apply(SAMPLE).unwrap();
        assert!(out.contains("   <Empty>\n      <A>\n      </A>\n   </Empty>\n</Scheme>"), "{out}");
        parse(&out).unwrap();
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse("<A><B></A>").is_err());
        assert!(parse("<A x=1></A>").is_err());
        assert!(parse("<A>").is_err());
    }
}

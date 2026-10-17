//! The expanded tree: repeats unrolled and numbering, fields and wrapped
//! content turned into pieces.

use super::parser::{Element, Item, Kind, Repeat, Token, Value};
use crate::error::Error;
use crate::snippet::{Piece, push_text};

/// The most elements an expansion may have.
pub(crate) const MAX_NODES: usize = 20_000;

#[derive(Clone, Debug, Default)]
pub(crate) struct Node {
    /// `None` for text (and for an element whose name is implicit until
    /// `transform` fills it in).
    pub(crate) name: Option<String>,
    pub(crate) attrs: Vec<NodeAttr>,
    pub(crate) value: Option<Vec<Piece>>,
    pub(crate) self_close: bool,
    pub(crate) children: Vec<Node>,
    /// The innermost repetition the node is in, as (index, count).
    pub(crate) repeat: Option<(usize, usize)>,
    /// Whether the node carries a `*` itself.
    pub(crate) repeated: bool,
    /// Wrapping with an implicit repeat: this repetition's line of content,
    /// for the node's innermost last element.
    pub(crate) line: Option<String>,
}

impl Node {
    pub(crate) fn text(pieces: Vec<Piece>) -> Self {
        Self {
            value: Some(pieces),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NodeAttr {
    pub(crate) name: String,
    pub(crate) value: Option<Vec<Piece>>,
    pub(crate) expr: bool,
    pub(crate) boolean: bool,
}

/// Adds `attr` to `attrs`: a class is appended to an existing one, any
/// other attribute replaces one of the same name where it stands.
pub(crate) fn merge_attr(attrs: &mut Vec<NodeAttr>, attr: NodeAttr) {
    let Some(existing) = attrs.iter_mut().find(|a| a.name == attr.name) else {
        attrs.push(attr);
        return;
    };
    if attr.name != "class" {
        *existing = attr;
        return;
    }
    match (&mut existing.value, attr.value) {
        (Some(old), Some(new)) => {
            if !old.is_empty() && !new.is_empty() {
                old.push(Piece::Text(" ".to_owned()));
            }
            old.extend(new);
        }
        (old @ None, new) => *old = new,
        (Some(_), None) => {}
    }
}

/// The content of Wrap with Abbreviation.
pub(crate) struct Content {
    /// All of it, without its common indentation and blank first and last
    /// lines.
    pub(crate) whole: String,
    /// Its non-empty lines, trimmed: one per implicit repetition.
    pub(crate) lines: Vec<String>,
}

impl Content {
    pub(crate) fn new(text: &str) -> Self {
        let text = text.replace("\r\n", "\n");
        let mut lines: Vec<&str> = text.split('\n').collect();
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
        let first = lines
            .iter()
            .position(|line| !line.trim().is_empty())
            .unwrap_or(lines.len());
        lines.drain(..first);
        let indent_of = |line: &str| line.len() - line.trim_start_matches([' ', '\t']).len();
        let indent = lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| indent_of(line))
            .min()
            .unwrap_or(0);
        let dedented: Vec<&str> = lines
            .iter()
            .map(|line| {
                if line.trim().is_empty() {
                    ""
                } else {
                    line[indent..].trim_end()
                }
            })
            .collect();
        Self {
            whole: dedented.join("\n"),
            lines: dedented
                .iter()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect(),
        }
    }
}

/// Unrolls parsed items into nodes.
pub(crate) struct Unroller<'a> {
    /// The group of the fields: 0 for the abbreviation, one per snippet
    /// instance.
    group: u32,
    content: Option<&'a Content>,
    repeaters: Vec<(usize, usize)>,
    /// The line of the implicit repetition being unrolled.
    line: Option<usize>,
    line_used: bool,
    /// Whether an implicit repeat took the content line by line.
    pub(crate) implicit_used: bool,
    /// Whether `$#` took the whole content.
    pub(crate) content_used: bool,
    budget: &'a mut usize,
}

impl<'a> Unroller<'a> {
    pub(crate) fn new(group: u32, content: Option<&'a Content>, budget: &'a mut usize) -> Self {
        Self {
            group,
            content,
            repeaters: Vec::new(),
            line: None,
            line_used: false,
            implicit_used: false,
            content_used: false,
            budget,
        }
    }

    pub(crate) fn unroll(&mut self, items: &[Item]) -> Result<Vec<Node>, Error> {
        let mut out = Vec::new();
        self.items(items, &mut out)?;
        Ok(out)
    }

    fn items(&mut self, items: &[Item], out: &mut Vec<Node>) -> Result<(), Error> {
        for item in items {
            let Some(repeat) = item.repeat else {
                self.item(item, false, out)?;
                continue;
            };
            let by_line = match (repeat, self.content) {
                (Repeat::Implicit, Some(content)) if !self.implicit_used => {
                    self.implicit_used = true;
                    Some(content)
                }
                _ => None,
            };
            let count = match (repeat, by_line) {
                (_, Some(content)) => content.lines.len().max(1),
                (Repeat::Count(count), None) => count,
                (Repeat::Implicit, None) => 1,
            };
            for index in 0..count {
                let line = by_line.and_then(|content| content.lines.get(index));
                if line.is_some() {
                    self.line = Some(index);
                    self.line_used = false;
                }
                let before = out.len();
                self.repeaters.push((index, count));
                let result = self.item(item, true, out);
                self.repeaters.pop();
                result?;
                if let Some(line) = line {
                    self.line = None;
                    if !self.line_used
                        && out.len() > before
                        && let Some(last) = out.last_mut()
                    {
                        last.line = Some(line.clone());
                    }
                }
            }
        }
        Ok(())
    }

    fn item(&mut self, item: &Item, repeated: bool, out: &mut Vec<Node>) -> Result<(), Error> {
        match &item.kind {
            Kind::Element(element) => {
                let node = self.element(element, repeated)?;
                out.push(node);
            }
            Kind::Group(items) => self.items(items, out)?,
        }
        Ok(())
    }

    fn element(&mut self, element: &Element, repeated: bool) -> Result<Node, Error> {
        if *self.budget == 0 {
            return Err(Error::TooLarge);
        }
        *self.budget -= 1;
        let name = element.name.as_ref().map(|name| self.string(name));
        let mut attrs = Vec::with_capacity(element.attrs.len());
        for attr in &element.attrs {
            let attr = NodeAttr {
                name: self.string(&attr.name),
                value: attr.value.as_ref().map(|value| self.pieces(value)),
                expr: attr.expr,
                boolean: attr.boolean,
            };
            merge_attr(&mut attrs, attr);
        }
        let value = element.text.as_ref().map(|text| self.pieces(text));
        Ok(Node {
            name,
            attrs,
            value,
            self_close: element.self_close,
            children: self.unroll(&element.children)?,
            repeat: self.repeaters.last().copied(),
            repeated,
            line: None,
        })
    }

    fn pieces(&mut self, value: &Value) -> Vec<Piece> {
        let mut pieces = Vec::new();
        for token in value {
            match token {
                Token::Literal(text) => push_text(&mut pieces, text),
                Token::Number {
                    size,
                    reverse,
                    base,
                    raw,
                } => push_text(&mut pieces, &self.number(*size, *reverse, *base, raw)),
                Token::Field { index, placeholder } => pieces.push(Piece::Field {
                    key: (self.group, *index),
                    placeholder: placeholder.clone(),
                }),
                Token::Content => {
                    let text = self.content_text();
                    push_text(&mut pieces, &text);
                }
            }
        }
        pieces
    }

    /// A name: the text of its pieces, a field's placeholder included.
    fn string(&mut self, value: &Value) -> String {
        let mut out = String::new();
        for piece in self.pieces(value) {
            match piece {
                Piece::Text(text) => out.push_str(&text),
                Piece::Field { placeholder, .. } => out.push_str(&placeholder),
            }
        }
        out
    }

    /// The innermost repetition's number; outside one, `$` is literal.
    fn number(&self, size: usize, reverse: bool, base: usize, raw: &str) -> String {
        let Some(&(index, count)) = self.repeaters.last() else {
            return raw.to_owned();
        };
        let number = if reverse {
            base.saturating_add(count - index - 1)
        } else {
            base.saturating_add(index)
        };
        format!("{number:0size$}")
    }

    fn content_text(&mut self) -> String {
        match (self.line, self.content) {
            (Some(index), Some(content)) => {
                self.line_used = true;
                content.lines.get(index).cloned().unwrap_or_default()
            }
            (None, Some(content)) => {
                self.content_used = true;
                content.whole.clone()
            }
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_is_dedented() {
        let content = Content::new("\n    <b>one</b>\n\n      two  \n");
        assert_eq!(content.whole, "<b>one</b>\n\n  two");
        assert_eq!(content.lines, ["<b>one</b>", "two"]);
    }

    #[test]
    fn classes_merge() {
        let class = |text: &str| NodeAttr {
            name: "class".into(),
            value: Some(vec![Piece::Text(text.into())]),
            expr: false,
            boolean: false,
        };
        let mut attrs = vec![class("a")];
        merge_attr(&mut attrs, class("b"));
        assert_eq!(
            attrs[0].value,
            Some(vec![
                Piece::Text("a".into()),
                Piece::Text(" ".into()),
                Piece::Text("b".into())
            ])
        );
    }
}

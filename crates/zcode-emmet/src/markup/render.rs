//! Prints the tree like Emmet's default HTML profile: block elements on
//! lines of their own, indented; inline elements and text kept on their
//! parent's line unless three or more inline elements are adjacent or a
//! block element is next to them.

use super::data::{
    FORMAT_FORCE, FORMAT_SKIP, INLINE_BREAK, is_boolean_attribute, is_inline, is_void,
};
use super::tree::{Node, NodeAttr};
use crate::Options;
use crate::error::Error;
use crate::snippet::{MAX_OUTPUT, Piece, SnippetBuf};

pub(crate) fn render(nodes: &[Node], jsx: bool, options: &Options) -> Result<String, Error> {
    let mut renderer = Renderer {
        buf: SnippetBuf::new(),
        level: 0,
        jsx,
        options,
    };
    renderer.items(nodes, None)?;
    if renderer.buf.len() > MAX_OUTPUT {
        return Err(Error::TooLarge);
    }
    Ok(renderer.buf.finish())
}

struct Renderer<'a> {
    buf: SnippetBuf,
    level: usize,
    jsx: bool,
    options: &'a Options,
}

impl Renderer<'_> {
    fn items(&mut self, items: &[Node], parent: Option<&Node>) -> Result<(), Error> {
        for (index, node) in items.iter().enumerate() {
            self.node(node, index, items, parent)?;
        }
        Ok(())
    }

    fn node(
        &mut self,
        node: &Node,
        index: usize,
        items: &[Node],
        parent: Option<&Node>,
    ) -> Result<(), Error> {
        if self.buf.len() > MAX_OUTPUT {
            return Err(Error::TooLarge);
        }
        let format = should_format(node, index, items, parent);
        let step = indent_step(parent);
        self.level += step;
        if format {
            self.newline();
        }
        if let Some(name) = &node.name {
            self.buf.text("<");
            self.buf.text(name);
            for attr in &node.attrs {
                self.attribute(attr);
            }
            if node.self_close && node.children.is_empty() && node.value.is_none() {
                self.buf.text(if self.jsx || !is_void(name) {
                    " />"
                } else {
                    ">"
                });
            } else {
                self.buf.text(">");
                if let Some(value) = &node.value {
                    let inner = has_newline(value) || starts_with_block_tag(value);
                    if inner {
                        self.level += 1;
                        self.newline();
                    }
                    self.pieces(value);
                    if inner {
                        self.level -= 1;
                        self.newline();
                    }
                }
                self.items(&node.children, Some(node))?;
                if node.value.is_none() && node.children.is_empty() {
                    let inner = FORMAT_FORCE.contains(&name.as_str());
                    if inner {
                        self.level += 1;
                        self.newline();
                    }
                    self.buf.tabstop("");
                    if inner {
                        self.level -= 1;
                        self.newline();
                    }
                }
                self.buf.text("</");
                self.buf.text(name);
                self.buf.text(">");
            }
        } else {
            if let Some(value) = &node.value {
                self.pieces(value);
            }
            self.items(&node.children, Some(node))?;
        }
        // The parent's closing tag goes on a line of its own after a
        // formatted last child.
        if format && index + 1 == items.len() && parent.is_some() {
            self.level -= step;
            self.newline();
        } else {
            self.level -= step;
        }
        Ok(())
    }

    fn attribute(&mut self, attr: &NodeAttr) {
        self.buf.text(" ");
        self.buf.text(&attr.name);
        match &attr.value {
            None if attr.boolean || is_boolean_attribute(&attr.name) => {}
            Some(value) if attr.expr => {
                self.buf.text("={");
                if is_empty(value) {
                    self.buf.tabstop("");
                } else {
                    self.buf.pieces(value);
                }
                self.buf.text("}");
            }
            Some(value) if !is_empty(value) => {
                let has_double = value
                    .iter()
                    .any(|p| matches!(p, Piece::Text(t) if t.contains('"')));
                let quote = if has_double { "'" } else { "\"" };
                self.buf.text("=");
                self.buf.text(quote);
                self.buf.pieces(value);
                self.buf.text(quote);
            }
            _ => {
                self.buf.text("=\"");
                self.buf.tabstop("");
                self.buf.text("\"");
            }
        }
    }

    /// Text and fields; lines after the first are indented at the current
    /// level.
    fn pieces(&mut self, pieces: &[Piece]) {
        for piece in pieces {
            match piece {
                Piece::Text(text) => {
                    let mut lines = text.split('\n');
                    if let Some(first) = lines.next() {
                        self.buf.text(first);
                    }
                    for line in lines {
                        self.buf.text(&self.options.newline);
                        if !line.is_empty() {
                            self.indent();
                            self.buf.text(line);
                        }
                    }
                }
                Piece::Field { key, placeholder } => self.buf.field(*key, placeholder),
            }
        }
    }

    fn newline(&mut self) {
        self.buf.text(&self.options.newline);
        self.indent();
    }

    fn indent(&mut self) {
        for _ in 0..self.level {
            self.buf.text(&self.options.indent);
        }
    }
}

fn is_text(node: &Node) -> bool {
    node.name.is_none()
}

fn is_inline_node(node: &Node) -> bool {
    is_text(node) || is_inline_element(node)
}

fn is_inline_element(node: &Node) -> bool {
    node.name.as_deref().is_some_and(is_inline)
}

fn is_empty(pieces: &[Piece]) -> bool {
    pieces
        .iter()
        .all(|p| matches!(p, Piece::Text(t) if t.is_empty()))
}

fn has_newline(pieces: &[Piece]) -> bool {
    pieces
        .iter()
        .any(|p| matches!(p, Piece::Text(t) if t.contains('\n')))
}

fn has_field(pieces: &[Piece]) -> bool {
    pieces.iter().any(|p| matches!(p, Piece::Field { .. }))
}

/// Text starting with a block element's tag (`<div>…`) is put on its own
/// lines.
fn starts_with_block_tag(pieces: &[Piece]) -> bool {
    let Some(Piece::Text(text)) = pieces.first() else {
        return false;
    };
    let Some(rest) = text.trim_start().strip_prefix('<') else {
        return false;
    };
    let name_len = rest
        .find(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | ':' | '_')))
        .unwrap_or(rest.len());
    name_len > 0 && !is_inline(&rest[..name_len])
}

/// Children are indented one level, except inside text and `html`.
fn indent_step(parent: Option<&Node>) -> usize {
    match parent.and_then(|p| p.name.as_deref()) {
        Some(name) if !FORMAT_SKIP.contains(&name) => 1,
        _ => 0,
    }
}

/// Whether `node` starts on a new line: Emmet's `shouldFormat`.
fn should_format(node: &Node, index: usize, items: &[Node], parent: Option<&Node>) -> bool {
    if index == 0 && parent.is_none() {
        return false;
    }
    if parent.is_some_and(is_text) && items.len() == 1 {
        return false;
    }
    if is_text(node) {
        let value = node.value.as_deref().unwrap_or_default();
        let adjacent =
            (index > 0 && is_text(&items[index - 1])) || items.get(index + 1).is_some_and(is_text);
        if adjacent || has_newline(value) || (has_field(value) && !node.children.is_empty()) {
            return true;
        }
    }
    if !is_inline_node(node) {
        return true;
    }
    if index == 0 {
        // First inline child: on its own line when a block sibling follows.
        if items.iter().any(|item| !is_inline_node(item)) {
            return true;
        }
    } else if !is_inline_node(&items[index - 1]) {
        return true;
    }
    let before = items[..index]
        .iter()
        .rev()
        .take_while(|n| is_inline_element(n))
        .count();
    let after = items[index + 1..]
        .iter()
        .take_while(|n| is_inline_element(n))
        .count();
    if 1 + before + after >= INLINE_BREAK {
        return true;
    }
    node.children
        .iter()
        .enumerate()
        .any(|(i, child)| should_format(child, i, &node.children, Some(node)))
}

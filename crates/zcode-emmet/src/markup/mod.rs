//! HTML and JSX abbreviations: parse, unroll, resolve snippets, transform,
//! render.

pub(crate) mod data;
pub(crate) mod parser;
mod render;
pub(crate) mod snippets;
mod transform;
mod tree;

use self::parser::Mode;
use self::snippets::{Resolver, innermost_last};
use self::tree::{Content, MAX_NODES, Node, Unroller};
use crate::Options;
use crate::error::Error;
use crate::lorem::Lorem;
use crate::snippet::Piece;

pub(crate) fn expand(abbreviation: &str, jsx: bool, options: &Options) -> Result<String, Error> {
    build(abbreviation, None, jsx, options)
}

pub(crate) fn wrap(
    abbreviation: &str,
    content: &str,
    jsx: bool,
    options: &Options,
) -> Result<String, Error> {
    build(abbreviation, Some(&Content::new(content)), jsx, options)
}

fn build(
    abbreviation: &str,
    content: Option<&Content>,
    jsx: bool,
    options: &Options,
) -> Result<String, Error> {
    let items = parser::parse(
        abbreviation,
        Mode {
            jsx,
            snippet: false,
        },
    )?;
    let mut budget = MAX_NODES;
    let mut unroller = Unroller::new(0, content, &mut budget);
    let nodes = unroller.unroll(&items)?;
    let placed = unroller.implicit_used || unroller.content_used;
    let mut nodes = Resolver::new(jsx, &mut budget).resolve(nodes)?;
    transform::transform(&mut nodes, None, jsx, &mut Lorem::new())?;
    if let Some(content) = content {
        insert_lines(&mut nodes);
        if !placed && !content.whole.is_empty() {
            insert_content(&mut nodes, &content.whole);
        }
    }
    render::render(&nodes, jsx, options)
}

/// Puts each implicit repetition's line of content into its innermost last
/// element.
fn insert_lines(nodes: &mut [Node]) {
    for node in nodes {
        match node.line.take() {
            Some(line) => put(innermost_last(node), line),
            None => insert_lines(&mut node.children),
        }
    }
}

/// Puts the wrapped content into the innermost last element, or after
/// everything when the abbreviation ends with text or a void element.
fn insert_content(nodes: &mut Vec<Node>, content: &str) {
    match nodes.last_mut() {
        Some(last) if last.name.is_some() && !last.self_close => {
            put(innermost_last(last), content.to_owned())
        }
        _ => nodes.push(Node::text(vec![Piece::Text(content.to_owned())])),
    }
}

fn put(target: &mut Node, text: String) {
    let pieces = vec![Piece::Text(text)];
    if target.value.is_none() && target.children.is_empty() {
        target.value = Some(pieces);
        target.self_close = false;
    } else {
        target.children.push(Node::text(pieces));
    }
}

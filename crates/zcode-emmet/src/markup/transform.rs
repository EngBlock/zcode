//! What Emmet fills in once snippets are resolved: implicit tag names, lorem
//! ipsum, void elements and JSX's attribute names.

use super::data::{implicit_name, is_void};
use super::tree::Node;
use crate::error::Error;
use crate::lorem::{self, Lorem};
use crate::snippet::Piece;

pub(crate) fn transform(
    nodes: &mut [Node],
    parent: Option<&str>,
    jsx: bool,
    lorem: &mut Lorem,
) -> Result<(), Error> {
    for node in nodes.iter_mut() {
        if node.name.is_none() && !node.attrs.is_empty() {
            node.name = Some(implicit_name(parent).to_owned());
        }
        if let Some(words) = node.name.as_deref().and_then(lorem::word_count) {
            let first = node.repeat.is_none_or(|(index, _)| index == 0);
            node.value = Some(vec![Piece::Text(lorem.paragraph(words, first)?)]);
            node.attrs.clear();
            // `ul>lorem*3` is three `li`s of lorem; a lone `lorem` is text.
            node.name =
                (node.repeated && parent.is_some()).then(|| implicit_name(parent).to_owned());
        }
        if let Some(name) = &node.name {
            validate_name(name)?;
            if is_void(name) {
                node.self_close = true;
            }
        }
        if jsx {
            for attr in &mut node.attrs {
                match attr.name.as_str() {
                    "class" => attr.name = "className".to_owned(),
                    "for" => attr.name = "htmlFor".to_owned(),
                    _ => {}
                }
            }
        }
        let Node { name, children, .. } = node;
        transform(children, name.as_deref().or(parent), jsx, lorem)?;
    }
    Ok(())
}

/// Element names start with a letter or `_` and go on with letters, digits,
/// `-`, `_`, `:` (namespaces) and `.` (JSX members).
fn validate_name(name: &str) -> Result<(), Error> {
    let mut chars = name.chars();
    let valid = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'));
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidName(name.to_owned()))
    }
}

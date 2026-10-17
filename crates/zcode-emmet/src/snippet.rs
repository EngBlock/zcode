//! LSP snippet output: literal text and numbered tabstops, escaped on the way
//! out.

use std::collections::HashMap;
use std::fmt::Write as _;

/// The most bytes an expansion may produce.
pub(crate) const MAX_OUTPUT: usize = 1 << 20;

/// Part of a value in the expanded tree: literal text or a field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    Text(String),
    /// A field of a snippet definition or of the abbreviation (`${1}`,
    /// `${1:placeholder}`). `key` is (group, index): fields with the same key
    /// are one tabstop, and every snippet instance has a group of its own.
    Field {
        key: (u32, u32),
        placeholder: String,
    },
}

/// Appends `text` to `pieces`, joining it to a preceding text piece.
pub(crate) fn push_text(pieces: &mut Vec<Piece>, text: &str) {
    if text.is_empty() {
        return;
    }
    match pieces.last_mut() {
        Some(Piece::Text(last)) => last.push_str(text),
        _ => pieces.push(Piece::Text(text.to_owned())),
    }
}

enum Segment {
    Text(String),
    Tabstop { number: u32, placeholder: String },
}

/// A snippet being written: text and tabstops, numbered in the order they
/// are written.
pub(crate) struct SnippetBuf {
    segments: Vec<Segment>,
    len: usize,
    fields: HashMap<(u32, u32), u32>,
    next: u32,
}

impl SnippetBuf {
    pub(crate) fn new() -> Self {
        Self {
            segments: Vec::new(),
            len: 0,
            fields: HashMap::new(),
            next: 1,
        }
    }

    /// Roughly the bytes written so far.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Literal text (escaped when the snippet is finished).
    pub(crate) fn text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.len += text.len();
        match self.segments.last_mut() {
            Some(Segment::Text(last)) => last.push_str(text),
            _ => self.segments.push(Segment::Text(text.to_owned())),
        }
    }

    /// A tabstop of its own.
    pub(crate) fn tabstop(&mut self, placeholder: &str) {
        let number = self.next;
        self.next += 1;
        self.push_tabstop(number, placeholder);
    }

    /// The tabstop of a field: the same key always gets the same number.
    pub(crate) fn field(&mut self, key: (u32, u32), placeholder: &str) {
        let number = match self.fields.get(&key) {
            Some(&number) => number,
            None => {
                let number = self.next;
                self.next += 1;
                self.fields.insert(key, number);
                number
            }
        };
        self.push_tabstop(number, placeholder);
    }

    /// Pieces as they are (no indentation of their lines).
    pub(crate) fn pieces(&mut self, pieces: &[Piece]) {
        for piece in pieces {
            match piece {
                Piece::Text(text) => self.text(text),
                Piece::Field { key, placeholder } => self.field(*key, placeholder),
            }
        }
    }

    fn push_tabstop(&mut self, number: u32, placeholder: &str) {
        self.len += placeholder.len() + 4;
        self.segments.push(Segment::Tabstop {
            number,
            placeholder: placeholder.to_owned(),
        });
    }

    /// The snippet string.
    pub(crate) fn finish(self) -> String {
        let mut out = String::with_capacity(self.len + self.len / 8);
        for (i, segment) in self.segments.iter().enumerate() {
            match segment {
                Segment::Text(text) => escape_into(&mut out, text),
                Segment::Tabstop {
                    number,
                    placeholder,
                } if placeholder.is_empty() => {
                    // `$1` followed by `0` would read as `$10`.
                    let braced = matches!(
                        self.segments.get(i + 1),
                        Some(Segment::Text(next)) if next.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                    );
                    if braced {
                        let _ = write!(out, "${{{number}}}");
                    } else {
                        let _ = write!(out, "${number}");
                    }
                }
                Segment::Tabstop {
                    number,
                    placeholder,
                } => {
                    let _ = write!(out, "${{{number}:");
                    escape_into(&mut out, placeholder);
                    out.push('}');
                }
            }
        }
        out
    }
}

/// Escapes the characters that are special in LSP snippets.
fn escape_into(out: &mut String, text: &str) {
    for c in text.chars() {
        if matches!(c, '$' | '}' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
}

/// The byte length of `s` up to the `}` that closes an already opened `{`
/// (nested braces balanced), if there is one.
pub(crate) fn closing_brace(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Splits a template of the snippet tables (`border: ${1:1px} ${2:solid}`)
/// into pieces whose fields are in `group`.
pub(crate) fn parse_template(template: &str, group: u32) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        push_text(&mut pieces, &rest[..start]);
        let after = &rest[start + 2..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let field = after[..digits].parse::<u32>().ok().and_then(|index| {
            let tail = &after[digits..];
            if tail.starts_with('}') {
                Some((index, String::new(), digits + 1))
            } else {
                let placeholder = tail.strip_prefix(':')?;
                let end = closing_brace(placeholder)?;
                Some((index, placeholder[..end].to_owned(), digits + 1 + end + 1))
            }
        });
        match field {
            Some((index, placeholder, used)) => {
                pieces.push(Piece::Field {
                    key: (group, index),
                    placeholder,
                });
                rest = &after[used..];
            }
            None => {
                push_text(&mut pieces, "${");
                rest = after;
            }
        }
    }
    push_text(&mut pieces, rest);
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_numbers() {
        let mut buf = SnippetBuf::new();
        buf.text("a$b}c\\");
        buf.tabstop("");
        buf.text("0");
        buf.field((1, 1), "x}");
        buf.field((1, 1), "");
        buf.tabstop("");
        assert_eq!(buf.finish(), "a\\$b\\}c\\\\${1}0${2:x\\}}$2$3");
    }

    #[test]
    fn templates() {
        let pieces = parse_template("a ${1:b {c}} ${2} $x ${", 7);
        assert_eq!(
            pieces,
            vec![
                Piece::Text("a ".into()),
                Piece::Field {
                    key: (7, 1),
                    placeholder: "b {c}".into()
                },
                Piece::Text(" ".into()),
                Piece::Field {
                    key: (7, 2),
                    placeholder: String::new()
                },
                Piece::Text(" $x ${".into()),
            ]
        );
    }
}

//! Finding the abbreviation that ends at the cursor, like Emmet's
//! `extract-abbreviation`.

use crate::markup::parser::{self, Mode};
use crate::{Syntax, css};

/// JavaScript keywords that are never an abbreviation in JSX, and the only
/// word an abbreviation may follow there (`return ul>li`).
const JS_KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "get",
    "if",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "of",
    "return",
    "set",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "yield",
];

pub(crate) fn extract(line: &str, syntax: Syntax) -> Option<(usize, &str)> {
    match syntax {
        Syntax::Css => css::extract(line),
        Syntax::Html => markup(line, false),
        Syntax::Jsx => markup(line, true),
    }
}

fn markup(line: &str, jsx: bool) -> Option<(usize, &str)> {
    let start = scan_start(line)?;
    let abbreviation = &line[start..];
    let before = &line[..start];
    if abbreviation.is_empty() || inside_tag(before) || (jsx && !jsx_context(before, abbreviation))
    {
        return None;
    }
    parser::parse(
        abbreviation,
        Mode {
            jsx,
            snippet: false,
        },
    )
    .ok()?;
    Some((start, abbreviation))
}

/// Characters an abbreviation has outside its brackets.
fn is_abbreviation_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '-' | '_' | ':' | '$' | '@' | '!' | '#' | '.' | '*' | '>' | '+' | '^' | '/'
        )
}

/// Scans back from the end over the abbreviation: whitespace and quotes
/// inside `[...]` and anything inside `{...}` belong to it, brackets must
/// balance, and it stops at the `>` that ends a tag (`<div>ul>li`) or any
/// character an abbreviation can't have.
fn scan_start(line: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    // The opening bracket each closing bracket seen so far needs.
    let mut stack: Vec<char> = Vec::new();
    let mut i = chars.len();
    while i > 0 {
        i -= 1;
        let (pos, c) = chars[i];
        match stack.last() {
            Some('{') => {
                match c {
                    '}' => stack.push('{'),
                    '{' => {
                        stack.pop();
                    }
                    _ => {}
                }
                continue;
            }
            Some('[') => {
                match c {
                    '"' | '\'' => i = chars[..i].iter().rposition(|&(_, q)| q == c)?,
                    ']' => stack.push('['),
                    '[' => {
                        stack.pop();
                    }
                    _ => {}
                }
                continue;
            }
            _ => {}
        }
        match c {
            ']' => stack.push('['),
            '}' => stack.push('{'),
            ')' => stack.push('('),
            '(' if stack.last() == Some(&'(') => {
                stack.pop();
            }
            '>' if is_tag_end(&line[..pos]) => return stack.is_empty().then_some(pos + 1),
            c if is_abbreviation_char(c) => {}
            _ => return stack.is_empty().then_some(pos + c.len_utf8()),
        }
    }
    stack.is_empty().then_some(0)
}

/// Whether the `>` after `before` closes a tag (`<div>`, `</p>`, `<a
/// href="x">`, `<>`, `<!-- -->`) rather than being the child operator.
fn is_tag_end(before: &str) -> bool {
    let Some(lt) = before.rfind('<') else {
        return false;
    };
    let tag = &before[lt + 1..];
    if tag.contains('>') {
        return false;
    }
    let tag = tag.strip_prefix('/').unwrap_or(tag);
    tag.is_empty() || tag.starts_with(|c: char| c.is_ascii_alphabetic() || c == '!')
}

/// Whether text ending in `before` leaves a tag open (the cursor is among
/// its attributes).
fn inside_tag(before: &str) -> bool {
    let Some(lt) = before.rfind('<') else {
        return false;
    };
    !before[lt..].contains('>')
        && before[lt + 1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '/')
}

/// In JSX, an abbreviation can't be a keyword or follow `=`, `,`, `.` or an
/// identifier (`const a = b`, `foo(a, b)`, `typeof x`), except `return`.
fn jsx_context(before: &str, abbreviation: &str) -> bool {
    if JS_KEYWORDS.contains(&abbreviation) {
        return false;
    }
    let before = before.trim_end();
    match before.chars().last() {
        None => true,
        Some('=' | ',' | '.') => false,
        Some(c) if c.is_alphanumeric() || c == '_' || c == '$' => {
            let word_start = before
                .char_indices()
                .rev()
                .take_while(|&(_, c)| c.is_alphanumeric() || c == '_' || c == '$')
                .last()
                .map_or(0, |(i, _)| i);
            &before[word_start..] == "return"
        }
        Some(_) => true,
    }
}

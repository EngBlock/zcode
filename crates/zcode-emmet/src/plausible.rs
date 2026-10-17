//! Whether an abbreviation is worth offering unasked.

use crate::markup::data::is_element;
use crate::markup::snippets;
use crate::{Options, Syntax, css, lorem, markup};

/// Longer abbreviations are typed on purpose, not offered.
const MAX_LEN: usize = 512;

pub(crate) fn is_plausible(abbreviation: &str, syntax: Syntax) -> bool {
    if abbreviation.is_empty() || abbreviation.len() > MAX_LEN {
        return false;
    }
    match syntax {
        Syntax::Css => css::is_known(abbreviation),
        Syntax::Html => markup_plausible(abbreviation, false),
        Syntax::Jsx => markup_plausible(abbreviation, true),
    }
}

fn markup_plausible(abbreviation: &str, jsx: bool) -> bool {
    // In JSX a group must start like anything else: `(a+b)` is JavaScript.
    let start = if jsx {
        abbreviation.trim_start_matches('(')
    } else {
        abbreviation
    };
    let lead_len = start
        .find(['>', '+', '*', '.', '#', '[', '{', '(', ')', '^', '/'])
        .unwrap_or(start.len());
    let lead = &start[..lead_len];
    let has_operators = lead_len < start.len() || start.len() < abbreviation.len();
    let shape = if jsx {
        if lead.is_empty() {
            start.starts_with(['.', '#'])
        } else {
            is_known(lead, true)
        }
    } else if lead.is_empty() {
        // Text alone (`{name}`) is more likely Svelte or Vue than Emmet.
        !start.starts_with('{')
    } else if has_operators {
        is_known(lead, false) || lead.contains('-') || lead.starts_with(|c: char| c.is_uppercase())
    } else {
        is_known(lead, false)
    };
    shape && markup::expand(abbreviation, jsx, &Options::default()).is_ok()
}

/// A known element, snippet or lorem name. `$` numbering stands for a digit
/// (`h$` is `h1`).
fn is_known(name: &str, jsx: bool) -> bool {
    let numbered;
    let name = if name.contains('$') {
        numbered = strip_numbering(name);
        numbered.as_str()
    } else {
        name
    };
    is_element(name) || snippets::lookup(name, jsx).is_some() || lorem::word_count(name).is_some()
}

/// `h$$@2` as `h1`.
fn strip_numbering(name: &str) -> String {
    let mut out = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        while chars.peek() == Some(&'$') {
            chars.next();
        }
        if chars.peek() == Some(&'@') {
            chars.next();
            while chars
                .peek()
                .is_some_and(|c| *c == '-' || c.is_ascii_digit())
            {
                chars.next();
            }
        }
        out.push('1');
    }
    out
}

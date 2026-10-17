//! Vim's patterns as Rust regexes, and searching the text with them.
//!
//! Patterns are read as Vim's default "magic" ones: `\(`…`\)` group,
//! `\|` alternates, `\{n,m}`, `\+`, `\?` and `\=` repeat, `\<` and `\>`
//! match word edges, while a bare `(`, `|`, `{`, `+` or `?` is the
//! character itself. `\v` makes the rest "very magic" (regex syntax as is),
//! `\V` makes it literal, and `\c` / `\C` ignore case or don't.

use std::ops::Range;

use regex::{Regex, RegexBuilder};
use ropey::Rope;

/// The regex for a Vim pattern, or the message Vim would show.
pub(super) fn compile(pattern: &str) -> Result<Regex, String> {
    let mut out = String::new();
    let mut ignore_case = false;
    let mut chars = pattern.chars().peekable();
    #[derive(PartialEq)]
    enum Magic {
        Normal,
        Very,
        Literal,
    }
    let mut magic = Magic::Normal;
    let mut in_count = false;
    while let Some(c) = chars.next() {
        if c == '\\' {
            let Some(next) = chars.next() else {
                out.push_str("\\\\");
                break;
            };
            match next {
                'c' => ignore_case = true,
                'C' => ignore_case = false,
                'v' => magic = Magic::Very,
                'V' => magic = Magic::Literal,
                'm' | 'M' => magic = Magic::Normal,
                '<' | '>' => out.push_str("\\b"),
                _ if magic == Magic::Literal => match next {
                    '\\' => out.push_str("\\\\"),
                    'n' => out.push_str("\\n"),
                    't' => out.push_str("\\t"),
                    other => out.push_str(&regex::escape(&other.to_string())),
                },
                '(' | ')' | '|' | '+' | '?' if magic == Magic::Normal => out.push(next),
                '=' if magic == Magic::Normal => out.push('?'),
                '{' if magic == Magic::Normal => {
                    in_count = true;
                    out.push('{');
                    // `\{-}` is Vim's lazy `*`.
                    if chars.peek() == Some(&'-') {
                        chars.next();
                        if chars.peek() == Some(&'}') {
                            chars.next();
                            in_count = false;
                            out.pop();
                            out.push_str("*?");
                        }
                    }
                }
                '}' if in_count => {
                    in_count = false;
                    out.push('}');
                }
                's' | 'S' | 'd' | 'D' | 'w' | 'W' | 'n' | 't' | 'b' | 'B' => {
                    out.push('\\');
                    out.push(next);
                }
                'a' => out.push_str("[A-Za-z]"),
                'A' => out.push_str("[^A-Za-z]"),
                'l' => out.push_str("[a-z]"),
                'u' => out.push_str("[A-Z]"),
                'x' => out.push_str("[0-9A-Fa-f]"),
                'h' => out.push_str("[A-Za-z_]"),
                other => out.push_str(&regex::escape(&other.to_string())),
            }
            continue;
        }
        match magic {
            Magic::Literal => out.push_str(&regex::escape(&c.to_string())),
            Magic::Very => match c {
                '<' | '>' => out.push_str("\\b"),
                '=' => out.push('?'),
                _ => out.push(c),
            },
            Magic::Normal => match c {
                '(' | ')' | '|' | '+' | '?' | '{' => {
                    out.push('\\');
                    out.push(c);
                }
                '}' if in_count => {
                    in_count = false;
                    out.push('}');
                }
                '}' => out.push_str("\\}"),
                '~' => out.push('~'),
                _ => out.push(c),
            },
        }
    }
    RegexBuilder::new(&out)
        .case_insensitive(ignore_case)
        .multi_line(true)
        .build()
        .map_err(|_| format!("E383: Invalid search string: {pattern}"))
}

/// The pattern `*` and `#` search for: the word under the cursor, whole.
pub(super) fn word_pattern(word: &str) -> String {
    let edges = word.chars().all(|c| c.is_alphanumeric() || c == '_');
    let escaped: String = word
        .chars()
        .map(|c| if "\\/.*$^~[]".contains(c) { format!("\\{c}") } else { c.to_string() })
        .collect();
    if edges { format!("\\<{escaped}\\>") } else { escaped }
}

/// The next match after `from` (or the previous one before it, going
/// backward), wrapping around the end; whether it wrapped.
pub(super) fn find(text: &Rope, regex: &Regex, from: usize, forward: bool) -> Option<(Range<usize>, bool)> {
    let source = text.to_string();
    let matches = regex.find_iter(&source).map(|m| m.range()).filter(|r| !(r.is_empty() && r.start == source.len()));
    if forward {
        let mut first = None;
        for range in matches {
            if range.start > from {
                return Some((range, false));
            }
            first.get_or_insert(range);
        }
        first.map(|range| (range, true))
    } else {
        let mut before = None;
        let mut last = None;
        for range in matches {
            if range.start < from {
                before = Some(range.clone());
            }
            last = Some(range);
        }
        before.map(|range| (range, false)).or(last.map(|range| (range, true)))
    }
}

/// Every match, for highlighting.
pub(super) fn all(text: &Rope, regex: &Regex) -> Vec<Range<usize>> {
    let source = text.to_string();
    regex.find_iter(&source).map(|m| m.range()).filter(|r| !r.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, text: &str) -> Vec<String> {
        let regex = compile(pattern).unwrap();
        regex.find_iter(text).map(|m| m.as_str().to_string()).collect()
    }

    #[test]
    fn reads_vims_magic_patterns() {
        assert_eq!(matches(r"\<is\>", "this is it"), ["is"]);
        assert_eq!(matches(r"a\(b\|c\)\+", "abcbd ac"), ["abcb", "ac"]);
        assert_eq!(matches(r"f(x)", "f(x) fx"), ["f(x)"]);
        assert_eq!(matches(r"x\{2}", "x xx xxx"), ["xx", "xx"]);
        assert_eq!(matches(r"a.\{-}b", "aXbYb"), ["aXb"]);
        assert_eq!(matches(r"\cHELLO", "hello"), ["hello"]);
        assert_eq!(matches(r"\v(a|b)+", "abba c"), ["abba"]);
        assert_eq!(matches(r"\Va.b", "axb a.b"), ["a.b"]);
        assert_eq!(matches(r"^\s*let", "  let x\nlet y"), ["  let", "let"]);
        assert_eq!(matches(&word_pattern("count"), "count counter count"), ["count", "count"]);
        assert_eq!(matches(&word_pattern("a.b"), "a.b axb"), ["a.b"]);
    }

    #[test]
    fn finds_wrapping_around() {
        let text = Rope::from_str("one two one two");
        let regex = compile("two").unwrap();
        assert_eq!(find(&text, &regex, 0, true), Some((4..7, false)));
        assert_eq!(find(&text, &regex, 4, true), Some((12..15, false)));
        assert_eq!(find(&text, &regex, 12, true), Some((4..7, true)));
        assert_eq!(find(&text, &regex, 12, false), Some((4..7, false)));
        assert_eq!(find(&text, &regex, 4, false), Some((12..15, true)));
        assert_eq!(find(&text, &compile("zzz").unwrap(), 0, true), None);
    }
}

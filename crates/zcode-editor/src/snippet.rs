//! LSP snippets (`foo(${1:Int})$0`, Emmet's `<a href="$1">$2</a>`) as the
//! editor inserts them: a placeholder with text becomes an Xcode-style
//! `<#text#>` token, and every tab stop is a range of the inserted text.
//! After insertion the first stop is selected and Tab moves through the
//! others ([`crate::EditorState::insert_snippet`]).

use std::ops::Range;

/// A snippet's text and its tab stops: byte ranges of `text`, in the order
/// Tab visits them (`$1`, `$2`, … then `$0`). A stop without a placeholder
/// is empty; one with a placeholder covers its `<#…#>` token.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    pub stops: Vec<Range<usize>>,
}

/// Expands an LSP snippet. Variables (`$TM_FILENAME`, `${CLIPBOARD}`) become
/// their default text or nothing; a choice (`${1|a,b|}`) its first option;
/// a tab stop seen before (a mirror) the text it was given.
pub fn expand(snippet: &str) -> Expansion {
    let chars: Vec<char> = snippet.chars().collect();
    let mut parser = Parser { chars: &chars, i: 0, text: String::new(), stops: vec![] };
    parser.run(None);
    let mut stops = parser.stops;
    // `$0` last; the first occurrence of a number is its stop.
    stops.sort_by_key(|(n, _, _)| if *n == 0 { u32::MAX } else { *n });
    let mut seen = vec![];
    let stops = stops
        .into_iter()
        .filter(|(n, _, mirror)| !*mirror && if seen.contains(n) { false } else { seen.push(*n); true })
        .map(|(_, range, _)| range)
        .collect();
    Expansion { text: parser.text, stops }
}

/// The snippet's text with its placeholders as `<#…#>` tokens.
pub fn to_placeholders(snippet: &str) -> String {
    expand(snippet).text
}

struct Parser<'a> {
    chars: &'a [char],
    i: usize,
    text: String,
    /// Number, range, and whether it repeats an earlier stop's number.
    stops: Vec<(u32, Range<usize>, bool)>,
}

impl Parser<'_> {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.i + ahead).copied()
    }

    /// Reads until the end, or the `}` closing a placeholder when `inside`
    /// (the `}` is consumed).
    fn run(&mut self, inside: Option<()>) {
        while let Some(ch) = self.peek(0) {
            match ch {
                '\\' if matches!(self.peek(1), Some('$' | '}' | '\\' | ',' | '|')) => {
                    self.text.push(self.peek(1).unwrap_or('\\'));
                    self.i += 2;
                }
                '}' if inside.is_some() => {
                    self.i += 1;
                    return;
                }
                '$' => {
                    if !self.dollar() {
                        self.text.push('$');
                        self.i += 1;
                    }
                }
                ch => {
                    self.text.push(ch);
                    self.i += 1;
                }
            }
        }
    }

    fn number(&mut self) -> Option<u32> {
        let start = self.i;
        while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        (self.i > start).then(|| self.chars[start..self.i].iter().collect::<String>().parse().unwrap_or(0))
    }

    fn name(&mut self) -> bool {
        let start = self.i;
        while self.peek(0).is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            self.i += 1;
        }
        self.i > start
    }

    fn stop(&mut self, number: u32, range: Range<usize>) {
        let mirror = self.stops.iter().any(|(n, _, _)| *n == number);
        self.stops.push((number, range, mirror));
    }

    /// At a `$`: a tab stop, placeholder, choice or variable. `false` (and
    /// nothing read) when it's a plain dollar sign.
    fn dollar(&mut self) -> bool {
        let start = self.i;
        self.i += 1;
        if let Some(number) = self.number() {
            let at = self.text.len();
            self.repeat_or_stop(number, at..at);
            return true;
        }
        if self.peek(0) != Some('{') {
            if self.name() {
                return true;
            }
            self.i = start;
            return false;
        }
        self.i += 1;
        if let Some(number) = self.number() {
            match self.peek(0) {
                Some('}') => {
                    self.i += 1;
                    let at = self.text.len();
                    self.repeat_or_stop(number, at..at);
                }
                Some(':') => {
                    self.i += 1;
                    if let Some(earlier) = self.mirror_text(number) {
                        // A mirror shows the first occurrence's text.
                        let mut inner = Parser { chars: self.chars, i: self.i, text: String::new(), stops: vec![] };
                        inner.run(Some(()));
                        self.i = inner.i;
                        self.text.push_str(&earlier);
                        return true;
                    }
                    let token_start = self.text.len();
                    self.text.push_str("<#");
                    let inner_start = self.text.len();
                    let stops_before = self.stops.len();
                    self.run(Some(()));
                    // Nested stops inside a placeholder are part of its text.
                    self.stops.truncate(stops_before);
                    if self.text.len() == inner_start {
                        self.text.truncate(token_start);
                        let at = self.text.len();
                        self.stop(number, at..at);
                    } else {
                        self.text.push_str("#>");
                        let end = self.text.len();
                        self.stop(number, token_start..end);
                    }
                }
                Some('|') => {
                    self.i += 1;
                    let mut first = String::new();
                    let mut in_first = true;
                    while let Some(ch) = self.peek(0) {
                        self.i += 1;
                        match ch {
                            '\\' => {
                                if let Some(next) = self.peek(0) {
                                    self.i += 1;
                                    if in_first {
                                        first.push(next);
                                    }
                                }
                            }
                            ',' => in_first = false,
                            '|' if self.peek(0) == Some('}') => {
                                self.i += 1;
                                break;
                            }
                            ch if in_first => first.push(ch),
                            _ => {}
                        }
                    }
                    let token_start = self.text.len();
                    if !first.is_empty() {
                        self.text.push_str("<#");
                        self.text.push_str(&first);
                        self.text.push_str("#>");
                    }
                    let end = self.text.len();
                    self.stop(number, token_start..end);
                }
                _ => {
                    self.i = start;
                    return false;
                }
            }
            return true;
        }
        // `${NAME}` or `${NAME:default}`.
        if self.name() {
            match self.peek(0) {
                Some('}') => {
                    self.i += 1;
                    return true;
                }
                Some(':') => {
                    self.i += 1;
                    let stops_before = self.stops.len();
                    self.run(Some(()));
                    self.stops.truncate(stops_before);
                    return true;
                }
                _ => {}
            }
        }
        self.i = start;
        false
    }

    /// A stop at `range`, or for a number seen before, that stop's text.
    fn repeat_or_stop(&mut self, number: u32, range: Range<usize>) {
        match self.mirror_text(number) {
            Some(text) => self.text.push_str(&text),
            None => self.stop(number, range),
        }
    }

    /// The text of the stop numbered `number` seen before, without its
    /// `<#…#>` marks.
    fn mirror_text(&self, number: u32) -> Option<String> {
        let (_, range, _) = self.stops.iter().find(|(n, _, _)| *n == number)?;
        let text = &self.text[range.clone()];
        Some(text.strip_prefix("<#").and_then(|t| t.strip_suffix("#>")).unwrap_or(text).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_become_tokens() {
        let e = expand("increment(by: ${1:Int})");
        assert_eq!(e.text, "increment(by: <#Int#>)");
        assert_eq!(e.stops, [14..21]);
        assert_eq!(to_placeholders("print($0)"), "print()");
        assert_eq!(to_placeholders("a(${1})"), "a()");
        assert_eq!(to_placeholders("x\\$y"), "x$y");
        assert_eq!(to_placeholders("cost: $"), "cost: $");
    }

    #[test]
    fn tab_stops_in_order_with_zero_last() {
        let e = expand("<a href=\"$1\">$2</a>$0");
        assert_eq!(e.text, "<a href=\"\"></a>");
        assert_eq!(e.stops, [9..9, 11..11, 15..15]);
        let e = expand("$2 then $1");
        assert_eq!(e.stops, [6..6, 0..0]);
    }

    #[test]
    fn choices_variables_and_mirrors() {
        assert_eq!(to_placeholders("${1|one,two|}"), "<#one#>");
        assert_eq!(to_placeholders("${TM_FILENAME} ${CLIPBOARD:fallback} $HOME"), " fallback ");
        let e = expand("${1:name} = ${1}");
        assert_eq!(e.text, "<#name#> = name");
        assert_eq!(e.stops.len(), 1);
        // Nested placeholders are text of the outer one.
        assert_eq!(to_placeholders("${1:foo(${2:bar})}"), "<#foo(<#bar#>)#>");
        assert_eq!(expand("${1:foo(${2:bar})}").stops.len(), 1);
    }

    #[test]
    fn malformed_snippets_stay_text() {
        assert_eq!(to_placeholders("${"), "${");
        assert_eq!(to_placeholders("${1:open"), "<#open#>");
        assert_eq!(to_placeholders("$ {1}"), "$ {1}");
        for s in ["${1|a,b", "\\", "$", "${x", "${1:${2:${3}}}"] {
            expand(s);
        }
    }
}

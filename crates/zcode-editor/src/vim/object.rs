//! Text objects: what `iw`, `a"`, `i(`, `ip` and `it` select around the
//! cursor.

use std::ops::Range;

use ropey::Rope;

use super::text::{self, Class, class};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Object {
    Word { big: bool },
    Paragraph,
    Quote(char),
    Bracket(char, char),
    Tag,
}

impl Object {
    pub(super) fn from_char(c: char) -> Option<Self> {
        Some(match c {
            'w' => Object::Word { big: false },
            'W' => Object::Word { big: true },
            'p' => Object::Paragraph,
            '"' | '\'' | '`' => Object::Quote(c),
            '(' | ')' | 'b' => Object::Bracket('(', ')'),
            '[' | ']' => Object::Bracket('[', ']'),
            '{' | '}' | 'B' => Object::Bracket('{', '}'),
            '<' | '>' => Object::Bracket('<', '>'),
            't' => Object::Tag,
            _ => return None,
        })
    }
}

/// What an object covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Selected {
    pub(super) range: Range<usize>,
    /// Whole lines (`ip`, `ap`).
    pub(super) linewise: bool,
    /// The inside of a bracket pair on lines of their own (`i{` over a
    /// block): changing it leaves an empty line to type on.
    pub(super) inner_lines: bool,
}

impl Selected {
    fn chars(range: Range<usize>) -> Self {
        Selected { range, linewise: false, inner_lines: false }
    }
}

/// The object around `cursor`; `around` is `a…` rather than `i…`. In visual
/// mode `current` is the selection, so selecting `i(` again takes the next
/// pair out.
pub(super) fn select(
    text: &Rope,
    cursor: usize,
    object: Object,
    around: bool,
    count: usize,
    current: Option<Range<usize>>,
) -> Option<Selected> {
    let count = count.max(1);
    match object {
        Object::Word { big } => word(text, cursor, big, around, count),
        Object::Paragraph => paragraph(text, cursor, around, count),
        Object::Quote(quote) => self::quote(text, cursor, quote, around),
        Object::Bracket(open, close) => {
            let mut n = count;
            loop {
                let selected = bracket(text, cursor, open, close, around, n)?;
                let grows = current.as_ref().is_some_and(|current| {
                    current.start <= selected.range.start && selected.range.end <= current.end
                });
                if !grows {
                    return Some(selected);
                }
                n += 1;
            }
        }
        Object::Tag => {
            let mut n = count;
            loop {
                let selected = tag(text, cursor, around, n)?;
                let grows = current.as_ref().is_some_and(|current| {
                    current.start <= selected.range.start && selected.range.end <= current.end && current.len() > 1
                });
                if !grows {
                    return Some(selected);
                }
                n += 1;
            }
        }
    }
}

/// The run of characters of one class around `offset` in its line.
fn run(text: &Rope, offset: usize, big: bool) -> Range<usize> {
    let row = text::row(text, offset);
    let (start, end) = (text::line_start(text, row), text::line_end(text, row));
    let Some(c) = text::char_at(text, offset).filter(|_| offset < end) else { return offset..offset };
    let k = class(c, big);
    let from = text::backward(text, offset)
        .take_while(|(o, c)| *o >= start && class(*c, big) == k)
        .last()
        .map_or(offset, |(o, _)| o);
    let to = text::forward(text, offset)
        .take_while(|(o, c)| *o < end && class(*c, big) == k)
        .last()
        .map_or(offset, |(o, c)| o + c.len_utf8());
    from..to
}

fn word(text: &Rope, cursor: usize, big: bool, around: bool, count: usize) -> Option<Selected> {
    let row = text::row(text, cursor);
    let (start, end) = (text::line_start(text, row), text::line_end(text, row));
    if start == end {
        return None;
    }
    let cursor = cursor.min(text::prev(text, end));
    let is_blank_at = |offset| text::char_at(text, offset).is_some_and(|c| class(c, big) == Class::Blank);
    let on_blank = is_blank_at(cursor);
    let mut range = run(text, cursor, big);
    let extend = |range: &mut Range<usize>| {
        if range.end < end {
            range.end = run(text, range.end, big).end;
        }
    };
    if !around {
        // Blank runs count as words too.
        for _ in 1..count {
            extend(&mut range);
        }
    } else if on_blank {
        // The blanks and the word after them, for each count.
        extend(&mut range);
        for _ in 1..count {
            extend(&mut range);
            extend(&mut range);
        }
    } else {
        // Each word with the blanks after it; without any after the last,
        // the blanks before the first.
        let mut trailing = false;
        for i in 0..count {
            if i > 0 {
                extend(&mut range);
            }
            trailing = range.end < end && is_blank_at(range.end);
            if trailing {
                extend(&mut range);
            }
        }
        if !trailing && range.start > start && is_blank_at(text::prev(text, range.start)) {
            range.start = run(text, text::prev(text, range.start), big).start;
        }
    }
    Some(Selected::chars(range))
}

fn paragraph(text: &Rope, cursor: usize, around: bool, count: usize) -> Option<Selected> {
    let last = text::last_row(text);
    let blank = |row| text::is_blank_line(text, row);
    let row = text::row(text, cursor).min(last);
    // The last row of the run of lines (blank or not) starting at `row`.
    let run_end = |mut row: usize| {
        let kind = blank(row);
        while row < last && blank(row + 1) == kind {
            row += 1;
        }
        row
    };
    let mut first = row;
    while first > 0 && blank(first - 1) == blank(row) {
        first -= 1;
    }
    let mut end = run_end(row);
    if !around {
        for _ in 1..count {
            if end < last {
                end = run_end(end + 1);
            }
        }
    } else {
        // A paragraph and the blank lines after it (or blank lines and the
        // paragraph after them), for each count.
        for i in 0..count {
            if i > 0 && end < last {
                end = run_end(end + 1);
            }
            if end < last {
                end = run_end(end + 1);
            } else if i == 0 && !blank(row) {
                // Nothing after it: the blank lines before.
                while first > 0 && blank(first - 1) {
                    first -= 1;
                }
            }
        }
    }
    Some(Selected {
        range: text::line_start(text, first)..text::line_full_end(text, end),
        linewise: true,
        inner_lines: false,
    })
}

fn quote(text: &Rope, cursor: usize, quote: char, around: bool) -> Option<Selected> {
    let row = text::row(text, cursor);
    let (start, end) = (text::line_start(text, row), text::line_end(text, row));
    let mut quotes = vec![];
    let mut escaped = false;
    for (offset, c) in text::forward(text, start).take_while(|(o, _)| *o < end) {
        if c == quote && !escaped {
            quotes.push(offset);
        }
        escaped = c == '\\' && !escaped;
    }
    let (open, close) = quotes
        .chunks_exact(2)
        .map(|pair| (pair[0], pair[1]))
        // The pair the cursor is in, or the next one on the line.
        .find(|&(_, close)| cursor <= close)?;
    let mut range = if around { open..close + quote.len_utf8() } else { open + quote.len_utf8()..close };
    if around {
        let trailing = text::forward(text, range.end).take_while(|(o, c)| *o < end && text::is_blank(*c)).last();
        if let Some((o, c)) = trailing {
            range.end = o + c.len_utf8();
        } else if let Some((o, _)) =
            text::backward(text, range.start).take_while(|(o, c)| *o >= start && text::is_blank(*c)).last()
        {
            range.start = o;
        }
    }
    Some(Selected::chars(range))
}

/// The `n`th pair of `open`/`close` around `cursor` (the cursor on a bracket
/// counts as inside its pair).
fn enclosing_pair(text: &Rope, cursor: usize, open: char, close: char, n: usize) -> Option<(usize, usize)> {
    let mut from = cursor;
    let mut result = None;
    for i in 0..n {
        let on = text::char_at(text, from);
        let open_at = if i == 0 && on == Some(open) {
            from
        } else if i == 0 && on == Some(close) {
            matching(text, from, open, close, false)?
        } else {
            let mut depth = 0usize;
            let mut found = None;
            for (offset, c) in text::backward(text, from) {
                if c == close {
                    depth += 1;
                } else if c == open {
                    if depth == 0 {
                        found = Some(offset);
                        break;
                    }
                    depth -= 1;
                }
            }
            found?
        };
        let close_at = matching(text, open_at, open, close, true)?;
        result = Some((open_at, close_at));
        from = open_at;
    }
    result
}

/// The bracket matching the one at `at`, searching forward from an opening
/// one or backward from a closing one.
fn matching(text: &Rope, at: usize, open: char, close: char, forward: bool) -> Option<usize> {
    let mut depth = 0usize;
    if forward {
        for (offset, c) in text::forward(text, at) {
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
        }
    } else {
        for (offset, c) in text::backward(text, text::next(text, at)) {
            if c == close {
                depth += 1;
            } else if c == open {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
        }
    }
    None
}

fn bracket(text: &Rope, cursor: usize, open: char, close: char, around: bool, n: usize) -> Option<Selected> {
    let (open_at, close_at) = enclosing_pair(text, cursor, open, close, n)?;
    if around {
        return Some(Selected::chars(open_at..close_at + close.len_utf8()));
    }
    let mut range = open_at + open.len_utf8()..close_at;
    // The brackets on lines of their own around a block: the lines between.
    let open_row = text::row(text, open_at);
    let close_row = text::row(text, close_at);
    let rest_blank = text::forward(text, range.start)
        .take_while(|(o, _)| *o < text::line_end(text, open_row))
        .all(|(_, c)| text::is_blank(c));
    let before_blank = text::first_non_blank(text, close_row) == close_at;
    let inner_lines = close_row > open_row && rest_blank && before_blank;
    if inner_lines {
        range = text::line_start(text, open_row + 1)..text::line_start(text, close_row);
    }
    Some(Selected { range, linewise: false, inner_lines })
}

fn tag(text: &Rope, cursor: usize, around: bool, n: usize) -> Option<Selected> {
    // Every tag in the text, paired up with a stack.
    let source = text.to_string();
    let pattern = regex::Regex::new(r"<(/?)([A-Za-z][\w:.-]*)[^<>]*?(/?)>").ok()?;
    let mut stack: Vec<(String, Range<usize>)> = vec![];
    let mut pairs: Vec<(Range<usize>, Range<usize>)> = vec![];
    for captures in pattern.captures_iter(&source) {
        let whole = captures.get(0)?.range();
        let name = captures[2].to_string();
        if !captures[3].is_empty() {
            continue;
        }
        if captures[1].is_empty() {
            stack.push((name, whole));
        } else if let Some(ix) = stack.iter().rposition(|(open, _)| *open == name) {
            let (_, open) = stack.remove(ix);
            stack.truncate(ix);
            pairs.push((open, whole));
        }
    }
    let mut enclosing: Vec<_> = pairs
        .into_iter()
        .filter(|(open, close)| open.start <= cursor && cursor < close.end)
        .collect();
    enclosing.sort_by_key(|(open, close)| close.end - open.start);
    let (open, close) = enclosing.get(n - 1)?.clone();
    Some(Selected::chars(if around { open.start..close.end } else { open.end..close.start }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(text: &str, cursor: usize, object: Object, around: bool) -> Option<String> {
        let rope = Rope::from_str(text);
        select(&rope, cursor, object, around, 1, None).map(|s| rope.slice(s.range).to_string())
    }

    #[test]
    fn words_with_and_without_their_blanks() {
        let w = Object::Word { big: false };
        assert_eq!(sel("foo bar baz", 5, w, false).as_deref(), Some("bar"));
        assert_eq!(sel("foo bar baz", 5, w, true).as_deref(), Some("bar "));
        assert_eq!(sel("foo bar", 5, w, true).as_deref(), Some(" bar"), "no blanks after: the ones before");
        assert_eq!(sel("foo  bar", 3, w, false).as_deref(), Some("  "));
        assert_eq!(sel("foo  bar", 3, w, true).as_deref(), Some("  bar"));
        assert_eq!(sel("a.b", 0, Object::Word { big: true }, false).as_deref(), Some("a.b"));
        let rope = Rope::from_str("one two three");
        let three = select(&rope, 0, w, false, 3, None).unwrap();
        assert_eq!(rope.slice(three.range).to_string(), "one two");
        let two = select(&rope, 0, w, true, 2, None).unwrap();
        assert_eq!(rope.slice(two.range).to_string(), "one two ");
    }

    #[test]
    fn quotes_and_brackets() {
        let q = Object::Quote('"');
        assert_eq!(sel(r#"say "hi \"x\"" now"#, 6, q, false).as_deref(), Some(r#"hi \"x\""#));
        assert_eq!(sel(r#"a "b" c"#, 0, q, false).as_deref(), Some("b"), "the next pair on the line");
        assert_eq!(sel(r#"a "b" c"#, 3, q, true).as_deref(), Some(r#""b" "#));
        let p = Object::Bracket('(', ')');
        assert_eq!(sel("f(a, (b), c)", 3, p, false).as_deref(), Some("a, (b), c"));
        assert_eq!(sel("f(a, (b), c)", 6, p, true).as_deref(), Some("(b)"));
        assert_eq!(sel("f(a, (b), c)", 5, p, false).as_deref(), Some("b"), "on the bracket");
        let rope = Rope::from_str("f(a, (b), c)");
        let outer = select(&rope, 6, p, false, 2, None).unwrap();
        assert_eq!(rope.slice(outer.range).to_string(), "a, (b), c");
        let again = select(&rope, 6, p, false, 1, Some(6..7)).unwrap();
        assert_eq!(again.range, 2..11, "selecting it again grows");
        let block = "if x {\n    a\n    b\n}\n";
        let rope = Rope::from_str(block);
        let inner = select(&rope, 12, Object::Bracket('{', '}'), false, 1, None).unwrap();
        assert!(inner.inner_lines);
        assert_eq!(rope.slice(inner.range).to_string(), "    a\n    b\n");
    }

    #[test]
    fn paragraphs_and_tags() {
        let text = "a\nb\n\n\nc\n";
        assert_eq!(sel(text, 0, Object::Paragraph, false).as_deref(), Some("a\nb\n"));
        assert_eq!(sel(text, 0, Object::Paragraph, true).as_deref(), Some("a\nb\n\n\n"));
        assert_eq!(sel(text, 6, Object::Paragraph, true).as_deref(), Some("\n\nc\n"), "the last one takes the blanks before");
        let html = "<div><p class=\"x\">hi <b>there</b></p></div>";
        assert_eq!(sel(html, 20, Object::Tag, false).as_deref(), Some("hi <b>there</b>"));
        assert_eq!(sel(html, 25, Object::Tag, true).as_deref(), Some("<b>there</b>"));
        assert_eq!(sel(html, 2, Object::Tag, false).as_deref(), Some("<p class=\"x\">hi <b>there</b></p>"));
    }
}

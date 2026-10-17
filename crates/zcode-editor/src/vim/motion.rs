//! Motions: where `w`, `j`, `}`, `f(`, `%`… take the cursor, and whether an
//! operator applied over them takes characters up to the target, through
//! it, or whole lines.

use std::ops::Range;

use ropey::Rope;

use super::text::{self, Class, class};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// Up to the target, not including it (`w`, `b`, `h`).
    Exclusive,
    /// Through the target character (`e`, `f`, `$`, `%`).
    Inclusive,
    /// Whole lines from the cursor's to the target's (`j`, `G`, `'a`).
    Linewise,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordStart { big: bool },
    WordEnd { big: bool },
    WordBack { big: bool },
    WordEndBack { big: bool },
    /// `0`
    LineStart,
    /// `^`
    FirstNonBlank,
    /// `$`
    LineEnd,
    /// `g_`
    LastNonBlank,
    /// `|`
    Column,
    /// `gg`, or line N with a count.
    FirstLine,
    /// `G`, or line N with a count.
    LastLine,
    ParagraphForward,
    ParagraphBack,
    Find { c: char, forward: bool, till: bool },
    /// `;` (or `,` with `reverse`): the last `f`, `F`, `t` or `T` again.
    RepeatFind { reverse: bool },
    /// `%`: the bracket matching the one at or after the cursor; N% goes to
    /// N percent of the file.
    Match,
    ScreenTop,
    ScreenMiddle,
    ScreenBottom,
    /// `+`, Return
    NextLine,
    /// `-`
    PreviousLine,
    /// `_`
    CurrentLine,
    /// `n` (or `N` with `reverse`), and the search a `/`, `?`, `*` or `#` starts.
    Search { reverse: bool },
    /// `` `a `` (exact) or `'a` (the line).
    Mark { name: char, line: bool },
}

impl Motion {
    pub(super) fn is_jump(self) -> bool {
        matches!(
            self,
            Motion::FirstLine
                | Motion::LastLine
                | Motion::ParagraphForward
                | Motion::ParagraphBack
                | Motion::Match
                | Motion::ScreenTop
                | Motion::ScreenMiddle
                | Motion::ScreenBottom
                | Motion::Search { .. }
                | Motion::Mark { .. }
        )
    }

    /// Whether the cursor keeps the column it wanted (`j` after `$`).
    pub(super) fn keeps_column(self) -> bool {
        matches!(self, Motion::Up | Motion::Down)
    }
}

/// Where a motion lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub(super) offset: usize,
    pub(super) kind: Kind,
}

/// The last `f`, `F`, `t` or `T`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LastFind {
    pub(super) c: char,
    pub(super) forward: bool,
    pub(super) till: bool,
}

/// What a motion needs besides the text and the cursor.
pub(super) struct Env<'a> {
    pub(super) text: &'a Rope,
    pub(super) count: Option<usize>,
    /// The character column `j` and `k` aim for (`usize::MAX`: the end).
    pub(super) column: usize,
    /// Buffer rows on screen, for `H`, `M` and `L`.
    pub(super) visible_rows: Option<Range<usize>>,
    pub(super) last_find: Option<LastFind>,
    /// Rows hidden in folds, which `j` and `k` step over.
    pub(super) hidden: &'a dyn Fn(usize) -> bool,
    /// The next search match from an offset (`reverse` flips the search's
    /// direction).
    pub(super) search: &'a dyn Fn(usize, bool) -> Option<usize>,
    pub(super) mark: &'a dyn Fn(char) -> Option<usize>,
    /// Whether an operator is waiting for the motion (`dl` may reach the
    /// end of the line, `l` may not).
    pub(super) operator: bool,
}

impl Env<'_> {
    fn n(&self) -> usize {
        self.count.unwrap_or(1).max(1)
    }
}

pub(super) fn eval(motion: Motion, from: usize, env: &Env) -> Option<Target> {
    use Kind::*;
    let text = env.text;
    let n = env.n();
    let row = text::row(text, from);
    let target = |offset, kind| Some(Target { offset, kind });
    match motion {
        Motion::Left => {
            let start = text::line_start(text, row);
            if from == start {
                return env.operator.then_some(Target { offset: from, kind: Exclusive });
            }
            let mut offset = from;
            for _ in 0..n {
                if offset == start {
                    break;
                }
                offset = text::prev(text, offset);
            }
            target(offset, Exclusive)
        }
        Motion::Right => {
            let end = text::line_end(text, row);
            let last = if env.operator { end } else { text::last_char(text, row) };
            if from >= last {
                return env.operator.then_some(Target { offset: from, kind: Exclusive });
            }
            let mut offset = from;
            for _ in 0..n {
                if offset >= last {
                    break;
                }
                offset = text::next(text, offset);
            }
            target(offset.min(last), Exclusive)
        }
        Motion::Down | Motion::Up => {
            let mut to = row;
            for _ in 0..n {
                let mut next = if motion == Motion::Down { to + 1 } else { to.checked_sub(1)? };
                while next > 0 && next < text::last_row(text) && (env.hidden)(next) {
                    next = if motion == Motion::Down { next + 1 } else { next - 1 };
                }
                if next > text::last_row(text) {
                    break;
                }
                to = next;
            }
            if to == row {
                return None;
            }
            target(text::at_column(text, to, env.column, false), Linewise)
        }
        Motion::WordStart { big } => {
            let mut offset = from;
            for _ in 0..n {
                offset = next_word_start(text, offset, big);
            }
            (offset != from || env.operator).then_some(Target { offset, kind: Exclusive })
        }
        Motion::WordEnd { big } => {
            let mut offset = from;
            for _ in 0..n {
                offset = next_word_end(text, offset, big);
            }
            target(offset, Inclusive)
        }
        Motion::WordBack { big } => {
            let mut offset = from;
            for _ in 0..n {
                offset = previous_word_start(text, offset, big);
            }
            target(offset, Exclusive)
        }
        Motion::WordEndBack { big } => {
            let mut offset = from;
            for _ in 0..n {
                offset = previous_word_end(text, offset, big);
            }
            target(offset, Inclusive)
        }
        Motion::LineStart => target(text::line_start(text, row), Exclusive),
        Motion::FirstNonBlank => target(text::first_non_blank(text, row), Exclusive),
        Motion::LineEnd => {
            let row = (row + n - 1).min(text::last_row(text));
            let end = text::line_end(text, row);
            // Inclusive of the last character; an empty line has none.
            let offset = if end > text::line_start(text, row) { text::prev(text, end) } else { end };
            target(offset, Inclusive)
        }
        Motion::LastNonBlank => {
            let row = (row + n - 1).min(text::last_row(text));
            let start = text::line_start(text, row);
            let offset = text::backward(text, text::line_end(text, row))
                .take_while(|(o, _)| *o >= start)
                .find(|(_, c)| !text::is_blank(*c))
                .map_or(start, |(o, _)| o);
            target(offset, Inclusive)
        }
        Motion::Column => target(text::at_column(text, row, n - 1, false), Exclusive),
        Motion::FirstLine | Motion::LastLine => {
            let to = match env.count {
                Some(line) => line.max(1) - 1,
                None if motion == Motion::FirstLine => 0,
                None => text::last_row(text),
            }
            .min(text::last_row(text));
            target(text::first_non_blank(text, to), Linewise)
        }
        Motion::ParagraphForward => {
            let mut to = row;
            let last = text::last_row(text);
            for _ in 0..n {
                while to < last && is_empty_line(text, to) {
                    to += 1;
                }
                while to < last && !is_empty_line(text, to) {
                    to += 1;
                }
            }
            let offset = if is_empty_line(text, to) { text::line_start(text, to) } else { text::line_end(text, to) };
            target(offset, Exclusive)
        }
        Motion::ParagraphBack => {
            let mut to = row;
            for _ in 0..n {
                while to > 0 && is_empty_line(text, to) {
                    to -= 1;
                }
                while to > 0 && !is_empty_line(text, to) {
                    to -= 1;
                }
            }
            target(text::line_start(text, to), Exclusive)
        }
        Motion::Find { c, forward, till } => find(text, from, n, LastFind { c, forward, till }, false),
        Motion::RepeatFind { reverse } => {
            let mut last = env.last_find?;
            if reverse {
                last.forward = !last.forward;
            }
            find(text, from, n, last, true)
        }
        Motion::Match => match env.count {
            Some(percent) => {
                let rows = text::last_row(text) + 1;
                let to = ((percent.min(100) * rows).div_ceil(100)).max(1) - 1;
                target(text::first_non_blank(text, to), Linewise)
            }
            None => target(matching_bracket(text, from)?, Inclusive),
        },
        Motion::ScreenTop | Motion::ScreenMiddle | Motion::ScreenBottom => {
            let rows = env.visible_rows.clone().unwrap_or(row..row + 1);
            let last = rows.end.saturating_sub(1).min(text::last_row(text)).max(rows.start);
            let to = match motion {
                Motion::ScreenTop => (rows.start + n - 1).min(last),
                Motion::ScreenBottom => last.saturating_sub(n - 1).max(rows.start),
                _ => rows.start + (last - rows.start) / 2,
            };
            target(text::first_non_blank(text, to), Linewise)
        }
        Motion::NextLine => {
            let to = row + n;
            (to <= text::last_row(text)).then(|| Target { offset: text::first_non_blank(text, to), kind: Linewise })
        }
        Motion::PreviousLine => {
            let to = row.checked_sub(n)?;
            target(text::first_non_blank(text, to), Linewise)
        }
        Motion::CurrentLine => {
            let to = (row + n - 1).min(text::last_row(text));
            target(text::first_non_blank(text, to), Linewise)
        }
        Motion::Search { reverse } => {
            let mut offset = from;
            for _ in 0..n {
                offset = (env.search)(offset, reverse)?;
            }
            target(offset, Exclusive)
        }
        Motion::Mark { name, line } => {
            let offset = (env.mark)(name)?.min(text.len());
            if line {
                let row = text::row(text, offset).min(text::last_row(text));
                target(text::first_non_blank(text, row), Linewise)
            } else {
                target(offset, Exclusive)
            }
        }
    }
}

fn is_empty_line(text: &Rope, row: usize) -> bool {
    text::line_start(text, row) == text::line_end(text, row)
}

/// Whether an empty line starts at `offset` (a newline right after another,
/// or at the start).
fn empty_line_at(text: &Rope, offset: usize) -> bool {
    text::char_at(text, offset) == Some('\n') && (offset == 0 || text::char_at(text, offset - 1) == Some('\n'))
}

pub(super) fn next_word_start(text: &Rope, from: usize, big: bool) -> usize {
    let mut chars = text::forward(text, from);
    let Some((_, first)) = chars.next() else { return text.len() };
    let mut last = class(first, big);
    for (offset, c) in chars {
        let k = class(c, big);
        match k {
            Class::Newline if last == Class::Newline => return offset,
            Class::Newline | Class::Blank => {}
            _ if k != last => return offset,
            _ => {}
        }
        last = k;
    }
    text.len()
}

pub(super) fn next_word_end(text: &Rope, from: usize, big: bool) -> usize {
    let mut chars = text::forward(text, from)
        .skip(1)
        .skip_while(|(_, c)| matches!(class(*c, big), Class::Blank | Class::Newline));
    let Some((mut end, first)) = chars.next() else { return text::prev(text, text.len()) };
    let k = class(first, big);
    for (offset, c) in chars {
        if class(c, big) != k {
            break;
        }
        end = offset;
    }
    end
}

pub(super) fn previous_word_start(text: &Rope, from: usize, big: bool) -> usize {
    let mut word: Option<(Class, usize)> = None;
    for (offset, c) in text::backward(text, from) {
        let k = class(c, big);
        match word {
            None if matches!(k, Class::Blank | Class::Newline) => {
                if empty_line_at(text, offset) {
                    return offset;
                }
            }
            None => word = Some((k, offset)),
            Some((w, _)) if w == k => word = Some((w, offset)),
            Some(_) => break,
        }
    }
    word.map_or(0, |(_, offset)| offset)
}

pub(super) fn previous_word_end(text: &Rope, from: usize, big: bool) -> usize {
    let current = text::char_at(text, from).map(|c| class(c, big));
    let mut left_word = !matches!(current, Some(Class::Word | Class::Punctuation));
    for (offset, c) in text::backward(text, from) {
        let k = class(c, big);
        match k {
            Class::Blank => left_word = true,
            Class::Newline => {
                if empty_line_at(text, offset) {
                    return offset;
                }
                left_word = true;
            }
            _ if left_word || Some(k) != current => return offset,
            _ => {}
        }
    }
    0
}

fn find(text: &Rope, from: usize, n: usize, find: LastFind, repeat: bool) -> Option<Target> {
    let row = text::row(text, from);
    let (start, end) = (text::line_start(text, row), text::line_end(text, row));
    let mut offset = from;
    for i in 0..n {
        // `;` after `t` steps past the character it stopped before.
        let skip_adjacent = find.till && repeat && i == 0;
        offset = if find.forward {
            let from = if skip_adjacent { text::next(text, offset) } else { offset };
            text::forward(text, text::next(text, from))
                .take_while(|(o, _)| *o < end)
                .find(|(_, c)| *c == find.c)?
                .0
        } else {
            let from = if skip_adjacent { text::prev(text, offset) } else { offset };
            text::backward(text, from).take_while(|(o, _)| *o >= start).find(|(_, c)| *c == find.c)?.0
        };
        if find.till && i + 1 == n {
            offset = if find.forward { text::prev(text, offset) } else { text::next(text, offset) };
        }
    }
    Some(Target { offset, kind: if find.forward { Kind::Inclusive } else { Kind::Exclusive } })
}

const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// The bracket matching the first bracket at or after `from` on its line.
pub(super) fn matching_bracket(text: &Rope, from: usize) -> Option<usize> {
    let end = text::line_end(text, text::row(text, from));
    let (at, c) = text::forward(text, from)
        .take_while(|(o, _)| *o < end)
        .find(|(_, c)| PAIRS.iter().any(|(open, close)| c == open || c == close))?;
    let (open, close) = *PAIRS.iter().find(|(open, close)| c == *open || c == *close)?;
    let mut depth = 0usize;
    if c == open {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env(text: &Rope) -> Env<'_> {
        Env {
            text,
            count: None,
            column: 0,
            visible_rows: None,
            last_find: None,
            hidden: &|_| false,
            search: &|_, _| None,
            mark: &|_| None,
            operator: false,
        }
    }

    fn at(text: &str, motion: Motion, from: usize) -> Option<usize> {
        let rope = Rope::from_str(text);
        eval(motion, from, &env(&rope)).map(|t| t.offset)
    }

    #[test]
    fn words_stop_at_word_and_punctuation_starts_and_empty_lines() {
        let w = Motion::WordStart { big: false };
        let text = "foo.bar  baz\n\nqux";
        assert_eq!(at(text, w, 0), Some(3));
        assert_eq!(at(text, w, 3), Some(4));
        assert_eq!(at(text, w, 4), Some(9));
        assert_eq!(at(text, w, 9), Some(13), "the empty line");
        assert_eq!(at(text, w, 13), Some(14));
        assert_eq!(at(text, Motion::WordStart { big: true }, 0), Some(9));
        let e = Motion::WordEnd { big: false };
        assert_eq!(at(text, e, 0), Some(2));
        assert_eq!(at(text, e, 2), Some(3));
        assert_eq!(at(text, e, 9), Some(11));
        assert_eq!(at(text, e, 11), Some(16), "past the empty line");
        let b = Motion::WordBack { big: false };
        assert_eq!(at(text, b, 14), Some(13), "back to the empty line");
        assert_eq!(at(text, b, 13), Some(9));
        assert_eq!(at(text, b, 9), Some(4));
        assert_eq!(at(text, b, 5), Some(4));
        assert_eq!(at(text, b, 4), Some(3));
        assert_eq!(at(text, Motion::WordBack { big: true }, 9), Some(0));
        let ge = Motion::WordEndBack { big: false };
        assert_eq!(at(text, ge, 9), Some(6));
        assert_eq!(at(text, ge, 6), Some(3));
        assert_eq!(at(text, ge, 14), Some(13));
    }

    #[test]
    fn line_motions() {
        let text = "  ab  \nxyz";
        assert_eq!(at(text, Motion::FirstNonBlank, 5), Some(2));
        assert_eq!(at(text, Motion::LineStart, 5), Some(0));
        assert_eq!(at(text, Motion::LineEnd, 0), Some(5));
        assert_eq!(at(text, Motion::LastNonBlank, 0), Some(3));
        assert_eq!(at(text, Motion::Right, 5), None, "l stops at the last character");
        assert_eq!(at(text, Motion::Left, 0), None);
        let rope = Rope::from_str(text);
        let mut env = env(&rope);
        env.column = 9;
        assert_eq!(eval(Motion::Down, 1, &env), Some(Target { offset: 9, kind: Kind::Linewise }));
        env.count = Some(2);
        assert_eq!(eval(Motion::LastLine, 0, &env).unwrap().offset, 7);
        env.operator = true;
        env.count = None;
        assert_eq!(eval(Motion::Right, 5, &env).unwrap().offset, 6, "dl reaches the end of the line");
        assert_eq!(eval(Motion::Right, 6, &env).unwrap().offset, 6, "and still has a target there");
    }

    #[test]
    fn paragraphs_brackets_and_finds() {
        let text = "a\nb\n\nc\n";
        assert_eq!(at(text, Motion::ParagraphForward, 0), Some(4));
        assert_eq!(at(text, Motion::ParagraphForward, 4), Some(6), "the end of the last line");
        assert_eq!(at(text, Motion::ParagraphBack, 5), Some(4));
        assert_eq!(at(text, Motion::ParagraphBack, 4), Some(0));
        let code = "f(a[1], {b})";
        assert_eq!(at(code, Motion::Match, 0), Some(11));
        assert_eq!(at(code, Motion::Match, 11), Some(1));
        assert_eq!(at(code, Motion::Match, 3), Some(5));
        let find = |c, forward, till| Motion::Find { c, forward, till };
        assert_eq!(at(code, find(',', true, false), 0), Some(6));
        assert_eq!(at(code, find(',', true, true), 0), Some(5));
        assert_eq!(at(code, find('(', false, false), 6), Some(1));
        assert_eq!(at(code, find('(', false, true), 6), Some(2));
        assert_eq!(at(code, find('z', true, false), 0), None);
        let rope = Rope::from_str("a,b,c,d");
        let mut env = env(&rope);
        env.last_find = Some(LastFind { c: ',', forward: true, till: true });
        assert_eq!(eval(Motion::RepeatFind { reverse: false }, 0, &env).unwrap().offset, 2, "; after t moves on");
        env.count = Some(2);
        assert_eq!(eval(Motion::Find { c: ',', forward: true, till: false }, 0, &env).unwrap().offset, 3);
        let rope = Rope::from_str("a,,b");
        let mut env = super::tests::env(&rope);
        env.count = Some(2);
        assert_eq!(eval(Motion::Find { c: ',', forward: true, till: true }, 0, &env).unwrap().offset, 1);
    }
}

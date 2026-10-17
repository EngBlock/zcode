//! Characters, words and lines of a rope as Vim sees them.
//!
//! Offsets are UTF-8 bytes, as everywhere in the editor. Vim's lines are
//! the rope's lines, except that the empty line after a final newline isn't
//! one: `G` goes to the last line with text, `j` stops there.

use ropey::{LineType, Rope};

pub(super) fn char_at(text: &Rope, offset: usize) -> Option<char> {
    (offset < text.len()).then(|| text.char(offset))
}

/// The offset after the character at `offset` (the end stays the end).
pub(super) fn next(text: &Rope, offset: usize) -> usize {
    match char_at(text, offset) {
        Some(c) => offset + c.len_utf8(),
        None => text.len(),
    }
}

/// The offset of the character before `offset` (0 stays 0).
pub(super) fn prev(text: &Rope, offset: usize) -> usize {
    if offset == 0 { 0 } else { text.floor_char_boundary(offset.min(text.len()) - 1) }
}

pub(super) fn row(text: &Rope, offset: usize) -> usize {
    text.byte_to_line_idx(offset.min(text.len()), LineType::LF)
}

/// The last line Vim counts.
pub(super) fn last_row(text: &Rope) -> usize {
    let lines = text.len_lines(LineType::LF);
    if lines > 1 && text.len() > 0 && text.byte(text.len() - 1) == b'\n' { lines - 2 } else { lines - 1 }
}

pub(super) fn line_start(text: &Rope, row: usize) -> usize {
    let lines = text.len_lines(LineType::LF);
    if row >= lines { text.len() } else { text.line_to_byte_idx(row, LineType::LF) }
}

/// The end of the line's text, before its newline.
pub(super) fn line_end(text: &Rope, row: usize) -> usize {
    let end = line_full_end(text, row);
    if end > line_start(text, row) && text.byte(end - 1) == b'\n' { end - 1 } else { end }
}

/// The start of the next line (after the newline), or the end of the text.
pub(super) fn line_full_end(text: &Rope, row: usize) -> usize {
    line_start(text, row + 1)
}

pub(super) fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// The line's first character that isn't a space or a tab (its end when
/// there's none).
pub(super) fn first_non_blank(text: &Rope, row: usize) -> usize {
    let (start, end) = (line_start(text, row), line_end(text, row));
    let mut offset = start;
    for c in text.slice(start..end).chars() {
        if !is_blank(c) {
            break;
        }
        offset += c.len_utf8();
    }
    offset
}

/// The last character of the line, where normal mode's cursor stops (the
/// line's start when it's empty).
pub(super) fn last_char(text: &Rope, row: usize) -> usize {
    let (start, end) = (line_start(text, row), line_end(text, row));
    if end > start { prev(text, end) } else { start }
}

/// Keeps a normal-mode cursor on a character of its line, not on the
/// newline after it.
pub(super) fn clamp_normal(text: &Rope, offset: usize) -> usize {
    let offset = text.floor_char_boundary(offset.min(text.len()));
    let row = row(text, offset).min(last_row(text));
    offset.clamp(line_start(text, row), last_char(text, row))
}

/// The character column of `offset` in its line.
pub(super) fn column(text: &Rope, offset: usize) -> usize {
    let start = line_start(text, row(text, offset));
    text.slice(start..offset).chars().count()
}

/// The offset of character column `column` in `row`, or of the line's end
/// (`past_end`) or last character when the line is shorter.
pub(super) fn at_column(text: &Rope, row: usize, column: usize, past_end: bool) -> usize {
    let (start, end) = (line_start(text, row), line_end(text, row));
    let mut offset = start;
    for (i, c) in text.slice(start..end).chars().enumerate() {
        if i == column {
            return offset;
        }
        offset += c.len_utf8();
    }
    if past_end { end } else { last_char(text, row) }
}

pub(super) fn line_text(text: &Rope, row: usize) -> String {
    text.slice(line_start(text, row)..line_end(text, row)).to_string()
}

pub(super) fn indentation(text: &Rope, row: usize) -> String {
    text.slice(line_start(text, row)..first_non_blank(text, row)).to_string()
}

pub(super) fn is_blank_line(text: &Rope, row: usize) -> bool {
    first_non_blank(text, row) == line_end(text, row)
}

/// What a word motion treats as one word: letters, digits and `_`; runs of
/// other punctuation; or, for WORDs, anything but blanks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    Blank,
    Newline,
    Word,
    Punctuation,
}

pub(super) fn class(c: char, big: bool) -> Class {
    match c {
        '\n' => Class::Newline,
        c if c.is_whitespace() => Class::Blank,
        _ if big => Class::Word,
        c if c.is_alphanumeric() || c == '_' => Class::Word,
        _ => Class::Punctuation,
    }
}

/// Characters from `offset` on, with their offsets.
pub(super) fn forward(text: &Rope, offset: usize) -> impl Iterator<Item = (usize, char)> + '_ {
    text.char_indices_at(offset.min(text.len()))
}

/// Characters before `offset`, nearest first, with their offsets.
pub(super) fn backward(text: &Rope, offset: usize) -> impl Iterator<Item = (usize, char)> + '_ {
    let mut chars = text.char_indices_at(offset.min(text.len()));
    std::iter::from_fn(move || chars.prev())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_leave_out_the_empty_one_after_a_final_newline() {
        let text = Rope::from_str("ab\n\ncd\n");
        assert_eq!(last_row(&text), 2);
        assert_eq!(last_row(&Rope::from_str("ab")), 0);
        assert_eq!(last_row(&Rope::from_str("")), 0);
        assert_eq!((line_start(&text, 2), line_end(&text, 2), line_full_end(&text, 2)), (4, 6, 7));
        assert_eq!(line_end(&text, 1), 3);
        assert_eq!(last_char(&text, 0), 1);
        assert_eq!(last_char(&text, 1), 3);
    }

    #[test]
    fn a_normal_cursor_stays_off_the_newline() {
        let text = Rope::from_str("aé\n\nx");
        assert_eq!(clamp_normal(&text, 3), 1, "the newline after é: back on é");
        assert_eq!(clamp_normal(&text, 4), 4, "an empty line");
        assert_eq!(clamp_normal(&text, 99), 5);
        assert_eq!(at_column(&text, 0, 5, false), 1);
        assert_eq!(at_column(&text, 0, 5, true), 3);
        assert_eq!(column(&text, 3), 2);
    }

    #[test]
    fn iterates_both_ways() {
        let text = Rope::from_str("aéb");
        assert_eq!(forward(&text, 1).collect::<Vec<_>>(), vec![(1, 'é'), (3, 'b')]);
        assert_eq!(backward(&text, 3).collect::<Vec<_>>(), vec![(1, 'é'), (0, 'a')]);
        assert_eq!((next(&text, 1), prev(&text, 3)), (3, 1));
        assert_eq!(first_non_blank(&Rope::from_str("  \tx"), 0), 3);
    }
}

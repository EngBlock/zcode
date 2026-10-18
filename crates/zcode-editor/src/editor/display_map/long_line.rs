//! Long lines: x positions without shaping the line.
//!
//! Shaping a line costs time and memory in proportion to its length, and a
//! minified file is one line of megabytes. A line longer than
//! [`LONG_LINE_LEN`] bytes gets a [`LongLine`] instead: the x of a
//! checkpoint about every [`CHECKPOINT_LEN`] bytes, summed from the editor
//! font's width for each character (measured once per character, as gpui's
//! line wrapper does). The element shapes only the pieces on screen,
//! [`TILE_LEN`] bytes at a time with soft wrap off or the visible rows with
//! it on, and places them at these x positions; an offset anywhere else is
//! measured from the checkpoint before it. An edit shifts the checkpoints
//! after it rather than measuring the line again, and soft wrap re-wraps
//! from the edited row until its rows meet the old ones again.
//!
//! The sum of a character's widths is what CoreText lays out for a
//! monospaced font, ligatures and fallback fonts aside. A shaped tile
//! always starts at the summed x, so within a tile the glyphs are CoreText's
//! own and any difference only shows where tiles meet.

use std::{collections::HashMap, ops::Range, sync::Arc};

use ropey::Rope;
use sum_tree::Bias;

use crate::RopeExt as _;

/// Lines longer than this many bytes are laid out in pieces.
pub(crate) const LONG_LINE_LEN: usize = 4 * 1024;
/// About how far apart a long line's checkpoints are, in bytes.
const CHECKPOINT_LEN: usize = 1024;
/// The bytes in each separately shaped piece of a long line with soft wrap off.
pub(crate) const TILE_LEN: usize = 256;

/// How wide each character is in the editor font, learned as lines are
/// measured.
#[derive(Clone)]
pub(crate) struct CharWidths {
    /// NaN until measured.
    ascii: [f32; 128],
    other: HashMap<char, f32>,
    /// The width of every printable ASCII character, when they share one (a
    /// monospaced font), so a run of them is measured by its length.
    uniform: Option<f32>,
    ascii_known: bool,
}

impl Default for CharWidths {
    fn default() -> Self {
        Self {
            ascii: [f32::NAN; 128],
            other: HashMap::new(),
            uniform: None,
            ascii_known: false,
        }
    }
}

fn printable_ascii(text: &str) -> bool {
    text.bytes().all(|b| b.wrapping_sub(0x20) < 0x5f)
}

impl CharWidths {
    /// Measures the characters of `text` not measured yet.
    pub(crate) fn learn(&mut self, text: &str, measure: &mut dyn FnMut(char) -> f32) {
        if !self.ascii_known {
            self.ascii_known = true;
            for b in 0x20u8..0x7f {
                self.ascii[b as usize] = measure(b as char);
            }
            let first = self.ascii[0x20];
            self.uniform = self.ascii[0x20..0x7f]
                .iter()
                .all(|w| (w - first).abs() < 0.001)
                .then_some(first);
        }
        if printable_ascii(text) {
            return;
        }
        for c in text.chars() {
            if (c as u32) < 128 {
                if self.ascii[c as usize].is_nan() {
                    self.ascii[c as usize] = measure(c);
                }
            } else if !self.other.contains_key(&c) {
                self.other.insert(c, measure(c));
            }
        }
    }

    /// Whether every character of `text` has been measured.
    fn knows(&self, text: &str) -> bool {
        self.ascii_known
            && (printable_ascii(text)
                || text.chars().all(|c| {
                    if (c as u32) < 128 {
                        !self.ascii[c as usize].is_nan()
                    } else {
                        self.other.contains_key(&c)
                    }
                }))
    }

    /// The width of `c`; a character never measured counts as a space.
    pub(crate) fn get(&self, c: char) -> f32 {
        let width = if (c as u32) < 128 {
            self.ascii[c as usize]
        } else {
            self.other.get(&c).copied().unwrap_or(f32::NAN)
        };
        if width.is_nan() {
            self.ascii[b' ' as usize].max(0.)
        } else {
            width
        }
    }

    /// The width of `text`, the sum of its characters'.
    pub(crate) fn width(&self, text: &str) -> f32 {
        if let Some(width) = self.uniform
            && printable_ascii(text)
        {
            return text.len() as f32 * width;
        }
        text.chars().map(|c| self.get(c)).sum()
    }
}

/// Learns `text`'s characters into `widths`, copying the table only when
/// something is new (long lines measured earlier share the old copy).
fn learn(widths: &mut Arc<CharWidths>, text: &str, measure: &mut dyn FnMut(char) -> f32) {
    if !widths.knows(text) {
        Arc::make_mut(widths).learn(text, measure);
    }
}

/// The x positions of a long line's offsets.
pub(crate) struct LongLine {
    /// The document when the line was last measured, and the line's place in
    /// it. Later edits elsewhere don't change the line's text there.
    text: Rope,
    start: usize,
    len: usize,
    /// `(byte offset in the line, x)` at character boundaries, ascending, the
    /// first at `(0, 0.)`, at most [`CHECKPOINT_LEN`] bytes apart (plus a
    /// character).
    checkpoints: Vec<(usize, f64)>,
    widths: Arc<CharWidths>,
}

impl std::fmt::Debug for LongLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LongLine")
            .field("len", &self.len)
            .field("checkpoints", &self.checkpoints.len())
            .finish()
    }
}

impl LongLine {
    /// Measures the line `start..start + len` of `text`.
    pub(crate) fn new(
        text: &Rope,
        start: usize,
        len: usize,
        widths: &mut Arc<CharWidths>,
        measure: &mut dyn FnMut(char) -> f32,
    ) -> Self {
        let mut checkpoints = vec![(0, 0.)];
        measure_to(text, start, len, &mut checkpoints, widths, measure);
        Self {
            text: text.clone(),
            start,
            len,
            checkpoints,
            widths: widths.clone(),
        }
    }

    /// The same line after `old` (line-relative) was replaced by `new_len`
    /// bytes, now at `start..start + len` of `text`. Measures from the
    /// checkpoint before the edit to the first one after it; the rest move.
    pub(crate) fn edited(
        &self,
        text: &Rope,
        start: usize,
        len: usize,
        old: Range<usize>,
        new_len: usize,
        widths: &mut Arc<CharWidths>,
        measure: &mut dyn FnMut(char) -> f32,
    ) -> Self {
        let keep = self.checkpoints.partition_point(|&(at, _)| at <= old.start);
        let mut checkpoints = self.checkpoints[..keep].to_vec();
        let next = keep.max(self.checkpoints.partition_point(|&(at, _)| at < old.end));
        let new_end = old.start + new_len;
        match self.checkpoints.get(next) {
            Some(&(old_at, old_x)) => {
                let at = old_at - old.end + new_end;
                let x = measure_to(text, start, at, &mut checkpoints, widths, measure);
                if checkpoints.last().is_some_and(|&(last, _)| last < at) {
                    checkpoints.push((at, x));
                }
                let dx = x - old_x;
                checkpoints.extend(
                    self.checkpoints[next + 1..]
                        .iter()
                        .map(|&(old_at, old_x)| (old_at - old.end + new_end, old_x + dx)),
                );
            }
            None => {
                measure_to(text, start, len, &mut checkpoints, widths, measure);
            }
        }
        Self {
            text: text.clone(),
            start,
            len,
            checkpoints,
            widths: widths.clone(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn widths(&self) -> &CharWidths {
        &self.widths
    }

    /// The line's text in `range` (line-relative, character boundaries).
    pub(crate) fn slice(&self, range: Range<usize>) -> ropey::RopeSlice<'_> {
        self.text.slice(self.start + range.start..self.start + range.end)
    }

    /// `ix` moved back to a character boundary.
    pub(crate) fn floor_boundary(&self, ix: usize) -> usize {
        let ix = ix.min(self.len);
        self.text.clip_offset(self.start + ix, Bias::Left) - self.start
    }

    /// The x of line offset `ix`.
    pub(crate) fn x(&self, ix: usize) -> f64 {
        let ix = self.floor_boundary(ix);
        let i = self.checkpoints.partition_point(|&(at, _)| at <= ix) - 1;
        let (at, x) = self.checkpoints[i];
        x + self.measure(at..ix)
    }

    /// The width of the line.
    pub(crate) fn width(&self) -> f64 {
        self.x(self.len)
    }

    /// The width of `range` (line-relative, character boundaries), which the
    /// caller keeps short.
    pub(crate) fn measure(&self, range: Range<usize>) -> f64 {
        if range.is_empty() {
            return 0.;
        }
        self.slice(range)
            .chunks()
            .map(|chunk| self.widths.width(chunk) as f64)
            .sum()
    }

    /// The character boundary nearest to `x`, at or after `from` (a boundary)
    /// and measured from `from_x`, its x; `None` past `to`.
    fn index_from(&self, from: usize, from_x: f64, to: usize, x: f64) -> Option<usize> {
        let mut at = from;
        let mut cx = from_x;
        for chunk in self.slice(from..to).chunks() {
            for c in chunk.chars() {
                let width = self.widths.get(c) as f64;
                if x < cx + width / 2. {
                    return Some(at);
                }
                cx += width;
                at += c.len_utf8();
            }
        }
        (x < cx).then_some(to)
    }

    /// The character boundary nearest to `x`.
    pub(crate) fn index_for_x(&self, x: f64) -> usize {
        if x <= 0. {
            return 0;
        }
        let i = self.checkpoints.partition_point(|&(_, cx)| cx <= x) - 1;
        let (at, cx) = self.checkpoints[i];
        let to = self.checkpoints.get(i + 1).map_or(self.len, |&(at, _)| at);
        self.index_from(at, cx, to, x).unwrap_or(to)
    }

    /// The character boundary nearest to `x`, measured from `from` (a
    /// boundary) for a row starting there, going no further than `to`.
    pub(crate) fn index_for_row_x(&self, from: usize, to: usize, x: f64) -> usize {
        if x <= 0. {
            return from;
        }
        if to - from > 2 * CHECKPOINT_LEN {
            return self.index_for_x(self.x(from) + x).clamp(from, to);
        }
        self.index_from(from, 0., to, x).unwrap_or(to)
    }
}

/// Measures `text[start + from..start + to]`, where `from` is the last
/// checkpoint, adding a checkpoint whenever [`CHECKPOINT_LEN`] bytes have
/// passed since one; returns the x at `to`.
fn measure_to(
    text: &Rope,
    start: usize,
    to: usize,
    checkpoints: &mut Vec<(usize, f64)>,
    widths: &mut Arc<CharWidths>,
    measure: &mut dyn FnMut(char) -> f32,
) -> f64 {
    let (from, mut x) = *checkpoints.last().expect("the first checkpoint");
    let mut at = from;
    if to <= from {
        return x;
    }
    for chunk in text.slice(start + from..start + to).chunks() {
        learn(widths, chunk, measure);
        let mut chunk = chunk;
        while !chunk.is_empty() {
            let last = checkpoints.last().map_or(0, |&(at, _)| at);
            let room = (last + CHECKPOINT_LEN).saturating_sub(at);
            if room >= chunk.len() {
                x += widths.width(chunk) as f64;
                at += chunk.len();
                break;
            }
            let mut split = room.max(1);
            while !chunk.is_char_boundary(split) {
                split += 1;
            }
            let (head, tail) = chunk.split_at(split);
            x += widths.width(head) as f64;
            at += head.len();
            if at < to {
                checkpoints.push((at, x));
            }
            chunk = tail;
        }
    }
    x
}

/// Soft wrap of a long line by its characters' widths.
pub(crate) struct LongWrap<'a> {
    pub(crate) line: &'a LongLine,
    pub(crate) width: f32,
    /// Leading characters (spaces) continuation rows are indented by, and
    /// their width.
    pub(crate) indent: usize,
    pub(crate) indent_width: f32,
}

impl LongWrap<'_> {
    /// Where the row starting at `start` ends: the last line break
    /// opportunity that fits, or the last character that does when none
    /// does; `None` when the rest of the line fits.
    fn row_end(&self, start: usize) -> Option<usize> {
        let line = self.line;
        let available = if start == 0 {
            self.width
        } else {
            self.width - self.indent_width
        } as f64;
        let mut x = 0.;
        let mut at = start;
        let mut fit_end = None;
        'measure: for chunk in line.slice(start..line.len()).chunks() {
            for c in chunk.chars() {
                let width = line.widths().get(c) as f64;
                if x + width > available && at > start {
                    fit_end = Some(at);
                    break 'measure;
                }
                x += width;
                at += c.len_utf8();
            }
        }
        let fit_end = fit_end?;
        // Opportunities depend on the characters around them, so look a
        // little past the fitting text; the window's own end isn't one.
        let window_end = line.floor_boundary((fit_end + 32).min(line.len()));
        let window: std::borrow::Cow<str> = line.slice(start..window_end).into();
        let min = if start == 0 { self.indent } else { 0 };
        let mut best = None;
        for (ix, _) in unicode_linebreak::linebreaks(&window) {
            let at = start + ix;
            if at > fit_end || (ix == window.len() && window_end < line.len()) {
                break;
            }
            if ix > min {
                best = Some(at);
            }
        }
        Some(best.unwrap_or(fit_end))
    }

    /// Wraps from the row starting at `from`, pushing each later row's start
    /// onto `starts` until `stop` says to (it sees each start as pushed).
    fn wrap_from(&self, from: usize, starts: &mut Vec<usize>, mut stop: impl FnMut(usize) -> bool) {
        let mut start = from;
        while let Some(end) = self.row_end(start) {
            starts.push(end);
            if stop(end) {
                return;
            }
            start = end;
        }
    }

    /// Each row's byte range.
    pub(crate) fn rows(&self) -> Vec<Range<usize>> {
        let mut starts = vec![0];
        self.wrap_from(0, &mut starts, |_| false);
        rows_from_starts(&starts, self.line.len())
    }

    /// Each row's byte range after `old` (line-relative) was replaced by
    /// `new_len` bytes, given the rows before: re-wraps from the row before
    /// the edit until a row starts where one did before, past the edit (the
    /// same text from the same kind of row start wraps the same way).
    pub(crate) fn rewrap(
        &self,
        old_rows: &[Range<usize>],
        old: Range<usize>,
        new_len: usize,
    ) -> Vec<Range<usize>> {
        let old_starts: Vec<usize> = old_rows.iter().map(|row| row.start).collect();
        if old_starts.first() != Some(&0) {
            return self.rows();
        }
        // A row's break can depend on a little text past its end, so start
        // two rows before the edit.
        let row = old_starts.partition_point(|&s| s <= old.start) - 1;
        let from_row = row.saturating_sub(2);
        let mut starts = old_starts[..=from_row].to_vec();
        let new_end = old.start + new_len;
        let shift = |s: usize| s - old.end + new_end;
        let mut resumed = None;
        self.wrap_from(old_starts[from_row], &mut starts, |start| {
            if start < new_end {
                return false;
            }
            let old_start = start - new_end + old.end;
            match old_starts.binary_search(&old_start) {
                Ok(ix) => {
                    resumed = Some(ix);
                    true
                }
                Err(_) => false,
            }
        });
        if let Some(ix) = resumed {
            starts.extend(old_starts[ix + 1..].iter().map(|&s| shift(s)));
        }
        rows_from_starts(&starts, self.line.len())
    }
}

fn rows_from_starts(starts: &[usize], len: usize) -> Vec<Range<usize>> {
    starts
        .iter()
        .enumerate()
        .map(|(ix, &start)| start..starts.get(ix + 1).copied().unwrap_or(len))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Narrow characters are 1 wide, `W` 2, anything else 1.5.
    fn measure(c: char) -> f32 {
        match c {
            'W' => 2.,
            c if c.is_ascii() => 1.,
            _ => 1.5,
        }
    }

    fn naive_x(text: &str, ix: usize) -> f64 {
        text[..ix].chars().map(|c| measure(c) as f64).sum()
    }

    fn long_line(text: &Rope, start: usize, len: usize) -> LongLine {
        let mut widths = Arc::new(CharWidths::default());
        LongLine::new(text, start, len, &mut widths, &mut measure)
    }

    fn sample_line(len: usize) -> String {
        let words = ["alpha", "WW", "é", "be", "γράμμα", "x", "∑∫", "longerword"];
        let mut line = String::new();
        let mut ix = 0;
        while line.len() < len {
            line.push_str(words[ix % words.len()]);
            line.push(if ix % 7 == 0 { ',' } else { ' ' });
            ix += 1;
        }
        line
    }

    #[test]
    fn widths_sum_each_characters_width() {
        // `W` is wider, so ASCII is measured a character at a time.
        let mut widths = CharWidths::default();
        widths.learn("abc", &mut measure);
        assert_eq!(widths.uniform, None);
        assert_eq!(widths.width("abc dWf"), 8.);
        widths.learn("éW", &mut measure);
        assert_eq!(widths.width("aéW"), 1. + 1.5 + 2.);
        // A character never measured counts as a space.
        assert_eq!(widths.get('ж'), 1.);

        // A monospaced font's ASCII is measured by its length.
        let mut mono = CharWidths::default();
        mono.learn("x", &mut |_| 7.5);
        assert_eq!(mono.uniform, Some(7.5));
        assert_eq!(mono.width("var a=1;"), 60.);
    }

    #[test]
    fn x_matches_summing_every_character() {
        let line = sample_line(20_000);
        let text = Rope::from(format!("first\n{line}\nlast"));
        let long = long_line(&text, 6, line.len());
        assert!(long.checkpoints.len() > 15);
        for ix in (0..line.len()).filter(|&ix| line.is_char_boundary(ix)).step_by(97) {
            assert!((long.x(ix) - naive_x(&line, ix)).abs() < 0.01, "at {ix}");
        }
        assert!((long.width() - naive_x(&line, line.len())).abs() < 0.01);
        // Checkpoints are character boundaries no further apart than allowed.
        for pair in long.checkpoints.windows(2) {
            assert!(line.is_char_boundary(pair[1].0));
            assert!(pair[1].0 - pair[0].0 <= CHECKPOINT_LEN + 4);
        }
    }

    #[test]
    fn index_for_x_finds_the_nearest_boundary() {
        let line = sample_line(9_000);
        let text = Rope::from(line.clone());
        let long = long_line(&text, 0, line.len());
        for ix in (0..line.len()).filter(|&ix| line.is_char_boundary(ix)).step_by(131) {
            let x = naive_x(&line, ix);
            assert_eq!(long.index_for_x(x + 0.2), ix, "at {ix}");
            assert_eq!(long.index_for_row_x(0, line.len(), x + 0.2), ix);
        }
        assert_eq!(long.index_for_x(-5.), 0);
        assert_eq!(long.index_for_x(1e12), line.len());
        // Measured from a row's start, in a row.
        let from = line.char_indices().nth(500).unwrap().0;
        let target = line.char_indices().nth(520).unwrap().0;
        let x = naive_x(&line, target) - naive_x(&line, from);
        assert_eq!(long.index_for_row_x(from, line.len(), x), target);
    }

    #[test]
    fn edits_move_later_checkpoints_and_match_measuring_again() {
        let mut line = sample_line(30_000);
        let mut text = Rope::from(format!("head\n{line}\ntail"));
        let mut long = long_line(&text, 5, line.len());
        let mut widths = Arc::new(CharWidths::default());
        let edits: [(usize, usize, &str); 6] = [
            (10, 10, "inserted ∑ text"),
            (12_000, 12_004, ""),
            (500, 4_000, "W"),
            (29_000, 29_000, &"WWWW é ".repeat(400)),
            (0, 3, "X"),
            (2_048, 2_048, "\u{1F600}"),
        ];
        for (start, end, new) in edits {
            let start = line.floor_char_boundary(start);
            let end = line.floor_char_boundary(end);
            line.replace_range(start..end, new);
            text = Rope::from(format!("head\n{line}\ntail"));
            long = long.edited(&text, 5, line.len(), start..end, new.len(), &mut widths, &mut measure);
            let fresh = long_line(&text, 5, line.len());
            for ix in (0..=line.len()).filter(|&ix| line.is_char_boundary(ix)).step_by(89) {
                assert!((long.x(ix) - fresh.x(ix)).abs() < 0.01, "at {ix} after {start}..{end}");
            }
            for pair in long.checkpoints.windows(2) {
                assert!(pair[0].0 < pair[1].0);
                assert!(line.is_char_boundary(pair[1].0));
                assert!(pair[1].0 - pair[0].0 <= CHECKPOINT_LEN + 4);
            }
        }
    }

    fn wrap<'a>(long: &'a LongLine, width: f32) -> LongWrap<'a> {
        LongWrap { line: long, width, indent: 0, indent_width: 0. }
    }

    #[test]
    fn wrapping_breaks_at_opportunities_within_the_width() {
        let line = sample_line(10_000);
        let text = Rope::from(line.clone());
        let long = long_line(&text, 0, line.len());
        let rows = wrap(&long, 40.).rows();
        assert!(rows.len() > 200);
        assert_eq!(rows.first().unwrap().start, 0);
        assert_eq!(rows.last().unwrap().end, line.len());
        for pair in rows.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        for row in &rows[..rows.len() - 1] {
            let row_text = &line[row.clone()];
            assert!(naive_x(&line, row.end) - naive_x(&line, row.start) <= 40.0001, "{row_text:?}");
            // Rows end after a space or a comma, as the words allow.
            assert!(row_text.ends_with(' ') || row_text.ends_with(','), "{row_text:?}");
        }
        // A word wider than the row breaks between characters.
        let word = "W".repeat(5_000);
        let text = Rope::from(word.clone());
        let long = long_line(&text, 0, word.len());
        let rows = wrap(&long, 9.).rows();
        assert!(rows.iter().all(|row| row.len() == 4 || row.end == word.len()));
    }

    #[test]
    fn rewrapping_an_edit_matches_wrapping_again() {
        let mut line = sample_line(12_000);
        let mut text = Rope::from(line.clone());
        let mut long = long_line(&text, 0, line.len());
        let mut rows = wrap(&long, 37.).rows();
        let mut widths = Arc::new(CharWidths::default());
        let edits: [(usize, usize, &str); 6] = [
            (6_000, 6_000, "x"),
            (6_001, 6_001, " more words here "),
            (100, 900, ""),
            (11_000, 11_000, &"word ".repeat(300)),
            (0, 0, "WWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW"),
            (line.len() - 50, line.len() - 10, ""),
        ];
        for (start, end, new) in edits {
            let start = line.floor_char_boundary(start.min(line.len()));
            let end = line.floor_char_boundary(end.min(line.len()));
            line.replace_range(start..end, new);
            text = Rope::from(line.clone());
            long = long.edited(&text, 0, line.len(), start..end, new.len(), &mut widths, &mut measure);
            rows = wrap(&long, 37.).rewrap(&rows, start..end, new.len());
            assert_eq!(rows, wrap(&long, 37.).rows(), "after {start}..{end}");
        }
    }

    #[test]
    fn continuation_rows_keep_the_indent() {
        let line = format!("    {}", sample_line(6_000));
        let text = Rope::from(line.clone());
        let long = long_line(&text, 0, line.len());
        let rows = LongWrap { line: &long, width: 30., indent: 4, indent_width: 4. }.rows();
        for row in &rows[1..rows.len() - 1] {
            assert!(naive_x(&line, row.end) - naive_x(&line, row.start) <= 26.0001);
        }
        // The first row never breaks inside its indent.
        assert!(rows[0].end > 4);
    }
}

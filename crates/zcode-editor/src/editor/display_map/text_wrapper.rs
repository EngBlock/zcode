use super::inline_line::InputLine;
use super::long_line::{CharWidths, LONG_LINE_LEN, LongLine, LongWrap};
use gpui::Half;
use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Font, LineFragment, Pixels, Point, ShapedLine, Size, TextAlign, Window, point, px, size,
};
use ropey::Rope;
use smallvec::SmallVec;
use sum_tree::{Bias, Dimensions, SumTree};
use unicode_segmentation::UnicodeSegmentation as _;

use crate::{
    Point as TreeSitterPoint, RopeExt,
    layout::{LastLayout, WhitespaceIndicators},
};

/// Controls how soft-wrapped continuation lines are indented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrappingIndent {
    /// Continuation lines start flush-left at the full editor width.
    None,
    /// Continuation lines keep the same indentation as the first line.
    #[default]
    Same,
}

/// Choose Unicode line-break opportunities using the same shaped widths as
/// painting. Oversized words fall back to complete graphemes, never UTF-8 bytes.
fn measured_wrap_boundaries(
    text: &str,
    width: Pixels,
    wrapping_indent: WrappingIndent,
    mut measure: impl FnMut(&str) -> Pixels,
) -> Vec<gpui::Boundary> {
    let indent = if wrapping_indent == WrappingIndent::Same {
        text.chars()
            .take_while(|&c| c == ' ')
            .count()
            .min(gpui::LineWrapper::MAX_INDENT as usize)
    } else {
        0
    };
    let indent_width = measure(&text[..indent]);
    let ends: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(ix, grapheme)| ix + grapheme.len())
        .collect();
    let opportunities: Vec<usize> = unicode_linebreak::linebreaks(text)
        .map(|(ix, _)| ix)
        .filter(|ix| ends.binary_search(ix).is_ok())
        .collect();
    let mut result = Vec::new();
    let mut first = 0;
    let mut start = 0;
    while first < ends.len() {
        let available = if start == 0 {
            width
        } else {
            width - indent_width
        };
        // Find the fitting prefix locally. Exponential probing avoids shaping
        // the entire remaining logical line for every visual row of a long paste.
        let remaining = ends.len() - first;
        let mut low = 0;
        let mut high = 1;
        while measure(&text[start..ends[first + high - 1]]) <= available {
            low = high;
            if high == remaining {
                break;
            }
            high = (high * 2).min(remaining);
        }
        while low + 1 < high {
            let mid = (low + high) / 2;
            if measure(&text[start..ends[first + mid - 1]]) <= available {
                low = mid;
            } else {
                high = mid;
            }
        }
        // An indivisible grapheme wider than the viewport still consumes a row.
        let fitting_end = ends[first + low.max(1) - 1];
        if fitting_end == text.len() {
            break;
        }
        let candidate = opportunities.partition_point(|&ix| ix <= fitting_end);
        let end = candidate
            .checked_sub(1)
            .map(|ix| opportunities[ix])
            .filter(|&ix| ix > start && (start != 0 || ix > indent))
            .unwrap_or(fitting_end);
        result.push(gpui::Boundary {
            ix: end,
            next_indent: indent as u32,
        });
        first = ends.partition_point(|&ix| ix <= end);
        start = end;
    }
    result
}

/// A line with soft wrapped lines info.
#[derive(Debug, Clone)]
pub(crate) struct LineItem {
    /// The byte length of the line, without the end `\n`.
    len: usize,
    /// Number of leading characters of the line reserved as indentation for continuation wrapped
    /// lines, when [`WrappingIndent::Same`] is used.
    ///
    /// Zero when [`WrappingIndent::None`] is used or the line is not wrapped.
    pub(crate) indent: u32,
    /// The soft wrapped lines relative byte range (0..len) of this line (Include first line).
    ///
    /// Not contains the line end `\n`.
    pub(crate) wrapped_lines: SmallVec<[Range<usize>; 1]>,
    /// The x positions of a line over [`LONG_LINE_LEN`] bytes, which is laid
    /// out in pieces and wrapped by its characters' widths (`long_line.rs`).
    pub(crate) long: Option<Arc<LongLine>>,
}

impl LineItem {
    /// A line `len` bytes long that is one row on screen.
    fn plain(len: usize) -> Self {
        Self {
            len,
            indent: 0,
            wrapped_lines: smallvec::smallvec![0..len],
            long: None,
        }
    }

    /// Get the bytes length of this line.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Get number of soft wrapped lines of this line (include the first line).
    #[inline]
    pub(crate) fn lines_len(&self) -> usize {
        self.wrapped_lines.len()
    }

    /// Whether the line is one row on screen and laid out whole: all a
    /// [`LineRun::Plain`] needs to know of it is its length.
    fn is_plain(&self) -> bool {
        self.long.is_none() && self.indent == 0 && self.wrapped_lines.len() == 1
    }
}

/// At most this many plain lines make one [`LineRun::Plain`]: an edit
/// re-reads the lengths of the run's lines from the rope.
const PLAIN_RUN_LINES: u32 = 128;

/// What the wrap index keeps, so that it costs what the wrapped lines do: a
/// line that wraps (or is long) is kept whole, and consecutive lines that
/// are one row on screen each are one run that keeps only what the
/// summary needs. Their lengths are the rope's.
#[derive(Debug, Clone)]
pub(crate) enum LineRun {
    Line(LineItem),
    Plain(PlainLines),
}

/// Consecutive lines that are one row on screen each.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlainLines {
    lines: u32,
    /// Their byte lengths, summed, without the `\n`s.
    bytes: usize,
    /// The longest one's length, and its row in the run (the first one).
    max_len: u32,
    longest: u32,
}

/// Appends `item` to `runs`: to the last run if both are plain and it has
/// room, else on its own.
fn push_line(runs: &mut Vec<LineRun>, item: LineItem) {
    if !item.is_plain() {
        runs.push(LineRun::Line(item));
        return;
    }
    let len = item.len as u32;
    if let Some(LineRun::Plain(run)) = runs.last_mut()
        && run.lines < PLAIN_RUN_LINES
    {
        if len > run.max_len {
            run.max_len = len;
            run.longest = run.lines;
        }
        run.lines += 1;
        run.bytes += item.len;
        return;
    }
    runs.push(LineRun::Plain(PlainLines {
        lines: 1,
        bytes: item.len,
        max_len: len,
        longest: 0,
    }));
}

/// The index of `items`, in order.
fn line_runs(items: impl IntoIterator<Item = LineItem>) -> SumTree<LineRun> {
    let mut runs = Vec::new();
    for item in items {
        push_line(&mut runs, item);
    }
    SumTree::from_iter(runs, &())
}

/// The length of `row` in `text`, as a [`LineItem`]'s (without the `\n`).
fn line_len(text: &Rope, row: usize) -> usize {
    text.slice_line(row).len()
}

/// Summary of a subtree of [`LineRun`]s, maintained incrementally by the [`SumTree`].
#[derive(Debug, Clone)]
pub(crate) struct LineSummary {
    /// Number of buffer lines.
    buffer_rows: usize,
    /// Number of wrap rows (sum of each line's `lines_len()`).
    wrap_rows: usize,
    /// Sum of byte lengths of the buffer lines (without the trailing `\n`).
    bytes: usize,
    /// Byte length of the longest line in this subtree.
    max_line_len: usize,
    /// Buffer row (relative to this subtree) of the first line achieving `max_line_len`.
    longest_row: usize,
}

impl sum_tree::Summary for LineSummary {
    type Context<'a> = &'a ();

    fn zero(_: &()) -> Self {
        LineSummary {
            buffer_rows: 0,
            wrap_rows: 0,
            bytes: 0,
            max_line_len: 0,
            longest_row: 0,
        }
    }

    fn add_summary(&mut self, other: &Self, _: &()) {
        // Keep the leftmost row that achieves a strictly greater length
        if other.max_line_len > self.max_line_len {
            self.longest_row = self.buffer_rows + other.longest_row;
            self.max_line_len = other.max_line_len;
        }
        self.buffer_rows += other.buffer_rows;
        self.wrap_rows += other.wrap_rows;
        self.bytes += other.bytes;
    }
}

impl sum_tree::Item for LineRun {
    type Summary = LineSummary;

    fn summary(&self, _: &()) -> LineSummary {
        match self {
            LineRun::Line(item) => LineSummary {
                buffer_rows: 1,
                wrap_rows: item.lines_len(),
                bytes: item.len(),
                max_line_len: item.len(),
                longest_row: 0,
            },
            LineRun::Plain(run) => LineSummary {
                buffer_rows: run.lines as usize,
                wrap_rows: run.lines as usize,
                bytes: run.bytes,
                max_line_len: run.max_len as usize,
                longest_row: run.longest as usize,
            },
        }
    }
}

/// Cursor dimension counting buffer rows.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BufferRows(pub usize);

impl<'a> sum_tree::Dimension<'a, LineSummary> for BufferRows {
    fn zero(_: &()) -> Self {
        BufferRows(0)
    }

    fn add_summary(&mut self, summary: &'a LineSummary, _: &()) {
        self.0 += summary.buffer_rows;
    }
}

/// Cursor dimension counting wrap rows.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct WrapRows(pub usize);

impl<'a> sum_tree::Dimension<'a, LineSummary> for WrapRows {
    fn zero(_: &()) -> Self {
        WrapRows(0)
    }

    fn add_summary(&mut self, summary: &'a LineSummary, _: &()) {
        self.0 += summary.wrap_rows;
    }
}

/// Used to prepare the text with soft wrap to be get lines to displayed in the Editor.
///
/// After use lines to calculate the scroll size of the Editor.
pub(crate) struct TextWrapper {
    text: Rope,
    font: Font,
    font_size: Pixels,
    /// If is none, it means the text is not wrapped
    wrap_width: Option<Pixels>,
    wrapping_indent: WrappingIndent,
    /// The lines by split \n: runs of the ones not wrapped, the others whole.
    pub(crate) lines: SumTree<LineRun>,

    inline_metrics: Rc<[(Range<usize>, Pixels)]>,
    /// The font's character widths, for long lines.
    char_widths: Arc<CharWidths>,
    _initialized: bool,
}

#[allow(unused)]
impl TextWrapper {
    pub(crate) fn new(font: Font, font_size: Pixels, wrap_width: Option<Pixels>) -> Self {
        Self {
            text: Rope::new(),
            font,
            font_size,
            wrap_width,
            wrapping_indent: WrappingIndent::default(),
            lines: SumTree::new(&()),
            inline_metrics: Rc::from([]),
            char_widths: Arc::default(),
            _initialized: false,
        }
    }

    #[inline]
    pub(crate) fn set_default_text(&mut self, text: &Rope) {
        self.text = text.clone();
    }

    /// Get reference to the rope text.
    #[inline]
    pub(crate) fn text(&self) -> &Rope {
        &self.text
    }

    /// Get the total number of lines including wrapped lines.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.lines.summary().wrap_rows
    }

    /// Get the total number of buffer lines.
    #[inline]
    pub(crate) fn lines_count(&self) -> usize {
        self.lines.summary().buffer_rows
    }

    /// Get the 0-based row index of the longest line (by byte length).
    #[inline]
    pub(crate) fn longest_row(&self) -> usize {
        self.lines.summary().longest_row
    }

    /// Get the line item by buffer row index: borrowed when the line wraps
    /// or is long, else made from its length.
    #[inline]
    pub(crate) fn line(&self, row: usize) -> Option<Cow<'_, LineItem>> {
        let mut cursor = self.lines.cursor::<BufferRows>(&());
        cursor.seek(&BufferRows(row), Bias::Right);
        match cursor.item()? {
            LineRun::Line(item) => Some(Cow::Borrowed(item)),
            LineRun::Plain(_) => Some(Cow::Owned(LineItem::plain(line_len(&self.text, row)))),
        }
    }

    /// Iterate buffer lines in order.
    #[cfg(test)]
    pub(crate) fn iter_lines(&self) -> impl Iterator<Item = Cow<'_, LineItem>> {
        let mut row = 0;
        self.lines.iter().flat_map(move |run| {
            let start = row;
            let items: Vec<Cow<'_, LineItem>> = match run {
                LineRun::Line(item) => vec![Cow::Borrowed(item)],
                LineRun::Plain(run) => (start..start + run.lines as usize)
                    .map(|row| Cow::Owned(LineItem::plain(line_len(&self.text, row))))
                    .collect(),
            };
            row += items.len();
            items
        })
    }

    /// First wrap row of buffer line `row`. Returns the total wrap row count if `row` is
    /// out of range.
    pub(crate) fn buffer_line_to_first_wrap_row(&self, row: usize) -> usize {
        let mut cursor = self.lines.cursor::<Dimensions<BufferRows, WrapRows>>(&());
        cursor.seek(&BufferRows(row), Bias::Right);
        let Dimensions(BufferRows(first_row), WrapRows(first_wrap_row), _) = *cursor.start();
        match cursor.item() {
            // One row a line.
            Some(LineRun::Plain(_)) => first_wrap_row + (row - first_row),
            _ => first_wrap_row,
        }
    }

    /// Wrap row range of buffer line `row`.
    pub(crate) fn buffer_line_to_wrap_row_range(&self, row: usize) -> Range<usize> {
        let start = self.buffer_line_to_first_wrap_row(row);
        let mut cursor = self.lines.cursor::<BufferRows>(&());
        cursor.seek(&BufferRows(row), Bias::Right);
        let len = match cursor.item() {
            Some(LineRun::Line(item)) => item.lines_len(),
            Some(LineRun::Plain(_)) => 1,
            None => 0,
        };
        start..start + len
    }

    /// Buffer line containing wrap row `wrap_row`, clamped to the last line.
    pub(crate) fn wrap_row_to_buffer_line(&self, wrap_row: usize) -> usize {
        let mut cursor = self.lines.cursor::<Dimensions<WrapRows, BufferRows>>(&());
        cursor.seek(&WrapRows(wrap_row), Bias::Right);
        let Dimensions(WrapRows(first_wrap_row), BufferRows(first_row), _) = *cursor.start();
        match cursor.item() {
            Some(LineRun::Plain(_)) => first_row + (wrap_row - first_wrap_row),
            Some(LineRun::Line(_)) => first_row,
            None => self.lines_count().saturating_sub(1),
        }
    }

    pub(crate) fn set_wrap_width(&mut self, wrap_width: Option<Pixels>, cx: &mut App) {
        if wrap_width == self.wrap_width {
            return;
        }

        self.wrap_width = wrap_width;
        self.update_all(&self.text.clone(), cx);
    }

    pub(crate) fn set_wrapping_indent(&mut self, wrapping_indent: WrappingIndent, cx: &mut App) {
        if wrapping_indent == self.wrapping_indent {
            return;
        }

        self.wrapping_indent = wrapping_indent;
        self.update_all(&self.text.clone(), cx);
    }

    pub(crate) fn set_font(&mut self, font: Font, font_size: Pixels, cx: &mut App) {
        if self.font.eq(&font) && self.font_size == font_size {
            return;
        }

        self.font = font;
        self.font_size = font_size;
        self.char_widths = Arc::default();
        self.update_all(&self.text.clone(), cx);
    }

    pub(crate) fn prepare_if_need(&mut self, text: &Rope, cx: &mut App) -> bool {
        if self._initialized {
            return false;
        }
        self._initialized = true;
        self.update_all(text, cx);
        true
    }

    /// Update the text wrapper and recalculate the wrapped lines.
    ///
    /// If the `text` is the same as the current text, do nothing.
    ///
    /// - `changed_text`: The text [`Rope`] that has changed.
    /// - `range`: The `selected_range` before change.
    /// - `new_text`: The inserted text.
    /// - `force`: Whether to force the update, if false, the update will be skipped if the text is the same.
    /// - `cx`: The application context.
    pub(crate) fn update(
        &mut self,
        changed_text: &Rope,
        range: &Range<usize>,
        new_text: &Rope,
        cx: &mut App,
    ) {
        self.update_lines(changed_text, range, new_text, false, cx);
    }

    /// [`Self::update`]; `fresh` measures and wraps long lines from scratch
    /// instead of from their previous state (the wrap width or the inline
    /// metrics changed, not the text).
    fn update_lines(
        &mut self,
        changed_text: &Rope,
        range: &Range<usize>,
        new_text: &Rope,
        fresh: bool,
        cx: &mut App,
    ) {
        let font_id = cx.text_system().resolve_font(&self.font);
        let char_text_system = cx.text_system().clone();
        let char_font_size = self.font_size;
        let mut measure_char =
            move |c: char| char_text_system.layout_width(font_id, char_font_size, c).as_f32();
        let mut line_wrapper = cx
            .text_system()
            .line_wrapper(self.font.clone(), self.font_size);
        let metrics = self.inline_metrics.clone();
        let text_system = gpui::WindowTextSystem::new(cx.text_system().clone());
        let font = self.font.clone();
        let font_size = self.font_size;
        let wrapping_indent = self.wrapping_indent;
        self._update_measured(
            changed_text,
            range,
            new_text,
            fresh,
            &mut measure_char,
            &mut |line_str, wrap_width, line_start| {
                let mut fragments = Vec::new();
                let mut offset = 0;
                let first = metrics.partition_point(|(r, _)| r.end <= line_start);
                for (range, width) in &metrics[first..] {
                    if range.start >= line_start + line_str.len() {
                        break;
                    }
                    if range.start < line_start || range.end > line_start + line_str.len() {
                        continue;
                    }
                    let range = range.start - line_start..range.end - line_start;
                    if !line_str.is_char_boundary(range.start)
                        || !line_str.is_char_boundary(range.end)
                    {
                        continue;
                    }
                    if offset < range.start {
                        fragments.push(LineFragment::text(&line_str[offset..range.start]));
                    }
                    fragments.push(LineFragment::element(*width, range.len()));
                    offset = range.end;
                }
                if fragments.is_empty() {
                    return measured_wrap_boundaries(
                        line_str,
                        wrap_width,
                        wrapping_indent,
                        |text| {
                            text_system
                                .layout_line(
                                    text,
                                    font_size,
                                    &[gpui::TextRun {
                                        len: text.len(),
                                        font: font.clone(),
                                        color: gpui::black(),
                                        background_color: None,
                                        underline: None,
                                        strikethrough: None,
                                    }],
                                    None,
                                )
                                .width
                        },
                    );
                }
                if offset < line_str.len() {
                    fragments.push(LineFragment::text(&line_str[offset..]));
                }
                line_wrapper
                    .wrap_line(&fragments, wrap_width, gpui::IndentAdjustment::SameIndent)
                    .collect()
            },
        );
    }

    pub(crate) fn adjust_inline_metrics(&mut self, range: &Range<usize>, new_len: usize) {
        if self.inline_metrics.is_empty() {
            return;
        }
        let shift = new_len as isize - range.len() as isize;
        self.inline_metrics = self
            .inline_metrics
            .iter()
            .filter_map(|(token, width)| {
                if token.start < range.end && range.start < token.end {
                    return None;
                }
                let token = if token.start >= range.end {
                    token.start.checked_add_signed(shift)?..token.end.checked_add_signed(shift)?
                } else {
                    token.clone()
                };
                Some((token, *width))
            })
            .collect();
    }

    pub(crate) fn set_inline_metrics(
        &mut self,
        metrics: Rc<[(Range<usize>, Pixels)]>,
        cx: &mut App,
    ) {
        if self.inline_metrics == metrics {
            return;
        }
        // Only rows whose element geometry changed need another wrap pass.
        let mut affected = Vec::new();
        let (mut old, mut new) = (
            self.inline_metrics.iter().peekable(),
            metrics.iter().peekable(),
        );
        while old.peek().is_some() || new.peek().is_some() {
            match (old.peek(), new.peek()) {
                (Some(a), Some(b)) if a == b => {
                    old.next();
                    new.next();
                }
                (Some(a), Some(b)) if a.0.start <= b.0.start => {
                    affected.push(a.0.clone());
                    old.next();
                }
                (Some(_), Some(b)) | (None, Some(b)) => {
                    affected.push(b.0.clone());
                    new.next();
                }
                (Some(a), None) => {
                    affected.push(a.0.clone());
                    old.next();
                }
                (None, None) => break,
            }
        }
        self.inline_metrics = metrics;
        let text = self.text.clone();
        let mut rows: Vec<usize> = affected
            .iter()
            .map(|r| text.offset_to_point(r.start.min(text.len())).row)
            .collect();
        rows.sort_unstable();
        rows.dedup();
        for row in rows {
            let start = text.line_start_offset(row);
            let end = text.line_end_offset(row);
            // Long lines leave out their inlays (see `long_line.rs`).
            if end - start > LONG_LINE_LEN {
                continue;
            }
            self.update_lines(
                &text,
                &(start..end),
                &Rope::from(text.slice(start..end).to_string()),
                true,
                cx,
            );
        }
    }

    #[cfg(test)]
    fn _update<F>(
        &mut self,
        changed_text: &Rope,
        range: &Range<usize>,
        new_text: &Rope,
        wrap_line: &mut F,
    ) where
        F: FnMut(&str, Pixels, usize) -> Vec<gpui::Boundary>,
    {
        // Every character is 8 wide.
        self._update_measured(changed_text, range, new_text, false, &mut |_| 8., wrap_line);
    }

    fn _update_measured<F>(
        &mut self,
        changed_text: &Rope,
        range: &Range<usize>,
        new_text: &Rope,
        fresh: bool,
        measure_char: &mut dyn FnMut(char) -> f32,
        wrap_line: &mut F,
    ) where
        F: FnMut(&str, Pixels, usize) -> Vec<gpui::Boundary>,
    {
        // Remove the old changed lines.
        let buffer_line_count = self.lines_count();
        let start_row = self.text.offset_to_point(range.start).row;
        let start_row = start_row.min(buffer_line_count.saturating_sub(1));
        let end_row = self.text.offset_to_point(range.end).row;
        let end_row = end_row.min(buffer_line_count.saturating_sub(1));

        // To add the new lines.
        let new_start_row = changed_text.offset_to_point(range.start).row;
        let new_end_row = changed_text
            .offset_to_point(range.start + new_text.len())
            .row;

        let mut new_lines = Vec::new();
        let wrap_width = self.wrap_width;

        // An edit inside one long line carries its measurements and rows
        // over: the line, its old state, and the edit relative to it.
        let edited_long_line = (!fresh && start_row == end_row && new_start_row == new_end_row)
            .then(|| self.line(start_row))
            .flatten()
            .and_then(|item| Some((item.long.clone()?, item.wrapped_lines.clone(), item.indent)))
            .map(|(long, rows, indent)| {
                let old_start = self.text.line_start_offset(start_row);
                let old = range.start.saturating_sub(old_start)..range.end.saturating_sub(old_start);
                (long, rows, indent, old)
            });

        // line not contains `\n`.
        for row in new_start_row..=new_end_row {
            let line = changed_text.slice_line(row);
            if line.len() > LONG_LINE_LEN {
                new_lines.push(self.long_line_item(
                    changed_text,
                    row,
                    line.len(),
                    edited_long_line.as_ref().map(|(long, rows, indent, old)| {
                        (long.as_ref(), rows.as_slice(), *indent, old.clone(), new_text.len())
                    }),
                    measure_char,
                ));
                continue;
            }
            let mut wrapped_lines = SmallVec::<[Range<usize>; 1]>::new();
            let mut prev_boundary_ix = 0;
            let mut indent_chars = 0;

            // If wrap_width is Pixels::MAX, skip wrapping to disable word wrap
            if let Some(wrap_width) = wrap_width {
                // Borrowed for lines within a single rope chunk.
                let line_str: Cow<str> = line.into();
                match self.wrapping_indent {
                    WrappingIndent::Same => {
                        // Here only have wrapped line, if there is no wrap meet, the `line_wraps`
                        // result will empty.
                        for boundary in
                            wrap_line(&line_str, wrap_width, changed_text.line_start_offset(row))
                        {
                            wrapped_lines.push(prev_boundary_ix..boundary.ix);
                            prev_boundary_ix = boundary.ix;
                            indent_chars = boundary.next_indent;
                        }
                    }
                    WrappingIndent::None => {
                        // The first visual line keeps the line's leading indentation, so it is
                        // wrapped as is.
                        let boundaries =
                            wrap_line(&line_str, wrap_width, changed_text.line_start_offset(row));
                        if let Some(first_ix) = boundaries.first().map(|b| b.ix) {
                            wrapped_lines.push(prev_boundary_ix..first_ix);
                            prev_boundary_ix = first_ix;

                            for boundary in wrap_line(
                                &line_str[first_ix..],
                                wrap_width,
                                changed_text.line_start_offset(row) + first_ix,
                            ) {
                                let ix = first_ix + boundary.ix;
                                wrapped_lines.push(prev_boundary_ix..ix);
                                prev_boundary_ix = ix;
                            }
                        }
                    }
                }
            }

            // Reset of the line
            if prev_boundary_ix < line.len() || prev_boundary_ix == 0 {
                wrapped_lines.push(prev_boundary_ix..line.len());
            }

            new_lines.push(LineItem {
                len: line.len(),
                indent: indent_chars,
                wrapped_lines,
                long: None,
            });
        }

        if self.lines.is_empty() {
            self.lines = line_runs(new_lines);
        } else {
            let mut cursor = self.lines.cursor::<BufferRows>(&());
            let mut new_tree = cursor.slice(&BufferRows(start_row), Bias::Right);
            // A run the edit starts in keeps its lines before it, which the
            // edit didn't touch.
            let mut runs = Vec::new();
            for row in cursor.start().0..start_row {
                push_line(&mut runs, LineItem::plain(line_len(changed_text, row)));
            }
            for item in new_lines {
                push_line(&mut runs, item);
            }
            // Skip the replaced rows, and keep the lines after them of a run
            // they end in: they're the rows after the edit in the new text.
            cursor.seek_forward(&BufferRows(end_row + 1), Bias::Right);
            if let Some(LineRun::Plain(run)) = cursor.item()
                && cursor.start().0 < end_row + 1
            {
                let run_end = cursor.start().0 + run.lines as usize;
                let shift = new_end_row as isize - end_row as isize;
                for row in end_row + 1..run_end {
                    let row = row.checked_add_signed(shift).expect("a row of the new text");
                    push_line(&mut runs, LineItem::plain(line_len(changed_text, row)));
                }
                cursor.next();
            }
            new_tree.extend(runs, &());
            // Untouched rows after the edit
            new_tree.append(cursor.suffix(), &());
            drop(cursor);
            self.lines = new_tree;
        }

        self.text = changed_text.clone();
    }

    /// A long line's item: measured (moved on from `edited`, its previous
    /// state and the edit, when the edit stayed inside it) and wrapped by its
    /// characters' widths.
    fn long_line_item(
        &mut self,
        text: &Rope,
        row: usize,
        len: usize,
        edited: Option<(&LongLine, &[Range<usize>], u32, Range<usize>, usize)>,
        measure_char: &mut dyn FnMut(char) -> f32,
    ) -> LineItem {
        let start = text.line_start_offset(row);
        let long = match &edited {
            Some((old_line, _, _, old, new_len)) => old_line.edited(
                text,
                start,
                len,
                old.clone(),
                *new_len,
                &mut self.char_widths,
                measure_char,
            ),
            None => LongLine::new(text, start, len, &mut self.char_widths, measure_char),
        };
        let Some(width) = self.wrap_width else {
            return LineItem {
                len,
                indent: 0,
                wrapped_lines: smallvec::smallvec![0..len],
                long: Some(Arc::new(long)),
            };
        };
        let indent = match self.wrapping_indent {
            WrappingIndent::Same => long
                .slice(0..len.min(gpui::LineWrapper::MAX_INDENT as usize + 1))
                .chars()
                .take_while(|&c| c == ' ')
                .count()
                .min(gpui::LineWrapper::MAX_INDENT as usize),
            WrappingIndent::None => 0,
        };
        let wrap = LongWrap {
            line: &long,
            width: width.as_f32(),
            indent,
            indent_width: long.widths().get(' ') * indent as f32,
        };
        let rows = match edited {
            Some((_, old_rows, old_indent, old, new_len))
                if old_indent as usize == indent || old_rows.len() == 1 =>
            {
                wrap.rewrap(old_rows, old, new_len)
            }
            _ => wrap.rows(),
        };
        LineItem {
            len,
            indent: if rows.len() > 1 { indent as u32 } else { 0 },
            wrapped_lines: rows.into_iter().collect(),
            long: Some(Arc::new(long)),
        }
    }

    /// Update the text wrapper and recalculate the wrapped lines.
    ///
    /// If the `text` is the same as the current text, do nothing.
    fn update_all(&mut self, text: &Rope, cx: &mut App) {
        self.update_lines(text, &(0..text.len()), &text, true, cx);
    }

    /// Return display point (with soft wrap) from the given byte offset in the text.
    ///
    /// Panics if the `offset` is out of bounds.
    pub(crate) fn offset_to_display_point(&self, offset: usize) -> WrapDisplayPoint {
        self.offset_to_display_point_with_affinity(offset, false)
    }

    /// Like [`Self::offset_to_display_point`], but honours the caret's line-end affinity.
    ///
    /// A soft wrap boundary is one offset shared by two visual rows. Without affinity it always
    /// resolves to the start of the second row, which is wrong for a caret that is being drawn at
    /// the end of the first one -- vertical movement would then step from the row below the one
    /// the user can see.
    pub(crate) fn offset_to_display_point_with_affinity(
        &self,
        offset: usize,
        line_end_affinity: bool,
    ) -> WrapDisplayPoint {
        let row = self.text.offset_to_point(offset).row;
        let start = self.text.line_start_offset(row);

        // Seek to buffer row
        let mut cursor = self.lines.cursor::<Dimensions<BufferRows, WrapRows>>(&());
        cursor.seek(&BufferRows(row), Bias::Right);
        let Dimensions(BufferRows(first_row), WrapRows(wrapped_row), _) = *cursor.start();
        let local_offset = offset.saturating_sub(start);
        let line = match cursor.item() {
            None => return WrapDisplayPoint::new(wrapped_row, 0, 0),
            // One row, the whole line.
            Some(LineRun::Plain(_)) => {
                let column = local_offset.min(line_len(&self.text, row));
                return WrapDisplayPoint::new(wrapped_row + (row - first_row), 0, column);
            }
            Some(LineRun::Line(line)) => line,
        };

        // A long line's rows are many, contiguous and not empty: find the
        // row by its end.
        if line.long.is_some() {
            let rows = &line.wrapped_lines;
            let ix = if line_end_affinity {
                rows.partition_point(|range| range.end < local_offset)
            } else {
                rows.partition_point(|range| range.end <= local_offset)
            };
            if let Some(range) = rows.get(ix) {
                return WrapDisplayPoint::new(wrapped_row + ix, ix, local_offset.saturating_sub(range.start));
            }
        }
        for (ix, range) in line.wrapped_lines.iter().enumerate() {
            // With affinity the boundary offset closes the current row instead of opening the
            // next one, so the range is matched inclusively.
            let matches =
                range.contains(&local_offset) || (line_end_affinity && local_offset == range.end);
            if matches {
                return WrapDisplayPoint::new(
                    wrapped_row + ix,
                    ix,
                    local_offset.saturating_sub(range.start),
                );
            }
        }

        // Otherwise return the eof of the line.
        let last_range = line.wrapped_lines.last().unwrap_or(&(0..0));
        let ix = line.lines_len().saturating_sub(1);
        return WrapDisplayPoint::new(wrapped_row + ix, ix, last_range.len());
    }

    /// Return byte offset in the text from the given display point (with soft wrap).
    ///
    /// Panics if the `point.row` is out of bounds.
    pub(crate) fn display_point_to_offset(&self, point: WrapDisplayPoint) -> usize {
        // Seek to wrap row `point.row`
        let mut cursor = self.lines.cursor::<Dimensions<WrapRows, BufferRows>>(&());
        cursor.seek(&WrapRows(point.row), Bias::Right);
        let Dimensions(WrapRows(wrapped_row), BufferRows(row), _) = *cursor.start();
        let line = match cursor.item() {
            None => return self.text.len(),
            Some(LineRun::Plain(_)) => {
                let row = row + (point.row - wrapped_row);
                let len = line_len(&self.text, row);
                return self.text.line_start_offset(row) + point.column.min(len);
            }
            Some(LineRun::Line(line)) => line,
        };

        let line_start = self.text.line_start_offset(row);
        let local_row = point.row.saturating_sub(wrapped_row);
        if let Some(range) = line.wrapped_lines.get(local_row) {
            line_start + (range.start + point.column).min(range.end)
        } else {
            // If not found, return the end of the line.
            line_start + line.len()
        }
    }

    pub(crate) fn display_point_to_point(&self, point: WrapDisplayPoint) -> TreeSitterPoint {
        let offset = self.display_point_to_offset(point);
        self.text.offset_to_point(offset)
    }

    pub(crate) fn point_to_display_point(&self, point: TreeSitterPoint) -> WrapDisplayPoint {
        let offset = self.text.point_to_offset(point);
        self.offset_to_display_point(offset)
    }
}

/// A display point within the soft-wrapped text.
///
/// This represents a position in the text after soft-wrapping,
/// with an additional `local_row` field tracking the wrap line
/// within the original buffer line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WrapDisplayPoint {
    /// The 0-based soft wrapped row index in the text.
    pub row: usize,
    /// The 0-based row index in local line (include first line).
    ///
    /// This value only valid when return from [`TextWrapper::offset_to_display_point`], otherwise it will be ignored.
    pub local_row: usize,
    /// The 0-based column byte index in the display line (with soft wrap).
    pub column: usize,
}

impl WrapDisplayPoint {
    pub(crate) fn new(row: usize, local_row: usize, column: usize) -> Self {
        Self {
            row,
            local_row,
            column,
        }
    }
}

/// The layout info of a line with soft wrapped lines.
pub(crate) struct LineLayout {
    /// Total bytes length of this line.
    len: usize,
    /// The soft wrapped lines of this line (Include the first line).
    ///
    /// A long soft-wrapped line lays out only some of its rows, which
    /// [`Self::long_rows`] names.
    pub(crate) wrapped_lines: SmallVec<[InputLine; 1]>,
    /// Extra left offset applied to continuation wrapped lines, used to reserve the first line's
    /// indentation when [`WrappingIndent::Same`] is used.
    pub(crate) wrap_indent: Pixels,
    pub(crate) longest_width: Pixels,
    /// The layout's shaped space and tab marks, shared by its lines (two
    /// shaped lines are ~6 KB, twice what a line's own text costs).
    pub(crate) whitespace_indicators: Option<Rc<WhitespaceIndicators>>,
    /// Whitespace indicators: (row, x_position, is_tab)
    pub(crate) whitespace_chars: Vec<(usize, Pixels, bool)>,
    /// Whether any run of this line carries a background color, so [`Self::paint_background`]
    /// can skip the glyph walk for the common case of a line without highlights.
    has_background: bool,
    /// The height of the blocks below this line (`annotations.rs`), which
    /// the line's size includes so that the lines after it move down.
    pub(crate) extra_height: Pixels,
    /// For a long soft-wrapped line (`long_line.rs`): every row, and which
    /// of them `wrapped_lines` holds; the others are measured, not shaped.
    long_rows: Option<LongRows>,
}

/// The rows of a long soft-wrapped line.
pub(crate) struct LongRows {
    long: Arc<LongLine>,
    /// Every row's byte range in the line.
    ranges: SmallVec<[Range<usize>; 1]>,
    /// The row each of the layout's `wrapped_lines` is, ascending.
    laid_out: Vec<usize>,
}

/// One visual row of a [`LineLayout`]: shaped, or (a long line's row off
/// screen) measured from the line's characters.
enum Row<'a> {
    Shaped(&'a InputLine),
    Measured(&'a LongLine, Range<usize>),
}

impl Row<'_> {
    fn len(&self) -> usize {
        match self {
            Row::Shaped(line) => line.len,
            Row::Measured(_, range) => range.len(),
        }
    }

    fn width(&self) -> Pixels {
        match self {
            Row::Shaped(line) => line.width,
            Row::Measured(long, range) => px(long.measure(range.clone()) as f32),
        }
    }

    fn x_for_index(&self, ix: usize) -> Pixels {
        match self {
            Row::Shaped(line) => line.x_for_index(ix),
            Row::Measured(long, range) => {
                let end = long.floor_boundary(range.start + ix.min(range.len()));
                px(long.measure(range.start..end) as f32)
            }
        }
    }

    fn closest_index_for_x(&self, x: Pixels) -> usize {
        match self {
            Row::Shaped(line) => line.closest_index_for_x(x),
            Row::Measured(long, range) => {
                long.index_for_row_x(range.start, range.end, x.as_f32() as f64) - range.start
            }
        }
    }

    fn index_for_x(&self, x: Pixels) -> Option<usize> {
        match self {
            Row::Shaped(line) => line.index_for_x(x),
            Row::Measured(..) => (x >= px(0.) && x <= self.width()).then(|| self.closest_index_for_x(x)),
        }
    }
}

impl LineLayout {
    pub(crate) fn new() -> Self {
        Self {
            len: 0,
            longest_width: px(0.),
            wrapped_lines: SmallVec::new(),
            wrap_indent: px(0.),
            whitespace_chars: Vec::new(),
            whitespace_indicators: None,
            has_background: false,
            extra_height: px(0.),
            long_rows: None,
        }
    }

    /// Reserves `height` below the line, for blocks.
    pub(crate) fn extra_height(mut self, height: Pixels) -> Self {
        self.extra_height = height;
        self
    }

    /// The height of the line's own rows, without the blocks below it.
    pub(crate) fn text_height(&self, line_height: Pixels) -> Pixels {
        self.row_count() * line_height
    }

    /// The number of visual rows, laid out or not.
    pub(crate) fn row_count(&self) -> usize {
        self.long_rows
            .as_ref()
            .map_or(self.wrapped_lines.len(), |rows| rows.ranges.len())
    }

    /// Whether this is a long line laid out in pieces (`long_line.rs`).
    pub(crate) fn is_long(&self) -> bool {
        self.long_rows.is_some()
            || self
                .wrapped_lines
                .first()
                .is_some_and(|line| line.tile_ranges().is_some())
    }

    /// A long line's laid-out bytes, line-relative and ascending: its tiles,
    /// or its shaped rows. `None` for any other line, all of which is laid out.
    pub(crate) fn laid_out_ranges(&self) -> Option<Vec<Range<usize>>> {
        if let Some(rows) = &self.long_rows {
            return Some(rows.laid_out.iter().map(|&row| rows.ranges[row].clone()).collect());
        }
        Some(self.wrapped_lines.first()?.tile_ranges()?.collect())
    }

    /// The rows that `range` (line-relative) reaches, including the row a
    /// range ending at its start ends on.
    pub(crate) fn rows_reached(&self, range: Range<usize>) -> Range<usize> {
        match &self.long_rows {
            None => 0..self.wrapped_lines.len(),
            Some(rows) => {
                let first = rows.ranges.partition_point(|row| row.end < range.start);
                let end = rows.ranges.partition_point(|row| row.start <= range.end);
                first..end.max(first)
            }
        }
    }

    /// Where row `ix` starts in the line, its length and its width.
    pub(crate) fn row_span(&self, ix: usize) -> Option<(usize, usize, Pixels)> {
        let row = self.row(ix)?;
        let start = match &self.long_rows {
            Some(rows) => rows.ranges[ix].start,
            None => self.wrapped_lines[..ix].iter().map(|line| line.len).sum(),
        };
        Some((start, row.len(), row.width()))
    }

    /// The x of `ix` in row `row`, relative to the row's start.
    pub(crate) fn row_x(&self, row: usize, ix: usize) -> Pixels {
        self.row(row).map_or(px(0.), |line| line.x_for_index(ix))
    }

    fn row(&self, ix: usize) -> Option<Row<'_>> {
        match &self.long_rows {
            None => self.wrapped_lines.get(ix).map(Row::Shaped),
            Some(rows) => {
                let range = rows.ranges.get(ix)?;
                Some(match rows.laid_out.binary_search(&ix) {
                    Ok(laid) => Row::Shaped(&self.wrapped_lines[laid]),
                    Err(_) => Row::Measured(&rows.long, range.clone()),
                })
            }
        }
    }

    /// The row each of `wrapped_lines` is.
    fn laid_out_row(&self, ix: usize) -> usize {
        self.long_rows.as_ref().map_or(ix, |rows| rows.laid_out[ix])
    }

    /// Record whether any run of this line carries a background color.
    pub(crate) fn with_background(mut self, has_background: bool) -> Self {
        self.has_background = has_background;
        self
    }

    /// Set the left offset reserved for continuation wrapped lines.
    pub(crate) fn wrap_indent(mut self, wrap_indent: Pixels) -> Self {
        self.wrap_indent = wrap_indent;
        self
    }

    /// The pixel indent applied to the given visual line, relative to the line's
    /// leading text. Only continuation lines (index > 0) are indented.
    #[inline]
    fn line_indent(&self, line_index: usize) -> Pixels {
        if line_index == 0 {
            px(0.)
        } else {
            self.wrap_indent
        }
    }

    pub(crate) fn lines(mut self, wrapped_lines: SmallVec<[ShapedLine; 1]>) -> Self {
        self.set_wrapped_lines(wrapped_lines);
        self
    }

    pub(crate) fn set_wrapped_lines(&mut self, wrapped_lines: SmallVec<[ShapedLine; 1]>) {
        self.len = wrapped_lines.iter().map(|l| l.len).sum();
        let width = wrapped_lines
            .iter()
            .map(|l| l.width)
            .max()
            .unwrap_or_default();
        self.longest_width = width;
        self.wrapped_lines = wrapped_lines.into_iter().map(InputLine::from).collect();
    }

    pub(crate) fn inline_lines(mut self, lines: SmallVec<[InputLine; 1]>) -> Self {
        self.len = lines.iter().map(|line| line.len).sum();
        self.longest_width = lines
            .iter()
            .map(|line| line.width)
            .max()
            .unwrap_or_default();
        self.wrapped_lines = lines;
        self
    }

    /// A long soft-wrapped line with the rows `laid_out` (ascending) shaped
    /// as `lines`, out of rows `ranges`.
    pub(crate) fn long_rows(
        mut self,
        long: Arc<LongLine>,
        ranges: SmallVec<[Range<usize>; 1]>,
        laid_out: Vec<usize>,
        lines: SmallVec<[InputLine; 1]>,
    ) -> Self {
        debug_assert_eq!(laid_out.len(), lines.len());
        self = self.inline_lines(lines);
        self.len = long.len();
        self.long_rows = Some(LongRows { long, ranges, laid_out });
        self
    }

    pub(crate) fn with_whitespaces(mut self, indicators: Option<Rc<WhitespaceIndicators>>) -> Self {
        self.whitespace_indicators = indicators;
        let Some(indicators) = self.whitespace_indicators.as_ref() else {
            return self;
        };

        let space_indicator_offset = indicators.space.width.half();

        for (line_index, wrapped_line) in self.wrapped_lines.iter().enumerate() {
            let row = self.laid_out_row(line_index);
            for (piece_start, text) in wrapped_line.shaped_text() {
                for (relative_offset, c) in text.char_indices() {
                    if matches!(c, ' ' | '\t') {
                        let is_tab = c == '\t';
                        let offset = piece_start + relative_offset;
                        let start_x = wrapped_line.x_for_index(offset);
                        let end_x = wrapped_line.x_for_index(offset + c.len_utf8());
                        // Center the indicator in the actual character's space
                        let x_position = if c == ' ' {
                            (start_x + end_x).half() - space_indicator_offset
                        } else {
                            start_x
                        };

                        self.whitespace_chars.push((row, x_position, is_tab));
                    }
                }
            }
        }
        self
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Get the position (x, y) for the given index in this line layout.
    ///
    /// - The `offset` is a local byte index in this line layout.
    /// - When `line_end_affinity` is true, an offset at a soft wrap boundary is placed at
    ///   the end of the current visual line rather than the start of the next one.
    /// - The return value is relative to the top-left corner of this line layout, start from (0, 0)
    pub(crate) fn position_for_index(
        &self,
        offset: usize,
        last_layout: &LastLayout,
        line_end_affinity: bool,
    ) -> Option<Point<Pixels>> {
        let x_offset = last_layout.alignment_offset(self.longest_width);

        if let Some(rows) = &self.long_rows {
            // Rows are contiguous and not empty; the boundary offset belongs
            // to the next row unless the caret keeps to the line's end.
            let ix = if line_end_affinity {
                rows.ranges.partition_point(|range| range.end < offset)
            } else {
                rows.ranges.partition_point(|range| range.end <= offset)
            };
            let ix = ix.min(rows.ranges.len().checked_sub(1)?);
            let range = &rows.ranges[ix];
            if offset < range.start || offset > range.end {
                return None;
            }
            let x = self.row(ix)?.x_for_index(offset - range.start) + x_offset + self.line_indent(ix);
            return Some(point(x, ix * last_layout.line_height));
        }

        let mut acc_len = 0;
        let mut offset_y = px(0.);

        for (i, line) in self.wrapped_lines.iter().enumerate() {
            let is_last = i + 1 == self.wrapped_lines.len();

            let matches = if line.len == 0 {
                // Empty visual lines still own their boundary offset.
                offset == acc_len
            } else if is_last || line_end_affinity {
                // Inclusive: cursor can sit at end of this visual line.
                offset >= acc_len && offset <= acc_len + line.len
            } else {
                // Exclusive: boundary offset belongs to the next visual line.
                offset >= acc_len && offset < acc_len + line.len
            };

            if matches {
                let x = line.x_for_index(offset.saturating_sub(acc_len))
                    + x_offset
                    + self.line_indent(i);
                return Some(point(x, offset_y));
            }

            // Always advance by actual line length. The last line gets +1 so the
            // cursor can be placed after the final character.
            acc_len += if is_last { line.len + 1 } else { line.len };
            offset_y += last_layout.line_height;
        }

        None
    }

    /// Get the closest index for the given x in this line layout.
    ///
    /// This ignores y, so it only makes sense for a layout that is known to occupy a single
    /// visual line. Wrapped layouts must use [`Self::closest_index_for_position`], which also
    /// reports the caret affinity that a wrap boundary needs.
    pub(crate) fn closest_index_for_x(&self, x: Pixels, last_layout: &LastLayout) -> usize {
        let mut acc_len = 0;
        let x_offset = last_layout.alignment_offset(self.longest_width);
        let x = x - x_offset;

        if self.long_rows.is_some() {
            return self.row(0).map_or(0, |row| row.closest_index_for_x(x));
        }

        for (i, line) in self.wrapped_lines.iter().enumerate() {
            let line_indent = self.line_indent(i);
            if x <= line_indent + line.width {
                return acc_len + line.closest_index_for_x(x - line_indent);
            }
            acc_len += line.len;
        }

        acc_len
    }

    /// Resolve `pos` to the wrapped sub-line under it.
    ///
    /// Returns the sub-line index, the byte offset that sub-line starts at within this line
    /// layout, and `pos.x` translated into that sub-line's own coordinate space.
    fn wrapped_line_at(
        &self,
        pos: Point<Pixels>,
        last_layout: &LastLayout,
    ) -> Option<(usize, usize, Pixels)> {
        let x_offset = last_layout.alignment_offset(self.longest_width);

        if let Some(rows) = &self.long_rows {
            if pos.y < px(0.) || last_layout.line_height <= px(0.) {
                return None;
            }
            let ix = (pos.y / last_layout.line_height).floor() as usize;
            let range = rows.ranges.get(ix)?;
            return Some((ix, range.start, pos.x - x_offset - self.line_indent(ix)));
        }

        let mut offset = 0;
        let mut line_top = px(0.);

        for (i, line) in self.wrapped_lines.iter().enumerate() {
            let line_bottom = line_top + last_layout.line_height;
            if pos.y >= line_top && pos.y < line_bottom {
                return Some((i, offset, pos.x - x_offset - self.line_indent(i)));
            }

            offset += line.len;
            line_top = line_bottom;
        }

        None
    }

    /// Get the index for the given position (x, y) in this line layout.
    ///
    /// The `pos` is relative to the top-left corner of this line layout, start from (0, 0).
    ///
    /// Returns a local byte index in this line layout (start from 0) together with the caret
    /// affinity to use for it: `true` when the index landed on the wrap boundary of a non-final
    /// sub-line. That boundary offset is shared by the end of one visual line and the start of
    /// the next, so the affinity is what tells [`Self::position_for_index`] which of the two the
    /// caret belongs to. Without it a click past the last glyph of a wrapped line would put a
    /// visible caret on the following line.
    pub(crate) fn closest_index_for_position(
        &self,
        pos: Point<Pixels>,
        last_layout: &LastLayout,
    ) -> Option<(usize, bool)> {
        let (i, offset, x) = self.wrapped_line_at(pos, last_layout)?;
        let line = self.row(i)?;
        let ix = line.closest_index_for_x(x);
        let line_end_affinity = i + 1 < self.row_count() && ix == line.len();

        Some((offset + ix, line_end_affinity))
    }

    /// How many columns the given position sits past the end of the line under it.
    ///
    /// Past the end of a line there is no glyph to hit-test against, so a position out
    /// there resolves to the line end and loses how far right it really was. The extra
    /// distance is reported here in whole spaces, letting a columnar selection keep its
    /// width over a short row. Only the final visual line of a wrapped layout has that
    /// trailing space; a continuation line ends at a wrap boundary, where the next glyph
    /// merely lives on the following row.
    ///
    /// The `pos` is relative to the top-left corner of this line layout, start from (0, 0).
    pub(crate) fn columns_past_line_end(
        &self,
        pos: Point<Pixels>,
        last_layout: &LastLayout,
    ) -> usize {
        let Some((i, _, x)) = self.wrapped_line_at(pos, last_layout) else {
            return 0;
        };

        if i + 1 < self.row_count() || last_layout.space_width <= px(0.) {
            return 0;
        }

        let past_end = x - self.row(i).map_or(px(0.), |row| row.width());
        if past_end <= px(0.) {
            return 0;
        }

        (past_end / last_layout.space_width).round() as usize
    }

    pub(crate) fn index_for_position(
        &self,
        pos: Point<Pixels>,
        last_layout: &LastLayout,
    ) -> Option<usize> {
        let (i, offset, x) = self.wrapped_line_at(pos, last_layout)?;

        Some(offset + self.row(i)?.index_for_x(x)?)
    }

    pub(crate) fn size(&self, line_height: Pixels) -> Size<Pixels> {
        let width = self
            .wrapped_lines
            .iter()
            .enumerate()
            .map(|(ix, line)| line.width + self.line_indent(self.laid_out_row(ix)))
            .max()
            .unwrap_or(self.longest_width);
        size(width, self.row_count() * line_height + self.extra_height)
    }

    /// Paint only the glyph background quads of this line.
    ///
    /// gpui's [`ShapedLine::paint`] does not draw backgrounds, so every line painted with
    /// [`Self::paint`] needs this called first, with the same origin and align width.
    pub(crate) fn paint_background(
        &self,
        pos: Point<Pixels>,
        line_height: Pixels,
        text_align: TextAlign,
        align_width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Painting a background walks every glyph and pushes a scene layer, so skip the
        // whole pass for lines that have no background color to paint.
        if !self.has_background {
            return;
        }

        for (ix, line) in self.wrapped_lines.iter().enumerate() {
            let row = self.laid_out_row(ix);
            _ = line.paint_background(
                pos + point(self.line_indent(row), row * line_height),
                line_height,
                text_align,
                align_width,
                window,
                cx,
            );
        }
    }

    pub(crate) fn paint(
        &self,
        pos: Point<Pixels>,
        line_height: Pixels,
        text_align: TextAlign,
        align_width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        for (ix, line) in self.wrapped_lines.iter().enumerate() {
            let row = self.laid_out_row(ix);
            _ = line.paint(
                pos + point(self.line_indent(row), row * line_height),
                line_height,
                text_align,
                align_width,
                window,
                cx,
            );
        }

        // Paint whitespace indicators
        if let Some(indicators) = self.whitespace_indicators.as_ref() {
            for (line_index, x_position, is_tab) in &self.whitespace_chars {
                let invisible = if *is_tab {
                    indicators.tab.clone()
                } else {
                    indicators.space.clone()
                };

                let origin = point(
                    pos.x + *x_position + self.line_indent(*line_index),
                    pos.y + *line_index as f32 * line_height,
                );

                _ = invisible.paint(origin, line_height, text_align, align_width, window, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[cfg(target_os = "linux")]
    use gpui::TestAppContext;
    use gpui::{Boundary, Bounds, FontFeatures, FontStyle, FontWeight, px};

    #[cfg(target_os = "linux")]
    fn shaped_width(text: &str, font: &Font, font_size: Pixels, cx: &App) -> Pixels {
        gpui::WindowTextSystem::new(cx.text_system().clone())
            .layout_line(
                text,
                font_size,
                &[gpui::TextRun {
                    len: text.len(),
                    font: font.clone(),
                    color: gpui::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
            .width
    }

    // Linux exposes its native text engine without creating a desktop window.
    // macOS platform creation requires the main thread, and Windows headless
    // mode uses NoopTextSystem rather than native shaping.
    #[cfg(target_os = "linux")]
    fn shaping_test_context() -> TestAppContext {
        let platform = gpui_platform::current_platform(true);
        TestAppContext::build_with_text_system(
            gpui::TestDispatcher::new(0),
            None,
            platform.text_system(),
        )
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn production_wrap_keeps_shaped_cjk_latin_boundary_during_edits() {
        let cx = shaping_test_context();
        cx.update(|cx| {
            let font = test_font();
            let font_size = px(14.);
            let prefix = "abcd的";
            let width = shaped_width(prefix, &font, font_size, cx);
            assert!(shaped_width("abcd的s", &font, font_size, cx) > width);
            let mut wrapper = TextWrapper::new(font.clone(), font_size, Some(width));
            let mut previous = Rope::new();

            for value in ["abcd的", "abcd的s", "abcd的ss", "abcd的s", "abcd的"] {
                let text = Rope::from(value);
                let start = previous.len().min(text.len());
                let inserted = Rope::from(text.slice(start..).to_string());
                wrapper.update(&text, &(start..previous.len()), &inserted, cx);
                let expected = if value.ends_with('s') {
                    vec![0..prefix.len(), prefix.len()..value.len()]
                } else {
                    vec![0..prefix.len()]
                };
                assert_eq!(wrapper.line(0).unwrap().wrapped_lines.as_slice(), expected);
                for range in &wrapper.line(0).unwrap().wrapped_lines {
                    assert!(shaped_width(&value[range.clone()], &font, font_size, cx) <= width);
                }
                assert_eq!(
                    wrapper.offset_to_display_point(value.len()).row,
                    usize::from(value.ends_with('s'))
                );
                previous = text;
            }
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn production_wrap_preserves_words_and_complete_graphemes() {
        let cx = shaping_test_context();
        cx.update(|cx| {
            let font = test_font();
            let font_size = px(14.);
            for (value, prefix) in [("hello world", "hello "), ("a👩‍💻b", "a👩‍💻")] {
                // Either complete row must fit even when the native font makes
                // the second word wider than the first word and its space.
                let width = shaped_width(prefix, &font, font_size, cx).max(shaped_width(
                    &value[prefix.len()..],
                    &font,
                    font_size,
                    cx,
                ));
                assert!(shaped_width(value, &font, font_size, cx) > width);
                let text = Rope::from(value);
                let mut wrapper = TextWrapper::new(font.clone(), font_size, Some(width));
                wrapper.update(&text, &(0..0), &text, cx);
                assert_eq!(
                    wrapper.line(0).unwrap().wrapped_lines.as_slice(),
                    [0..prefix.len(), prefix.len()..value.len()]
                );
                for range in &wrapper.line(0).unwrap().wrapped_lines {
                    assert!(shaped_width(&value[range.clone()], &font, font_size, cx) <= width);
                }
            }
        });
    }

    #[test]
    fn measured_wrap_keeps_cjk_latin_boundary_stable_during_edits() {
        let measure = |text: &str| {
            px(text
                .chars()
                .map(|c| if c.is_ascii() { 1. } else { 2. })
                .sum())
        };
        let mut wrapper = TextWrapper::new(gpui::font("Arial"), px(14.), Some(px(6.)));
        let mut previous = Rope::new();
        for value in ["abcd的", "abcd的s", "abcd的ss", "abcd的s", "abcd的"] {
            let text = Rope::from(value);
            let start = previous.len().min(text.len());
            let inserted = Rope::from(text.slice(start..).to_string());
            wrapper._update(
                &text,
                &(start..previous.len()),
                &inserted,
                &mut |line, width, _| {
                    measured_wrap_boundaries(line, width, WrappingIndent::None, measure)
                },
            );
            let expected = if value.ends_with('s') {
                vec![0..7, 7..value.len()]
            } else {
                vec![0..7]
            };
            assert_eq!(wrapper.line(0).unwrap().wrapped_lines.as_slice(), expected);
            let cursor = wrapper.offset_to_display_point(value.len());
            assert_eq!(cursor.row, usize::from(value.ends_with('s')));
            previous = text;
        }
    }

    #[test]
    fn measured_wrap_preserves_words_graphemes_and_indentation() {
        let wrap = |text: &str, width, indent| {
            measured_wrap_boundaries(text, px(width), indent, |s| {
                px(s.graphemes(true).count() as f32)
            })
            .into_iter()
            .map(|b| b.ix)
            .collect::<Vec<_>>()
        };
        assert_eq!(wrap("hello world", 8., WrappingIndent::None), vec![6]);
        assert_eq!(wrap("a👩‍💻b", 2., WrappingIndent::None), vec!["a👩‍💻".len()]);
        assert_eq!(wrap("  abcdefgh", 5., WrappingIndent::Same), vec![5, 8]);
        assert_eq!(wrap("  abcdefgh", 5., WrappingIndent::None), vec![2, 7]);
        assert!(wrap("", 0., WrappingIndent::None).is_empty());
        assert_eq!(wrap("abc", 0., WrappingIndent::None), vec![1, 2]);
        // Closing punctuation stays with the preceding Chinese character.
        assert_eq!(wrap("你好，世界", 2., WrappingIndent::None), vec![3, 9]);
    }

    #[test]
    fn test_update() {
        let font = gpui::Font {
            family: "Arial".into(),
            weight: FontWeight::default(),
            style: FontStyle::Normal,
            features: FontFeatures::default(),
            fallbacks: None,
        };

        let mut wrapper = TextWrapper::new(font, px(14.), None);
        let mut text = Rope::from(
            "Hello, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。",
        );

        fn fake_wrap_line(_line: &str, _wrap_width: Pixels, _: usize) -> Vec<Boundary> {
            vec![]
        }

        #[track_caller]
        fn assert_wrapper_lines(text: &Rope, wrapper: &TextWrapper, expected_lines: &[&[&str]]) {
            let mut actual_lines = vec![];
            let mut offset = 0;
            for line in wrapper.iter_lines() {
                actual_lines.push(
                    line.wrapped_lines
                        .iter()
                        .map(|range| text.slice(offset + range.start..offset + range.end))
                        .collect::<Vec<_>>(),
                );
                // +1 \n
                offset += line.len() + 1;
            }
            assert_eq!(actual_lines, expected_lines);
        }

        wrapper._update(&text, &(0..text.len()), &text, &mut fake_wrap_line);
        assert_eq!(wrapper.lines_count(), 4);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["Hello, 世界!\r"],
                &["This is second line."],
                &["This is third line."],
                &["这里是第 4 行。"],
            ],
        );

        // Add a new text to end
        let range = text.len()..text.len();
        let new_text = "New text";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "Hello, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines_count(), 4);
        assert_eq!(wrapper.lines_count(), 4);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["Hello, 世界!\r"],
                &["This is second line."],
                &["This is third line."],
                &["这里是第 4 行。New text"],
            ],
        );

        // Replace first line `Hello` to `AAA`
        let range = 0..5;
        let new_text = "AAA";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "AAA, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines_count(), 4);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["AAA, 世界!\r"],
                &["This is second line."],
                &["This is third line."],
                &["这里是第 4 行。New text"],
            ],
        );

        // Remove the second line
        let start_offset = text.line_start_offset(1);
        let end_offset = text.line_end_offset(1);
        let range = start_offset..end_offset + 1;
        text.replace(range.clone(), "");
        wrapper._update(&text, &range, &Rope::from(""), &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "AAA, 世界!\r\nThis is third line.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines_count(), 3);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["AAA, 世界!\r"],
                &["This is third line."],
                &["这里是第 4 行。New text"],
            ],
        );

        // Replace the first 2 lines to "This is a new line."
        let range = text.line_start_offset(0)..text.line_end_offset(1) + 1;
        let new_text = "This is a new line.\nThis is new line 2.\n";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "This is a new line.\nThis is new line 2.\n这里是第 4 行。New text"
        );
        assert_eq!(wrapper.lines_count(), 3);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["This is a new line."],
                &["This is new line 2."],
                &["这里是第 4 行。New text"],
            ],
        );

        // Add a new line at the end
        let range = text.len()..text.len();
        let new_text = "\nThis is a new line at the end.";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "This is a new line.\nThis is new line 2.\n这里是第 4 行。New text\nThis is a new line at the end."
        );
        assert_eq!(wrapper.lines_count(), 4);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["This is a new line."],
                &["This is new line 2."],
                &["这里是第 4 行。New text"],
                &["This is a new line at the end."],
            ],
        );

        // Add a new line at the beginning
        let range = 0..0;
        let new_text = "This is a new line at the beginning.\n";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "This is a new line at the beginning.\nThis is a new line.\nThis is new line 2.\n这里是第 4 行。New text\nThis is a new line at the end."
        );
        assert_eq!(wrapper.lines_count(), 5);
        assert_wrapper_lines(
            &text,
            &wrapper,
            &[
                &["This is a new line at the beginning."],
                &["This is a new line."],
                &["This is new line 2."],
                &["这里是第 4 行。New text"],
                &["This is a new line at the end."],
            ],
        );

        // Remove all to at least one line in `lines`.
        let range = 0..text.len();
        let new_text = "";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);
        assert_eq!(text.to_string(), "");
        assert_eq!(wrapper.lines_count(), 1);
        assert_eq!(wrapper.line(0).unwrap().wrapped_lines.as_slice(), [0..0]);

        // Test update_all
        let range = 0..text.len();
        let new_text = "This is a full text.\nThis is a second line.";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &text, &mut fake_wrap_line);
        assert_eq!(
            text.to_string(),
            "This is a full text.\nThis is a second line."
        );
        assert_eq!(wrapper.lines_count(), 2);
    }

    fn test_font() -> gpui::Font {
        gpui::Font {
            family: "Arial".into(),
            weight: FontWeight::default(),
            style: FontStyle::Normal,
            features: FontFeatures::default(),
            fallbacks: None,
        }
    }

    /// The longest-row summary stays exact when the previously-longest line is shrunk.
    #[test]
    fn test_longest_row_after_shrink() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        let mut text = Rope::from("aa\nthis is the longest line\nbb");
        wrapper._update(&text, &(0..text.len()), &text, &mut |_, _, _| vec![]);
        assert_eq!(wrapper.longest_row(), 1);

        // Shrink line 1 so line 2-equivalent isn't longest.
        // Make line 0 the longest now.
        let start = text.line_start_offset(0);
        let end = text.line_end_offset(0);
        let range = start..end;
        let new_text = "a very very long first line now";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut |_, _, _| vec![]);
        assert_eq!(wrapper.longest_row(), 0);
    }

    /// Editing the last line and deleting everything must keep the tree consistent.
    #[test]
    fn test_edit_last_line_and_full_delete() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        let mut text = Rope::from("one\ntwo\nthree");
        wrapper._update(&text, &(0..text.len()), &text, &mut |_, _, _| vec![]);
        assert_eq!(wrapper.lines_count(), 3);

        // Replace the last line only.
        let start = text.line_start_offset(2);
        let range = start..text.len();
        let new_text = "THREE EDITED";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut |_, _, _| vec![]);
        assert_eq!(wrapper.lines_count(), 3);
        assert_eq!(wrapper.line(2).unwrap().len(), "THREE EDITED".len());

        // Delete everything.
        let range = 0..text.len();
        text.replace(range.clone(), "");
        wrapper._update(&text, &range, &Rope::from(""), &mut |_, _, _| vec![]);
        assert_eq!(wrapper.lines_count(), 1);
        assert_eq!(wrapper.len(), 1);
        assert_eq!(wrapper.line(0).unwrap().wrapped_lines.as_slice(), [0..0]);
    }

    #[test]
    fn test_wrap_row_buffer_line_boundaries() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        wrapper.text = Rope::from("aa\nbbbb\nc");
        wrapper.lines = line_runs(
            vec![
                LineItem {
                    len: 2,
                    indent: 0,
                    wrapped_lines: smallvec::smallvec![0..2],
                    long: None,
                },
                LineItem {
                    len: 4,
                    indent: 0,
                    wrapped_lines: smallvec::smallvec![0..2, 2..4],
                    long: None,
                },
                LineItem {
                    len: 1,
                    indent: 0,
                    wrapped_lines: smallvec::smallvec![0..1],
                    long: None,
                },
            ],
        );

        assert_eq!(wrapper.lines_count(), 3);
        assert_eq!(wrapper.len(), 4);

        assert_eq!(wrapper.buffer_line_to_first_wrap_row(0), 0);
        assert_eq!(wrapper.buffer_line_to_first_wrap_row(1), 1);
        assert_eq!(wrapper.buffer_line_to_first_wrap_row(2), 3);
        assert_eq!(wrapper.buffer_line_to_first_wrap_row(3), 4);

        assert_eq!(wrapper.buffer_line_to_wrap_row_range(0), 0..1);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(1), 1..3);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(2), 3..4);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(3), 4..4);

        assert_eq!(wrapper.wrap_row_to_buffer_line(0), 0);
        assert_eq!(wrapper.wrap_row_to_buffer_line(1), 1);
        assert_eq!(wrapper.wrap_row_to_buffer_line(2), 1);
        assert_eq!(wrapper.wrap_row_to_buffer_line(3), 2);
        assert_eq!(wrapper.wrap_row_to_buffer_line(4), 2);
    }

    #[test]
    fn test_wrap_row_queries_after_incremental_splice() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), Some(px(10.)));
        let mut text = Rope::from("aa\nbbbb\nc");
        let mut fake_wrap_line = |line: &str, _wrap_width: Pixels, _: usize| {
            if line.len() > 2 {
                vec![Boundary {
                    ix: 2,
                    next_indent: 0,
                }]
            } else {
                vec![]
            }
        };

        wrapper._update(&text, &(0..text.len()), &text, &mut fake_wrap_line);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(0), 0..1);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(1), 1..3);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(2), 3..4);

        let range = text.line_start_offset(1)..text.line_end_offset(1);
        let new_text = "dd\neeee";
        text.replace(range.clone(), new_text);
        wrapper._update(&text, &range, &Rope::from(new_text), &mut fake_wrap_line);

        assert_eq!(wrapper.lines_count(), 4);
        assert_eq!(wrapper.len(), 5);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(0), 0..1);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(1), 1..2);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(2), 2..4);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(3), 4..5);
        assert_eq!(wrapper.wrap_row_to_buffer_line(0), 0);
        assert_eq!(wrapper.wrap_row_to_buffer_line(1), 1);
        assert_eq!(wrapper.wrap_row_to_buffer_line(2), 2);
        assert_eq!(wrapper.wrap_row_to_buffer_line(3), 2);
        assert_eq!(wrapper.wrap_row_to_buffer_line(4), 3);
    }

    #[test]
    fn test_line_layout() {
        let mut line_layout = LineLayout::new();

        let line1 = ShapedLine::default().with_len(100);
        let line2 = ShapedLine::default().with_len(50);
        let wrapped_lines = smallvec::smallvec![line1, line2];
        line_layout.set_wrapped_lines(wrapped_lines);
        assert_eq!(line_layout.len(), 150);
        assert_eq!(line_layout.wrapped_lines.len(), 2);
    }

    /// A layout context whose only load-bearing field is the line height.
    fn test_last_layout(line_height: Pixels) -> LastLayout {
        LastLayout {
            visible_range: 0..1,
            visible_buffer_lines: vec![0],
            visible_line_byte_offsets: vec![0],
            visible_top: px(0.),
            visible_range_offset: 0..0,
            lines: Rc::new(vec![]),
            line_height,
            wrap_width: None,
            wrapping_indent: WrappingIndent::default(),
            line_number_width: px(0.),
            space_width: px(0.),
            cursor_bounds: None,
            text_align: TextAlign::Left,
            content_width: px(0.),
            viewport: Bounds::default(),
        }
    }

    #[test]
    fn test_position_for_index_prefers_first_leading_empty_visual_line() {
        let mut line_layout = LineLayout::new();
        line_layout.set_wrapped_lines(smallvec::smallvec![
            ShapedLine::default(),
            ShapedLine::default(),
            ShapedLine::default().with_len(3),
        ]);

        assert_eq!(
            line_layout.position_for_index(0, &test_last_layout(px(20.)), false),
            Some(point(px(0.), px(0.)))
        );
    }

    #[test]
    fn clicking_past_a_wrapped_row_keeps_the_caret_on_that_row() {
        // One buffer line wrapped into two visual rows, splitting at byte 10.
        let mut line_layout = LineLayout::new();
        line_layout.set_wrapped_lines(smallvec::smallvec![
            ShapedLine::default().with_len(10),
            ShapedLine::default().with_len(5),
        ]);
        let last_layout = test_last_layout(px(20.));

        // Clicking past the last glyph of the first row resolves to the wrap boundary, which is
        // also the first offset of the second row -- hence the affinity.
        let (ix, line_end_affinity) = line_layout
            .closest_index_for_position(point(px(999.), px(5.)), &last_layout)
            .unwrap();
        assert_eq!(ix, 10);
        assert!(line_end_affinity);

        // Carrying that affinity is what keeps the caret on the row that was clicked; dropping it
        // is the bug -- the caret shows up one row below the pointer.
        assert_eq!(
            line_layout
                .position_for_index(ix, &last_layout, line_end_affinity)
                .map(|pos| pos.y),
            Some(px(0.))
        );
        assert_eq!(
            line_layout
                .position_for_index(ix, &last_layout, false)
                .map(|pos| pos.y),
            Some(px(20.))
        );

        // The final row owns the end of the line outright, so there is nothing to disambiguate.
        let (ix, line_end_affinity) = line_layout
            .closest_index_for_position(point(px(999.), px(25.)), &last_layout)
            .unwrap();
        assert_eq!(ix, 15);
        assert!(!line_end_affinity);
    }

    #[test]
    fn a_wrap_boundary_offset_resolves_to_the_row_the_caret_is_drawn_on() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), None);
        wrapper.text = Rope::from("first line\nthis one wraps");
        wrapper.lines = line_runs(
            vec![
                LineItem {
                    len: Rope::from("first line").len(),
                    indent: 0,
                    wrapped_lines: smallvec::smallvec![0..10],
                    long: None,
                },
                LineItem {
                    len: Rope::from("this one wraps").len(),
                    indent: 0,
                    wrapped_lines: smallvec::smallvec![0..9, 9..14],
                    long: None,
                },
            ],
        );

        // Offset 20 is the wrap boundary of the second buffer line: 11 (line start) + 9.
        // It is the last offset of wrap row 1 and the first of wrap row 2 at the same time, so
        // only the affinity can say which row a vertical move should step away from.
        assert_eq!(
            wrapper.offset_to_display_point_with_affinity(20, true),
            WrapDisplayPoint::new(1, 0, 9)
        );
        assert_eq!(
            wrapper.offset_to_display_point_with_affinity(20, false),
            WrapDisplayPoint::new(2, 1, 0)
        );
        assert_eq!(
            wrapper.offset_to_display_point(20),
            wrapper.offset_to_display_point_with_affinity(20, false)
        );

        // An offset that is not on a boundary is unaffected either way.
        for line_end_affinity in [false, true] {
            assert_eq!(
                wrapper.offset_to_display_point_with_affinity(23, line_end_affinity),
                WrapDisplayPoint::new(2, 1, 3)
            );
        }
    }

    #[test]
    fn test_offset_to_display_point() {
        let font = gpui::Font {
            family: "Arial".into(),
            weight: FontWeight::default(),
            style: FontStyle::Normal,
            features: FontFeatures::default(),
            fallbacks: None,
        };

        let mut wrapper = TextWrapper::new(font, px(14.), None);
        wrapper.text = Rope::from(
            "Hello, 世界!\r\nThis is second line.\nThis is third line.\n这里是第 4 行。",
        );
        wrapper.lines = line_runs(
            vec![
                // range: 0..15
                LineItem {
                    len: Rope::from("Hello, 世界!\r").len(),
                    indent: 0,
                    wrapped_lines: smallvec::smallvec![0..15],
                    long: None,
                },
                // range: 16..36
                LineItem {
                    len: Rope::from("This is second line.\n").len(),
                    indent: 0,
                    wrapped_lines: smallvec::smallvec![0..10, 10..20],
                    long: None,
                },
                // range: 37..56
                LineItem {
                    len: Rope::from("This is third line.\n").len(),
                    indent: 0,
                    wrapped_lines: smallvec::smallvec![0..9, 9..15, 15..20],
                    long: None,
                },
                // range: 57..79
                LineItem {
                    len: Rope::from("这里是第 4 行。").len(),
                    indent: 0,
                    wrapped_lines: smallvec::smallvec![0..22],
                    long: None,
                },
            ],
        );

        assert_eq!(
            wrapper.offset_to_display_point(12),
            WrapDisplayPoint::new(0, 0, 12)
        );
        assert_eq!(
            wrapper.offset_to_display_point(15),
            WrapDisplayPoint::new(0, 0, 15)
        );

        assert_eq!(
            wrapper.offset_to_display_point(16),
            WrapDisplayPoint::new(1, 0, 0)
        );
        assert_eq!(
            wrapper.offset_to_display_point(21),
            WrapDisplayPoint::new(1, 0, 5)
        );
        assert_eq!(
            wrapper.offset_to_display_point(27),
            WrapDisplayPoint::new(2, 1, 1)
        );
        assert_eq!(
            wrapper.offset_to_display_point(37),
            WrapDisplayPoint::new(3, 0, 0)
        );
        assert_eq!(
            wrapper.offset_to_display_point(54),
            WrapDisplayPoint::new(5, 2, 2)
        );
        assert_eq!(
            wrapper.offset_to_display_point(59),
            WrapDisplayPoint::new(6, 0, 2)
        );

        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(6, 0, 2)),
            59
        );
        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(5, 2, 2)),
            54
        );
        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(3, 0, 0)),
            37
        );
        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(2, 1, 1)),
            27
        );
        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(1, 0, 5)),
            21
        );
        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(1, 0, 0)),
            16
        );
        assert_eq!(
            wrapper.display_point_to_offset(WrapDisplayPoint::new(0, 0, 15)),
            15
        );
    }

    #[test]
    fn test_wrapping_indent_same_keeps_indent_reserved() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.0), Some(px(10.)));
        wrapper.wrapping_indent = WrappingIndent::Same;
        let text = Rope::from("  abcdefghijklmnopqrstuv");
        let mut fake_wrap_line = |line: &str, _wrap_width: Pixels, _: usize| {
            if line.starts_with(' ') {
                vec![Boundary {
                    ix: 5,
                    next_indent: 2,
                }]
            } else {
                let mut boundaries = vec![];
                let mut i = 8;
                while i < line.len() {
                    boundaries.push(Boundary {
                        ix: i,
                        next_indent: 0,
                    });
                    i += 8;
                }
                boundaries
            }
        };

        wrapper._update(&text, &(0..text.len()), &text, &mut fake_wrap_line);

        let line = wrapper.line(0).unwrap();
        assert_eq!(line.indent, 2);
        assert_eq!(line.wrapped_lines.as_slice(), [0..5, 5..24]);
    }

    #[test]
    fn test_wrapping_indent_none_continuation_lines_wrapped_at_full_width() {
        let mut wrapper = TextWrapper::new(test_font(), px(14.0), Some(px(10.)));
        wrapper.wrapping_indent = WrappingIndent::None;
        let text = Rope::from("  abcdefghijklmnopqrstuv");
        let mut fake_wrap_line = |line: &str, _wrap_width: Pixels, _: usize| {
            if line.starts_with(' ') {
                vec![Boundary {
                    ix: 5,
                    next_indent: 2,
                }]
            } else {
                let mut boundaries = vec![];
                let mut i = 8;
                while i < line.len() {
                    boundaries.push(Boundary {
                        ix: i,
                        next_indent: 0,
                    });
                    i += 8;
                }
                boundaries
            }
        };

        wrapper._update(&text, &(0..text.len()), &text, &mut fake_wrap_line);

        let line = wrapper.line(0).unwrap();
        assert_eq!(line.indent, 0);
        assert_eq!(line.wrapped_lines.as_slice(), [0..5, 5..13, 13..21, 21..24]);
    }

    #[test]
    fn test_wrap_indent_offsets_continuation_lines() {
        let mut line_layout = LineLayout::new();
        line_layout.set_wrapped_lines(smallvec::smallvec![
            ShapedLine::default().with_len(5),
            ShapedLine::default().with_len(10),
        ]);

        line_layout = line_layout.wrap_indent(px(20.0));

        let last_layout = LastLayout {
            visible_range: 0..1,
            visible_buffer_lines: vec![0],
            visible_line_byte_offsets: vec![0],
            visible_top: px(0.),
            visible_range_offset: 0..0,
            lines: Rc::new(vec![]),
            line_height: px(20.0),
            wrap_width: Some(px(10.)),
            wrapping_indent: WrappingIndent::Same,
            line_number_width: px(0.),
            space_width: px(0.),
            cursor_bounds: None,
            text_align: TextAlign::Left,
            content_width: px(0.),
            viewport: Bounds::default(),
        };

        assert_eq!(
            line_layout.position_for_index(0, &last_layout, false),
            Some(point(px(0.), px(0.))),
        );

        assert_eq!(
            line_layout.position_for_index(6, &last_layout, false),
            Some(point(px(20.), px(20.))),
        )
    }

    /// Wraps a line every 8 bytes past its first 12, at char boundaries.
    fn every_eight(line: &str, _: Pixels, _: usize) -> Vec<Boundary> {
        (12..line.len())
            .step_by(8)
            .filter(|&ix| line.is_char_boundary(ix))
            .map(|ix| Boundary { ix, next_indent: 0 })
            .collect()
    }

    fn fresh_wrapper(text: &Rope) -> TextWrapper {
        let mut wrapper = TextWrapper::new(test_font(), px(14.), Some(px(100.)));
        wrapper._update(text, &(0..text.len()), text, &mut every_eight);
        wrapper
    }

    #[track_caller]
    fn assert_same_index(text: &Rope, wrapper: &TextWrapper) {
        let fresh = fresh_wrapper(text);
        assert_eq!(wrapper.lines_count(), fresh.lines_count());
        assert_eq!(wrapper.len(), fresh.len());
        assert_eq!(wrapper.longest_row(), fresh.longest_row());
        for row in 0..=fresh.lines_count() {
            let (a, b) = (wrapper.line(row), fresh.line(row));
            assert_eq!(a.as_ref().map(|l| (l.len(), l.wrapped_lines.clone())), b.as_ref().map(|l| (l.len(), l.wrapped_lines.clone())), "row {row}");
            assert_eq!(wrapper.buffer_line_to_wrap_row_range(row), fresh.buffer_line_to_wrap_row_range(row), "row {row}");
        }
        for wrap_row in 0..=fresh.len() {
            assert_eq!(wrapper.wrap_row_to_buffer_line(wrap_row), fresh.wrap_row_to_buffer_line(wrap_row));
            let point = WrapDisplayPoint::new(wrap_row, 0, 5);
            assert_eq!(wrapper.display_point_to_offset(point), fresh.display_point_to_offset(point));
        }
        for offset in (0..=text.len()).filter(|&ix| text.is_char_boundary(ix)) {
            for affinity in [false, true] {
                assert_eq!(
                    wrapper.offset_to_display_point_with_affinity(offset, affinity),
                    fresh.offset_to_display_point_with_affinity(offset, affinity),
                    "offset {offset}"
                );
            }
        }
    }

    #[test]
    fn lines_that_dont_wrap_cost_a_run_not_an_item() {
        let line = "let a = 1\n";
        let text = Rope::from(line.repeat(100_000).as_str());
        let wrapper = fresh_wrapper(&text);
        assert_eq!(wrapper.lines_count(), 100_001);
        // ~782 runs of 128 lines, not 100,001 items.
        assert!(wrapper.lines.iter().count() <= 100_001 / PLAIN_RUN_LINES as usize + 1);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(70_000), 70_000..70_001);
        assert_eq!(wrapper.wrap_row_to_buffer_line(70_000), 70_000);
        let offset = text.line_start_offset(70_000) + 4;
        assert_eq!(wrapper.offset_to_display_point(offset), WrapDisplayPoint::new(70_000, 0, 4));
        assert_eq!(wrapper.display_point_to_offset(WrapDisplayPoint::new(70_000, 0, 4)), offset);
        // A wrapped line among them is kept whole.
        let mut text = text;
        let at = text.line_start_offset(500);
        let long = "a line long enough to wrap twice over";
        text.insert(at, long);
        let mut wrapper = wrapper;
        wrapper._update(&text, &(at..at), &Rope::from(long), &mut every_eight);
        // 46 bytes: rows break at 12, 20, 28, 36 and 44.
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(500), 500..506);
        assert_eq!(wrapper.buffer_line_to_wrap_row_range(501), 506..507);
        assert_eq!(wrapper.longest_row(), 500);
        assert_eq!(wrapper.lines.iter().filter(|run| matches!(run, LineRun::Line(_))).count(), 1);
    }

    #[test]
    fn edits_keep_the_runs_what_a_fresh_index_has() {
        // Edits of every kind at pseudo-random places: inserting and deleting
        // lines, short and wrapping ones, across run boundaries and at the ends.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n.max(1) as u64) as usize
        };
        let pieces = ["", "x", "\n", "ab\ncd", "a wrapping line of text here", "\n\n\n", "é\n", "short\nand a much longer line that wraps\n"];
        let base: String = (0..400).map(|i| if i % 37 == 0 { "this line is long enough to wrap\n".to_string() } else { format!("line {i}\n") }).collect();
        let mut text = Rope::from(base.as_str());
        let mut wrapper = fresh_wrapper(&text);
        assert_same_index(&text, &wrapper);
        for step in 0..120 {
            let len = text.len();
            let mut start = next(len + 1);
            while !text.is_char_boundary(start) {
                start -= 1;
            }
            let mut end = (start + [0, 1, 5, 40, 300, 2000][next(6)]).min(len);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let insert = pieces[next(pieces.len())];
            text.replace(start..end, insert);
            wrapper._update(&text, &(start..end), &Rope::from(insert), &mut every_eight);
            if step % 10 == 0 || step > 110 {
                assert_same_index(&text, &wrapper);
            }
        }
        assert_same_index(&text, &wrapper);
        // Everything deleted, then typed again.
        let all = 0..text.len();
        text.replace(all.clone(), "");
        wrapper._update(&text, &all, &Rope::new(), &mut every_eight);
        assert_same_index(&text, &wrapper);
        text.insert(0, "a\nb");
        wrapper._update(&text, &(0..0), &Rope::from("a\nb"), &mut every_eight);
        assert_same_index(&text, &wrapper);
    }
}

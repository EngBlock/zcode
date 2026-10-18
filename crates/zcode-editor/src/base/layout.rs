use std::{ops::Range, rc::Rc};

use gpui::{Bounds, Half, Pixels, ShapedLine, TextAlign, px};

use super::{WrappingIndent, display_map::LineLayout};

#[derive(Clone, Default)]
pub(crate) struct WhitespaceIndicators {
    pub(crate) space: ShapedLine,
    pub(crate) tab: ShapedLine,
}

#[derive(Clone)]
pub(super) struct LastLayout {
    pub(super) visible_range: Range<usize>,
    pub(super) visible_buffer_lines: Vec<usize>,
    pub(super) visible_line_byte_offsets: Vec<usize>,
    pub(super) visible_top: Pixels,
    pub(super) visible_range_offset: Range<usize>,
    pub(super) lines: Rc<Vec<LineLayout>>,
    pub(super) line_height: Pixels,
    pub(super) wrap_width: Option<Pixels>,
    pub(super) wrapping_indent: WrappingIndent,
    pub(super) line_number_width: Pixels,
    /// Width of one space in the editor font.
    ///
    /// Past the end of a line there are no glyphs to hit-test against, so this is the
    /// step used to measure how far past the end a pointer sits.
    pub(super) space_width: Pixels,
    pub(super) cursor_bounds: Option<Bounds<Pixels>>,
    pub(super) text_align: TextAlign,
    pub(super) content_width: Pixels,
    /// The text area on screen, in content coordinates (x from the text's
    /// left edge, y from the document's top), after this frame's scrolling.
    /// Long lines draw ranges only here.
    pub(super) viewport: Bounds<Pixels>,
}

impl LastLayout {
    pub(crate) fn line(&self, row: usize) -> Option<&LineLayout> {
        let pos = self.visible_buffer_lines.binary_search(&row).ok()?;
        self.lines.get(pos)
    }

    /// The laid-out text's byte ranges, ascending and merged: each visible
    /// line with its newline, or a long line's laid-out pieces.
    pub(super) fn visible_spans(&self) -> Vec<Range<usize>> {
        let mut spans: Vec<Range<usize>> = Vec::new();
        let mut push = |span: Range<usize>| {
            let span = span.start..span.end.min(self.visible_range_offset.end);
            if let Some(previous) = spans.last_mut().filter(|previous| previous.end == span.start) {
                previous.end = span.end;
            } else if span.start < span.end {
                spans.push(span);
            }
        };
        for (&offset, line) in self.visible_line_byte_offsets.iter().zip(self.lines.iter()) {
            match line.laid_out_ranges() {
                Some(pieces) => {
                    for piece in pieces {
                        push(offset + piece.start..offset + piece.end);
                    }
                }
                None => push(offset..offset + line.len() + 1),
            }
        }
        spans
    }

    pub(super) fn alignment_offset(&self, line_width: Pixels) -> Pixels {
        match self.text_align {
            TextAlign::Left => px(0.),
            TextAlign::Center => (self.content_width - line_width).half().max(px(0.)),
            TextAlign::Right => (self.content_width - line_width).max(px(0.)),
        }
    }
}

//! A visual row whose offsets remain in source UTF-8 bytes.
use super::long_line::LongLine;
use gpui::{App, Pixels, Point, ShapedLine, SharedString, TextAlign, Window, point, px};
use smallvec::SmallVec;
use std::{ops::Range, sync::Arc};

pub(crate) struct InlineFragment {
    pub(crate) range: Range<usize>,
    pub(crate) x: Pixels,
    pub(crate) width: Pixels,
    pub(crate) text: Option<ShapedLine>,
    /// An inlay: an empty range with a width. `Some(before_text)`: whether
    /// the caret at its offset is drawn after it (it belongs to the text
    /// after it) or before it.
    pub(crate) inlay: Option<bool>,
}

/// A shaped piece of a long line, at its measured x.
pub(crate) struct LineTile {
    pub(crate) range: Range<usize>,
    pub(crate) x: Pixels,
    pub(crate) line: ShapedLine,
}

pub(crate) struct InputLine {
    pub(crate) len: usize,
    pub(crate) width: Pixels,
    /// The row's text; empty for a long line's tiles, which hold their own.
    pub(crate) text: SharedString,
    content: Content,
}
// Keep the ordinary shaped row inline: boxing it would add an allocation to
// every existing plain-text row. Only token rows allocate fragment storage.
#[allow(clippy::large_enum_variant)]
enum Content {
    Text(ShapedLine),
    Inline(Vec<InlineFragment>),
    /// A long line with soft wrap off (`long_line.rs`): the pieces on screen
    /// shaped, ascending; any other offset measured from the line.
    Tiles(Arc<LongLine>, Vec<LineTile>),
}
impl From<ShapedLine> for InputLine {
    fn from(line: ShapedLine) -> Self {
        Self {
            len: line.len,
            width: line.width,
            text: line.text.clone(),
            content: Content::Text(line),
        }
    }
}
impl InputLine {
    pub(crate) fn inline(text: SharedString, fragments: Vec<InlineFragment>) -> Self {
        let width = fragments.last().map_or(px(0.), |f| f.x + f.width);
        Self {
            len: text.len(),
            text,
            width,
            content: Content::Inline(fragments),
        }
    }
    /// A long line, with `tiles` (ascending, disjoint) shaped.
    pub(crate) fn tiled(long: Arc<LongLine>, tiles: Vec<LineTile>) -> Self {
        Self {
            len: long.len(),
            width: px(long.width() as f32),
            text: SharedString::default(),
            content: Content::Tiles(long, tiles),
        }
    }
    /// The row's shaped text and where each piece starts in the row.
    pub(crate) fn shaped_text(&self) -> SmallVec<[(usize, &str); 1]> {
        match &self.content {
            Content::Tiles(_, tiles) => tiles
                .iter()
                .map(|tile| (tile.range.start, tile.line.text.as_ref()))
                .collect(),
            _ => smallvec::smallvec![(0, self.text.as_ref())],
        }
    }
    /// The bytes shaped when the row is a long line's tiles.
    pub(crate) fn tile_ranges(&self) -> Option<impl Iterator<Item = Range<usize>> + '_> {
        match &self.content {
            Content::Tiles(_, tiles) => Some(tiles.iter().map(|tile| tile.range.clone())),
            _ => None,
        }
    }
    pub(crate) fn x_for_index(&self, ix: usize) -> Pixels {
        match &self.content {
            Content::Text(line) => line.x_for_index(ix),
            Content::Tiles(long, tiles) => {
                // A tile's end is the next one's start when they touch.
                let next = tiles.partition_point(|tile| tile.range.start <= ix);
                match next.checked_sub(1).map(|ix| &tiles[ix]) {
                    Some(tile) if ix <= tile.range.end => {
                        tile.x + tile.line.x_for_index(ix - tile.range.start)
                    }
                    _ => px(long.x(ix) as f32),
                }
            }
            Content::Inline(fragments) => {
                for f in fragments {
                    if let Some(before_text) = f.inlay {
                        if ix == f.range.start && !before_text {
                            return f.x;
                        }
                        continue;
                    }
                    if ix < f.range.end {
                        return f.x
                            + f.text.as_ref().map_or(
                                if ix <= f.range.start { px(0.) } else { f.width },
                                |line| line.x_for_index(ix.saturating_sub(f.range.start)),
                            );
                    }
                }
                self.width
            }
        }
    }
    pub(crate) fn closest_index_for_x(&self, x: Pixels) -> usize {
        match &self.content {
            Content::Text(line) => line.closest_index_for_x(x),
            Content::Tiles(long, tiles) => {
                let next = tiles.partition_point(|tile| tile.x <= x);
                match next.checked_sub(1).map(|ix| &tiles[ix]) {
                    Some(tile) if x <= tile.x + tile.line.width => {
                        tile.range.start + tile.line.closest_index_for_x(x - tile.x)
                    }
                    _ => long.index_for_x(x.as_f32() as f64),
                }
            }
            Content::Inline(fragments) => {
                for f in fragments {
                    if x <= f.x + f.width {
                        if f.inlay.is_some() {
                            return f.range.start;
                        }
                        return f.text.as_ref().map_or(
                            if x - f.x < f.width / 2. {
                                f.range.start
                            } else {
                                f.range.end
                            },
                            |line| f.range.start + line.closest_index_for_x(x - f.x),
                        );
                    }
                }
                self.len
            }
        }
    }
    pub(crate) fn index_for_x(&self, x: Pixels) -> Option<usize> {
        match &self.content {
            Content::Text(line) => line.index_for_x(x),
            Content::Inline(_) | Content::Tiles(..) => {
                (x >= px(0.) && x <= self.width).then(|| self.closest_index_for_x(x))
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        &self,
        pos: Point<Pixels>,
        height: Pixels,
        align: TextAlign,
        width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.paint_pass(pos, height, align, width, false, window, cx);
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_background(
        &self,
        pos: Point<Pixels>,
        height: Pixels,
        align: TextAlign,
        width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.paint_pass(pos, height, align, width, true, window, cx);
    }
    #[allow(clippy::too_many_arguments)]
    fn paint_pass(
        &self,
        pos: Point<Pixels>,
        height: Pixels,
        align: TextAlign,
        width: Option<Pixels>,
        background: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let paint = |line: &ShapedLine, pos, align, width, window: &mut Window, cx: &mut App| {
            if background {
                let _ = line.paint_background(pos, height, align, width, window, cx);
            } else {
                let _ = line.paint(pos, height, align, width, window, cx);
            }
        };
        match &self.content {
            Content::Text(line) => paint(line, pos, align, width, window, cx),
            // The editor's text is left-aligned.
            Content::Tiles(_, tiles) => {
                for tile in tiles {
                    paint(&tile.line, pos + point(tile.x, px(0.)), TextAlign::Left, None, window, cx);
                }
            }
            Content::Inline(fragments) => {
                let remaining = (width.unwrap_or(self.width) - self.width).max(px(0.));
                let offset = match align {
                    TextAlign::Right => remaining,
                    TextAlign::Center => remaining / 2.,
                    _ => px(0.),
                };
                for f in fragments {
                    if let Some(line) = &f.text {
                        paint(
                            line,
                            pos + point(offset + f.x, px(0.)),
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }
                }
            }
        }
    }
}

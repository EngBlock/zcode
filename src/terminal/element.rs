//! Painting a terminal's grid in the editor's font: cell backgrounds,
//! text in runs of one style (each character that isn't plain ASCII on its
//! own cell, so wide and fallback glyphs line up), the selection, the
//! cursor and the link under the mouse. The grid is copied out under the
//! terminal's lock (blank cells skipped) and laid out after it's released,
//! so the reader thread waits for a copy, never for shaping.

use std::sync::Arc;

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

use super::colors::Palette;
use super::view::{GridLayout, HoveredLink, TerminalView};
use super::{Listener, Terminal};

/// Room around the grid.
const PADDING: f32 = 6.;

pub(super) struct TerminalElement {
    view: Entity<TerminalView>,
    focus: FocusHandle,
}

impl TerminalElement {
    pub fn new(view: Entity<TerminalView>, focus: FocusHandle) -> Self {
        Self { view, focus }
    }
}

/// A cell worth painting: not a blank on the default background.
struct Cell {
    row: usize,
    column: usize,
    text: char,
    /// Combining marks drawn with it.
    zero_width: Option<Vec<char>>,
    fg: Hsla,
    bg: Option<Hsla>,
    flags: Flags,
}

struct Snapshot {
    cells: Vec<Cell>,
    /// Row, columns of each selected stretch on screen.
    selection: Vec<(usize, std::ops::Range<usize>)>,
    cursor: Option<(usize, usize, CursorShape, char, bool)>,
    display_offset: i32,
}

pub(super) struct Painted {
    hitbox: Hitbox,
    layout: GridLayout,
    backgrounds: Vec<(Bounds<Pixels>, Hsla)>,
    texts: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<(Bounds<Pixels>, Hsla, bool, Option<(Point<Pixels>, ShapedLine)>)>,
    underlines: Vec<(Bounds<Pixels>, Hsla)>,
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Painted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let theme = cx.theme();
        let palette = Palette::from_theme(theme);
        let font = Font { family: theme.mono_font_family.clone(), ..Font::default() };
        let font_size = px(self.view.read(cx).font_size);
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        let cell_width = text_system.advance(font_id, font_size, 'm').map(|size| size.width).unwrap_or(font_size * 0.6);
        let cell = size(cell_width, (font_size * 1.35).round());
        let origin = bounds.origin + point(px(PADDING), px(PADDING));
        let columns = ((f32::from(bounds.size.width) - 2. * PADDING) / f32::from(cell.width)).floor().max(2.) as usize;
        let rows = ((f32::from(bounds.size.height) - 2. * PADDING) / f32::from(cell.height)).floor().max(1.) as usize;
        let layout = GridLayout { origin, cell, columns, rows };
        if self.view.read(cx).layout != Some(layout) || self.view.read(cx).terminal().is_none() {
            self.view.update(cx, |view, cx| view.laid_out(layout, cx));
        }
        let view = self.view.read(cx);
        let hovered = view.hovered_link.clone();
        let snapshot = view.terminal().cloned().map(|terminal| snapshot(&terminal, &palette, rows));
        let focused = self.focus.is_focused(window);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let mut painted =
            Painted { hitbox, layout, backgrounds: vec![], texts: vec![], cursor: None, underlines: vec![] };
        if let Some(snapshot) = snapshot {
            lay_out(&mut painted, snapshot, &palette, &font, font_size, focused, hovered.as_ref(), window);
        }
        painted
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        painted: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_height = painted.layout.cell.height;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for (bounds, color) in &painted.backgrounds {
                window.paint_quad(fill(*bounds, *color));
            }
            for (origin, line) in &painted.texts {
                line.paint(*origin, line_height, TextAlign::Left, None, window, cx).ok();
            }
            for (bounds, color) in &painted.underlines {
                window.paint_quad(fill(*bounds, *color));
            }
            if let Some((bounds, color, filled, text)) = &painted.cursor {
                if *filled {
                    window.paint_quad(fill(*bounds, *color));
                    if let Some((origin, line)) = text {
                        line.paint(*origin, line_height, TextAlign::Left, None, window, cx).ok();
                    }
                } else {
                    window.paint_quad(outline(*bounds, *color, BorderStyle::Solid));
                }
            }
        });

        let hitbox = painted.hitbox.clone();
        let link = self.view.read(cx).hovered_link.is_some();
        window.set_cursor_style(if link { CursorStyle::PointingHand } else { CursorStyle::IBeam }, &hitbox);
        let view = self.view.clone();
        window.on_mouse_event({
            let hitbox = hitbox.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                    view.update(cx, |view, cx| view.mouse_down(event, window, cx));
                }
            }
        });
        let view = self.view.clone();
        window.on_mouse_event({
            let hitbox = hitbox.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                // A drag goes on outside the grid.
                if phase == DispatchPhase::Bubble && (hitbox.is_hovered(window) || event.pressed_button.is_some()) {
                    view.update(cx, |view, cx| view.mouse_move(event, cx));
                }
            }
        });
        let view = self.view.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                view.update(cx, |view, cx| view.mouse_up(event, cx));
            }
        });
        let view = self.view.clone();
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hitbox.should_handle_scroll(window) {
                view.update(cx, |view, cx| view.scroll_wheel(event, cx));
                cx.stop_propagation();
            }
        });
    }
}

/// The screen's cells worth painting, the selection and the cursor, copied
/// out under the lock.
fn snapshot(terminal: &Arc<Terminal>, palette: &Palette, rows: usize) -> Snapshot {
    let term: parking_lot::MutexGuard<'_, alacritty_terminal::Term<Listener>> = terminal.lock();
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let colors = *content.colors;
    let mut cells = Vec::with_capacity(1024);
    for indexed in content.display_iter {
        let cell = &*indexed;
        let flags = cell.flags;
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let default_bg = cell.bg == Color::Named(NamedColor::Background);
        if cell.c == ' ' && default_bg && !flags.intersects(Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT) {
            continue;
        }
        let row = (indexed.point.line.0 + offset) as usize;
        let mut fg = palette.resolve(cell.fg, &colors);
        let mut bg = (!default_bg).then(|| palette.resolve(cell.bg, &colors));
        if flags.contains(Flags::INVERSE) {
            let background = bg.unwrap_or(palette.background);
            bg = Some(fg);
            fg = background;
        }
        if flags.contains(Flags::DIM) {
            fg = fg.opacity(0.7);
        }
        cells.push(Cell {
            row,
            column: indexed.point.column.0,
            text: if flags.contains(Flags::HIDDEN) { ' ' } else { cell.c },
            zero_width: cell.zerowidth().map(<[char]>::to_vec),
            fg,
            bg,
            flags,
        });
    }
    let columns = term.columns();
    let mut selection = vec![];
    if let Some(range) = content.selection {
        for row in 0..rows {
            let line = row as i32 - offset;
            if line < range.start.line.0 || line > range.end.line.0 {
                continue;
            }
            let start = if range.is_block || line == range.start.line.0 { range.start.column.0 } else { 0 };
            let end = if range.is_block || line == range.end.line.0 { range.end.column.0 + 1 } else { columns };
            if start < end {
                selection.push((row, start..end.min(columns)));
            }
        }
    }
    let cursor = content.cursor;
    let cursor_row = cursor.point.line.0 + offset;
    let cursor = (cursor.shape != CursorShape::Hidden && cursor_row >= 0 && (cursor_row as usize) < rows).then(|| {
        let cell = &term.grid()[GridPoint::new(Line(cursor.point.line.0), Column(cursor.point.column.0))];
        (cursor_row as usize, cursor.point.column.0, cursor.shape, cell.c, cell.flags.contains(Flags::WIDE_CHAR))
    });
    Snapshot { cells, selection, cursor, display_offset: offset }
}

#[allow(clippy::too_many_arguments)]
fn lay_out(
    painted: &mut Painted,
    snapshot: Snapshot,
    palette: &Palette,
    font: &Font,
    font_size: Pixels,
    focused: bool,
    hovered: Option<&HoveredLink>,
    window: &mut Window,
) {
    let GridLayout { origin, cell, .. } = painted.layout;
    let at = |row: usize, column: usize| origin + point(cell.width * column as f32, cell.height * row as f32);
    let cell_bounds = |row: usize, column: usize, width: usize| Bounds::new(at(row, column), size(cell.width * width as f32, cell.height));

    // Backgrounds, a stretch of one color at a time.
    let mut run: Option<(usize, usize, usize, Hsla)> = None;
    for c in &snapshot.cells {
        let Some(bg) = c.bg else { continue };
        let width = if c.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        match &mut run {
            Some((row, start, end, color)) if *row == c.row && *end == c.column && *color == bg => *end += width,
            _ => {
                if let Some((row, start, end, color)) = run.take() {
                    painted.backgrounds.push((cell_bounds(row, start, end - start), color));
                }
                run = Some((c.row, c.column, c.column + width, bg));
            }
        }
    }
    if let Some((row, start, end, color)) = run {
        painted.backgrounds.push((cell_bounds(row, start, end - start), color));
    }
    for (row, columns) in &snapshot.selection {
        painted.backgrounds.push((cell_bounds(*row, columns.start, columns.len()), palette.selection));
    }

    // Text: ASCII in runs of one style; anything else on its own cell.
    let style_font = |flags: Flags| {
        let mut font = font.clone();
        if flags.contains(Flags::BOLD) {
            font.weight = FontWeight::BOLD;
        }
        if flags.contains(Flags::ITALIC) {
            font.style = FontStyle::Italic;
        }
        font
    };
    let decorations = |flags: Flags, color: Hsla| {
        let underline = flags.intersects(Flags::ALL_UNDERLINES).then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(color),
            wavy: flags.contains(Flags::UNDERCURL),
        });
        let strikethrough = flags.contains(Flags::STRIKEOUT).then(|| StrikethroughStyle { thickness: px(1.), color: Some(color) });
        (underline, strikethrough)
    };
    let style_bits = Flags::BOLD | Flags::ITALIC | Flags::ALL_UNDERLINES | Flags::STRIKEOUT;
    struct Piece {
        row: usize,
        column: usize,
        text: String,
        color: Hsla,
        flags: Flags,
        /// Plain ASCII so far, so more can join it.
        open: bool,
    }
    let mut pieces: Vec<Piece> = vec![];
    for c in &snapshot.cells {
        let decorated = c.flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT);
        if c.text == ' ' && !decorated {
            continue;
        }
        let simple = c.text.is_ascii() && c.zero_width.is_none();
        if simple
            && let Some(piece) = pieces.last_mut()
            && piece.open
            && piece.row == c.row
            && piece.color == c.fg
            && (piece.flags & style_bits) == (c.flags & style_bits)
        {
            // Blank cells between are spaces in the run (unless they'd
            // take its underline).
            let end = piece.column + piece.text.len();
            let gap = c.column.saturating_sub(end);
            if c.column >= end && (gap == 0 || (!decorated && gap <= 8)) {
                piece.text.extend(std::iter::repeat_n(' ', gap));
                piece.text.push(c.text);
                continue;
            }
        }
        let mut text = c.text.to_string();
        text.extend(c.zero_width.iter().flatten());
        pieces.push(Piece { row: c.row, column: c.column, text, color: c.fg, flags: c.flags, open: simple });
    }
    for piece in pieces {
        let (underline, strikethrough) = decorations(piece.flags, piece.color);
        let run = TextRun { len: piece.text.len(), font: style_font(piece.flags), color: piece.color, background_color: None, underline, strikethrough };
        let shaped = window.text_system().shape_line(piece.text.into(), font_size, &[run], None);
        painted.texts.push((at(piece.row, piece.column), shaped));
    }

    // The link under the mouse, underlined.
    if let Some(link) = hovered {
        let display_offset = snapshot.display_offset;
        let columns = painted.layout.columns;
        for line in link.start.line.0..=link.end.line.0 {
            let row = line + display_offset;
            if row < 0 || row as usize >= painted.layout.rows {
                continue;
            }
            let start = if line == link.start.line.0 { link.start.column.0 } else { 0 };
            let end = if line == link.end.line.0 { link.end.column.0 + 1 } else { columns };
            let mut bounds = cell_bounds(row as usize, start, end.saturating_sub(start));
            bounds.origin.y += cell.height - px(2.);
            bounds.size.height = px(1.);
            painted.underlines.push((bounds, palette.link));
        }
    }

    // The cursor: a block (hollow when the terminal isn't focused), a bar
    // or an underline.
    if let Some((row, column, shape, ch, wide)) = snapshot.cursor {
        let width = if wide { 2 } else { 1 };
        let mut bounds = cell_bounds(row, column, width);
        let shape = if focused { shape } else { CursorShape::HollowBlock };
        let (filled, text) = match shape {
            CursorShape::Block => {
                let text = (ch != ' ').then(|| {
                    let run = TextRun { len: ch.len_utf8(), font: font.clone(), color: palette.background, background_color: None, underline: None, strikethrough: None };
                    (at(row, column), window.text_system().shape_line(ch.to_string().into(), font_size, &[run], None))
                });
                (true, text)
            }
            CursorShape::Beam => {
                bounds.size.width = px(2.);
                (true, None)
            }
            CursorShape::Underline => {
                bounds.origin.y += cell.height - px(2.);
                bounds.size.height = px(2.);
                (true, None)
            }
            CursorShape::HollowBlock | CursorShape::Hidden => (false, None),
        };
        painted.cursor = Some((bounds, palette.cursor, filled, text));
    }
}

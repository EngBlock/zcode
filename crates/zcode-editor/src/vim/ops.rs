//! Operators and the commands that edit: what `d`, `c`, `y`, `>`, `g~`…
//! do over the ranges a motion, an object or a selection gave them, and
//! paste, join, replace, case and increment.
//!
//! Every command is one transaction (one undo step), made after the
//! cursors were put where undo should leave them.

use std::ops::Range;

use gpui::{Context, Window};
use ropey::Rope;

use super::motion::{Kind, Target};
use super::object::Selected;
use super::registers::{self, Kind as RegisterKind, Register, Write};
use super::text;
use super::{Cursor, Repeat, Typed, Vim, VimCommand, VimMode};
use crate::EditorState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
    Reindent,
    ToggleCase,
    Lowercase,
    Uppercase,
}

/// What one cursor's operator covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Span {
    pub(super) range: Range<usize>,
    pub(super) linewise: bool,
    pub(super) inner_lines: bool,
}

impl Span {
    pub(super) fn chars(range: Range<usize>) -> Self {
        Span { range, linewise: false, inner_lines: false }
    }

    pub(super) fn lines(range: Range<usize>) -> Self {
        Span { range, linewise: true, inner_lines: false }
    }

    pub(super) fn object(selected: Selected) -> Self {
        Span { range: selected.range, linewise: selected.linewise, inner_lines: selected.inner_lines }
    }
}

/// Whole lines `top` to `bottom`, newline included.
pub(super) fn line_range(text: &Rope, top: usize, bottom: usize) -> Range<usize> {
    text::line_start(text, top)..text::line_full_end(text, bottom)
}

/// What an operator covers from `from` to a motion's target.
pub(super) fn motion_span(text: &Rope, from: usize, target: Target) -> Option<Span> {
    let (start, end) = (from.min(target.offset), from.max(target.offset));
    match target.kind {
        Kind::Linewise => Some(Span::lines(line_range(text, text::row(text, start), text::row(text, end)))),
        Kind::Inclusive => Some(Span::chars(start..text::next(text, end).max(start))),
        Kind::Exclusive => {
            let (start_row, end_row) = (text::row(text, start), text::row(text, end));
            // An exclusive motion ending at a line's start stops at the end
            // of the line before (and covers whole lines when it started at
            // or before its line's first character): `d}`.
            if end_row > start_row && end == text::line_start(text, end_row) {
                if start <= text::first_non_blank(text, start_row) {
                    return Some(Span::lines(line_range(text, start_row, end_row - 1)));
                }
                return Some(Span::chars(start..text::line_end(text, end_row - 1)));
            }
            (start < end).then(|| Span::chars(start..end))
        }
    }
}

/// Where `offset` is after `edits` (sorted, not overlapping) are applied.
pub(super) fn map_offset(edits: &[(Range<usize>, String)], offset: usize) -> usize {
    let mut delta = 0isize;
    for (range, new) in edits {
        if range.start >= offset {
            break;
        }
        if range.end <= offset {
            delta += new.len() as isize - range.len() as isize;
        } else {
            return (range.start as isize + delta) as usize + new.len().min(offset - range.start);
        }
    }
    (offset as isize + delta) as usize
}

fn merge(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by_key(|s| s.range.start);
    let mut merged: Vec<Span> = vec![];
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.range.start < last.range.end || (span.range == last.range) => {
                last.range.end = last.range.end.max(span.range.end);
                last.linewise |= span.linewise;
            }
            _ => merged.push(span),
        }
    }
    merged
}

fn case(op: Operator, text: &str) -> String {
    match op {
        Operator::Lowercase => text.to_lowercase(),
        Operator::Uppercase => text.to_uppercase(),
        _ => text
            .chars()
            .map(|c| if c.is_uppercase() { c.to_lowercase().collect::<String>() } else { c.to_uppercase().collect() })
            .collect(),
    }
}

/// The indent one `>` adds.
fn shift_width(state: &EditorState) -> String {
    let tab = state.mode.tab_size();
    if tab.hard_tabs { "\t".into() } else { " ".repeat(tab.tab_size.max(1)) }
}

/// The edit that indents or outdents `row` once.
fn shift_edit(state: &EditorState, text: &Rope, row: usize, right: bool) -> Option<(Range<usize>, String)> {
    let start = text::line_start(text, row);
    if right {
        return (!text::is_blank_line(text, row) || text::line_start(text, row) != text::line_end(text, row))
            .then(|| (start..start, shift_width(state)))
            .filter(|_| text::line_end(text, row) > start);
    }
    let width = state.mode.tab_size().tab_size.max(1);
    let mut columns = 0;
    let mut end = start;
    for c in text.slice(start..text::line_end(text, row)).chars() {
        if columns >= width {
            break;
        }
        match c {
            ' ' => columns += 1,
            '\t' => columns = width,
            _ => break,
        }
        end += c.len_utf8();
    }
    (end > start).then(|| (start..end, String::new()))
}

impl Vim {
    /// Applies `op` over `spans` (one a cursor, or a block's rows); whether
    /// it started an insert (`c`).
    pub(super) fn operate_spans(
        &mut self,
        state: &mut EditorState,
        op: Operator,
        spans: Vec<Span>,
        block: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) -> bool {
        let text = state.text().clone();
        let register_name = self.pending.register;
        let spans = if block { spans } else { merge(spans) };
        if spans.is_empty() {
            return false;
        }
        let linewise = spans.iter().any(|s| s.linewise);
        let piece = |span: &Span| {
            let mut piece = text.slice(span.range.clone()).to_string();
            if span.linewise && !piece.ends_with('\n') {
                piece.push('\n');
            }
            piece
        };
        let register = if block {
            Register::new(spans.iter().map(|s| text.slice(s.range.clone()).to_string()).collect::<Vec<_>>().join("\n"), RegisterKind::Block)
        } else {
            Register { pieces: spans.iter().map(piece).collect(), kind: if linewise { RegisterKind::Lines } else { RegisterKind::Chars } }
        };
        let first = spans[0].range.start;
        let last_end = spans.last().map_or(first, |s| s.range.end);
        match op {
            Operator::Yank => {
                registers::write(cx, register_name, register, Write::Yank);
                self.set_mark(state, '[', first);
                self.set_mark(state, ']', text::prev(&text, last_end));
                if block {
                    // The cursor is already at the block's top left.
                } else {
                    for (cursor, span) in self.cursors.iter_mut().zip(&spans) {
                        let at = if span.linewise {
                            let top = text::row(&text, span.range.start);
                            if top < text::row(&text, cursor.head) { text::at_column(&text, top, cursor.column, false) } else { cursor.head }
                        } else {
                            span.range.start
                        };
                        *cursor = Cursor { anchor: at, head: at, column: text::column(&text, at) };
                    }
                }
                self.mode = VimMode::Normal;
                false
            }
            Operator::Delete | Operator::Change => {
                let change = op == Operator::Change;
                let mut edits: Vec<(Range<usize>, String)> = spans
                    .iter()
                    .map(|span| {
                        if span.inner_lines && change {
                            let indent = text::indentation(&text, text::row(&text, span.range.start));
                            return (span.range.clone(), format!("{indent}\n"));
                        }
                        if span.linewise && change {
                            let top = text::row(&text, span.range.start);
                            let bottom = text::row(&text, text::prev(&text, span.range.end).max(span.range.start));
                            let indent = text::indentation(&text, top);
                            return (text::line_start(&text, top)..text::line_end(&text, bottom), indent);
                        }
                        let mut range = span.range.clone();
                        // The last line without a newline after it takes the
                        // one before it.
                        if span.linewise && range.end == text.len() && !text.slice(..).to_string().ends_with('\n') && range.start > 0 {
                            range.start -= 1;
                        }
                        (range, String::new())
                    })
                    .collect();
                edits.sort_by_key(|(range, _)| range.start);
                let undo_mark = state.last_transaction();
                // Undo leaves the cursors where the deleted text started.
                let starts: Vec<(usize, usize, bool)> = edits.iter().map(|(r, _)| (r.start, r.start, false)).collect();
                state.vim_write_selections(&starts, None, cx);
                if state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx)).is_none() {
                    return false;
                }
                registers::write(cx, register_name, register, Write::Delete);
                let new_text = state.text().clone();
                let positions: Vec<usize> = edits
                    .iter()
                    .map(|(range, new)| {
                        let at = map_offset(&edits, range.start);
                        if change && (new.ends_with('\n') || !new.is_empty()) {
                            let indent = new.trim_end_matches('\n');
                            at + indent.len()
                        } else {
                            at
                        }
                    })
                    .collect();
                self.set_mark(state, '[', positions[0]);
                self.set_mark(state, ']', positions[0]);
                self.set_mark(state, '.', positions[0]);
                if change {
                    self.begin_insert(state, positions, undo_mark, Repeat::InPlace, false, cx);
                    return true;
                }
                self.mode = VimMode::Normal;
                let positions = if block { positions[..1].to_vec() } else { positions };
                self.cursors = positions
                    .into_iter()
                    .map(|at| {
                        let at = if linewise && !block {
                            let row = text::row(&new_text, at).min(text::last_row(&new_text));
                            text::first_non_blank(&new_text, row)
                        } else {
                            text::clamp_normal(&new_text, at)
                        };
                        Cursor::at(at, &new_text)
                    })
                    .collect();
                false
            }
            Operator::Indent | Operator::Outdent | Operator::Reindent => {
                let mut rows: Vec<usize> = spans
                    .iter()
                    .flat_map(|span| {
                        let top = text::row(&text, span.range.start);
                        let bottom = text::row(&text, text::prev(&text, span.range.end).max(span.range.start));
                        top..=bottom
                    })
                    .collect();
                rows.dedup();
                let top = rows.first().copied().unwrap_or(0);
                if op == Operator::Reindent {
                    for span in &spans {
                        let top = text::row(&text, span.range.start);
                        let bottom = text::row(&text, text::prev(&text, span.range.end).max(span.range.start));
                        cx.emit(VimCommand::Reindent(line_range(&text, top, bottom)));
                    }
                } else {
                    let edits: Vec<(Range<usize>, String)> =
                        rows.iter().filter_map(|&row| shift_edit(state, &text, row, op == Operator::Indent)).collect();
                    let at = text::line_start(&text, top);
                    state.vim_write_selections(&[(at, at, false)], None, cx);
                    if !edits.is_empty() {
                        state.transact(window, cx, |state, window, cx| state.edit(edits, window, cx));
                    }
                }
                let new_text = state.text().clone();
                self.mode = VimMode::Normal;
                self.cursors = vec![Cursor::at(text::first_non_blank(&new_text, top), &new_text)];
                false
            }
            Operator::ToggleCase | Operator::Lowercase | Operator::Uppercase => {
                let edits: Vec<(Range<usize>, String)> = spans
                    .iter()
                    .map(|span| (span.range.clone(), case(op, &text.slice(span.range.clone()).to_string())))
                    .filter(|(range, new)| text.slice(range.clone()).to_string() != *new)
                    .collect();
                let positions: Vec<usize> = spans
                    .iter()
                    .zip(&self.cursors)
                    .map(|(span, cursor)| if span.linewise { cursor.head } else { span.range.start })
                    .collect();
                let first = positions.first().copied().unwrap_or(0);
                state.vim_write_selections(&[(first, first, false)], None, cx);
                if !edits.is_empty() {
                    state.transact(window, cx, |state, window, cx| state.edit(edits, window, cx));
                }
                let new_text = state.text().clone();
                self.mode = VimMode::Normal;
                self.cursors = if block {
                    vec![Cursor::at(first, &new_text)]
                } else {
                    positions.into_iter().map(|at| Cursor::at(text::clamp_normal(&new_text, at), &new_text)).collect()
                };
                false
            }
        }
    }
}

/// `p` and `P` (`gp` / `gP` leave the cursor after the text).
pub(super) fn paste(vim: &mut Vim, state: &mut EditorState, before: bool, cursor_after: bool, count: usize, window: &mut Window, cx: &mut Context<EditorState>) {
    let Some(register) = registers::read(cx, vim.pending.register) else {
        vim.error("E353: Nothing in register");
        return;
    };
    let text = state.text().clone();
    let count = count.max(1);
    let cursors = vim.cursors.clone();
    let piece = |i: usize| -> String {
        if register.pieces.len() == cursors.len() { register.pieces[i].clone() } else { register.text() }
    };
    let mut edits: Vec<(Range<usize>, String)> = vec![];
    // Where each cursor goes in the text it inserted.
    let mut carets = vec![];
    match register.kind {
        RegisterKind::Chars => {
            for (i, cursor) in cursors.iter().enumerate() {
                let row = text::row(&text, cursor.head);
                let at = if before || text::line_start(&text, row) == text::line_end(&text, row) {
                    cursor.head
                } else {
                    text::next(&text, cursor.head).min(text::line_end(&text, row))
                };
                let inserted = piece(i).repeat(count);
                // On the last character pasted (the first, for several
                // lines), or after it with `gp`.
                carets.push(if cursor_after {
                    inserted.len()
                } else if inserted.contains('\n') {
                    0
                } else {
                    inserted.len().saturating_sub(inserted.chars().last().map_or(0, char::len_utf8))
                });
                edits.push((at..at, inserted));
            }
        }
        RegisterKind::Lines => {
            for (i, cursor) in cursors.iter().enumerate() {
                let row = text::row(&text, cursor.head);
                let mut lines = piece(i);
                if !lines.ends_with('\n') {
                    lines.push('\n');
                }
                let lines = lines.repeat(count);
                let (at, inserted) = if before {
                    (text::line_start(&text, row), lines)
                } else if text::line_full_end(&text, row) == text.len() && !text.to_string().ends_with('\n') {
                    // After a last line without a newline.
                    (text.len(), format!("\n{}", lines.trim_end_matches('\n')))
                } else {
                    (text::line_full_end(&text, row), lines)
                };
                carets.push(match (cursor_after, inserted.starts_with('\n')) {
                    (true, _) => inserted.len(),
                    (false, true) => 1,
                    (false, false) => 0,
                });
                edits.push((at..at, inserted));
            }
        }
        RegisterKind::Block => {
            let cursor = cursors[0];
            let row = text::row(&text, cursor.head);
            let line_empty = text::line_start(&text, row) == text::line_end(&text, row);
            let column = text::column(&text, cursor.head) + usize::from(!before && !line_empty);
            let rows: Vec<String> = register.text().split('\n').map(|r| r.repeat(count)).collect();
            let width = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
            let last = text::last_row(&text);
            let mut extra_lines = String::new();
            for (i, piece) in rows.iter().enumerate() {
                let target = row + i;
                if target > last {
                    extra_lines.push('\n');
                    extra_lines.push_str(&" ".repeat(column));
                    extra_lines.push_str(piece);
                    continue;
                }
                let len = text::line_text(&text, target).chars().count();
                let at = text::at_column(&text, target, column.min(len), true);
                let padding = " ".repeat(column.saturating_sub(len));
                // Shorter pieces are padded to the block's width when text
                // follows them.
                let fill = if len > column { " ".repeat(width - piece.chars().count()) } else { String::new() };
                edits.push((at..at, format!("{padding}{piece}{fill}")));
            }
            if !extra_lines.is_empty() {
                let end = text::line_end(&text, last);
                edits.push((end..end, extra_lines));
            }
            edits.sort_by_key(|(r, _)| r.start);
            let at = text::at_column(&text, row, column, true);
            state.vim_write_selections(&[(cursor.head, cursor.head, false)], None, cx);
            if state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx)).is_some() {
                let new_text = state.text().clone();
                let at = map_offset(&edits, at);
                vim.cursors = vec![Cursor::at(text::clamp_normal(&new_text, at), &new_text)];
            }
            return;
        }
    }
    let mut sorted: Vec<(Range<usize>, String)> = edits.clone();
    sorted.sort_by_key(|(r, _)| r.start);
    let heads: Vec<(usize, usize, bool)> = cursors.iter().map(|c| (c.head, c.head, false)).collect();
    state.vim_write_selections(&heads, None, cx);
    if state.transact(window, cx, |state, window, cx| state.edit(sorted.clone(), window, cx)).is_none() {
        return;
    }
    let new_text = state.text().clone();
    vim.cursors = edits
        .iter()
        .zip(&carets)
        .map(|((range, _), caret)| {
            let at = map_offset(&sorted, range.start) + caret;
            let at = if register.kind == RegisterKind::Lines && !cursor_after {
                text::first_non_blank(&new_text, text::row(&new_text, at))
            } else if cursor_after {
                at.min(new_text.len())
            } else {
                text::clamp_normal(&new_text, at)
            };
            Cursor::at(at, &new_text)
        })
        .collect();
    vim.set_mark(state, '[', map_offset(&sorted, sorted[0].0.start));
}

/// Visual `p` (the replaced text goes to the unnamed register) and `P`
/// (it doesn't).
pub(super) fn visual_paste(vim: &mut Vim, state: &mut EditorState, keep_replaced: bool, window: &mut Window, cx: &mut Context<EditorState>) {
    let Some(register) = registers::read(cx, vim.pending.register) else {
        vim.error("E353: Nothing in register");
        return;
    };
    let text = state.text().clone();
    let mode = vim.mode;
    vim.remember_visual(state);
    let mut edits: Vec<(Range<usize>, String)> = vec![];
    let mut replaced = vec![];
    for (i, cursor) in vim.cursors.iter().enumerate() {
        let (start, end) = (cursor.anchor.min(cursor.head), cursor.anchor.max(cursor.head));
        let piece = if register.pieces.len() == vim.cursors.len() { register.pieces[i].clone() } else { register.text() };
        let (range, new) = if mode == VimMode::VisualLine {
            let range = line_range(&text, text::row(&text, start), text::row(&text, end));
            let mut lines = piece;
            if !lines.ends_with('\n') {
                lines.push('\n');
            }
            (range, lines)
        } else {
            let range = start..text::next(&text, end);
            let new = if register.kind == RegisterKind::Lines { format!("\n{piece}") } else { piece };
            (range, new)
        };
        replaced.push(text.slice(range.clone()).to_string());
        edits.push((range, new));
    }
    edits.sort_by_key(|(r, _)| r.start);
    let starts: Vec<(usize, usize, bool)> = edits.iter().map(|(r, _)| (r.start, r.start, false)).collect();
    vim.mode = VimMode::Normal;
    state.vim_write_selections(&starts, None, cx);
    if state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx)).is_none() {
        return;
    }
    if keep_replaced {
        let kind = if mode == VimMode::VisualLine { RegisterKind::Lines } else { RegisterKind::Chars };
        registers::write(cx, None, Register { pieces: replaced, kind }, Write::Delete);
    }
    let new_text = state.text().clone();
    vim.cursors = edits
        .iter()
        .map(|(range, _)| Cursor::at(text::clamp_normal(&new_text, map_offset(&edits, range.start)), &new_text))
        .collect();
}

/// `J` (`spaces`) and `gJ`: rows `top` to `bottom` joined into one line.
pub(super) fn join(vim: &mut Vim, state: &mut EditorState, top: usize, bottom: usize, spaces: bool, window: &mut Window, cx: &mut Context<EditorState>) {
    let text = state.text().clone();
    let bottom = bottom.max(top + 1).min(text::last_row(&text));
    if bottom <= top {
        return;
    }
    let mut edits = vec![];
    for row in top..bottom {
        let end = text::line_end(&text, row);
        if !spaces {
            edits.push((end..text::line_full_end(&text, row), String::new()));
            continue;
        }
        let next = row + 1;
        let next_start = text::first_non_blank(&text, next);
        let line = text::line_text(&text, row);
        let rest = text::line_text(&text, next);
        let rest = rest.trim_start();
        let space = if line.is_empty() || line.ends_with([' ', '\t']) || rest.is_empty() || rest.starts_with(')') { "" } else { " " };
        edits.push((end..next_start, space.to_string()));
    }
    let at = edits.last().map_or(0, |(r, _): &(Range<usize>, String)| r.start);
    let head = vim.primary().head;
    state.vim_write_selections(&[(head, head, false)], None, cx);
    if state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx)).is_none() {
        return;
    }
    let new_text = state.text().clone();
    let at = map_offset(&edits, at);
    vim.mode = VimMode::Normal;
    vim.cursors = vec![Cursor::at(text::clamp_normal(&new_text, at), &new_text)];
}

/// `r`: the `count` characters from the cursor replaced with `c` (a
/// Return splits the line instead); false when the line is too short.
pub(super) fn replace_chars(vim: &mut Vim, state: &mut EditorState, c: char, count: usize, window: &mut Window, cx: &mut Context<EditorState>) -> bool {
    let text = state.text().clone();
    let mut edits = vec![];
    for cursor in &vim.cursors {
        let end = text::line_end(&text, text::row(&text, cursor.head));
        let mut to = cursor.head;
        for _ in 0..count {
            if to >= end {
                return false;
            }
            to = text::next(&text, to);
        }
        let new = if c == '\n' { "\n".to_string() } else { c.to_string().repeat(count) };
        edits.push((cursor.head..to, new));
    }
    edits.sort_by_key(|(r, _)| r.start);
    let heads: Vec<(usize, usize, bool)> = vim.cursors.iter().map(|c| (c.head, c.head, false)).collect();
    state.vim_write_selections(&heads, None, cx);
    if state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx)).is_none() {
        return false;
    }
    let new_text = state.text().clone();
    vim.cursors = edits
        .iter()
        .map(|(range, new)| {
            let start = map_offset(&edits, range.start);
            let at = if c == '\n' { start + 1 } else { start + new.len() - c.len_utf8() };
            Cursor::at(text::clamp_normal(&new_text, at), &new_text)
        })
        .collect();
    true
}

/// Visual `r`: every character selected (not the line breaks) becomes `c`.
pub(super) fn replace_selection(vim: &mut Vim, state: &mut EditorState, c: char, window: &mut Window, cx: &mut Context<EditorState>) {
    let text = state.text().clone();
    let ranges: Vec<Range<usize>> = match vim.mode {
        VimMode::VisualBlock => super::block_rows(&text, vim.primary(), vim.block_to_end),
        VimMode::VisualLine => vim
            .cursors
            .iter()
            .map(|cursor| {
                let (a, h) = (text::row(&text, cursor.anchor), text::row(&text, cursor.head));
                text::line_start(&text, a.min(h))..text::line_end(&text, a.max(h))
            })
            .collect(),
        _ => vim
            .cursors
            .iter()
            .map(|cursor| cursor.anchor.min(cursor.head)..text::next(&text, cursor.anchor.max(cursor.head)))
            .collect(),
    };
    let start = ranges.iter().map(|r| r.start).min().unwrap_or(0);
    let edits: Vec<(Range<usize>, String)> = ranges
        .iter()
        .map(|range| {
            let new: String = text.slice(range.clone()).chars().map(|ch| if ch == '\n' { '\n' } else { c }).collect();
            (range.clone(), new)
        })
        .collect();
    vim.remember_visual(state);
    vim.mode = VimMode::Normal;
    state.vim_write_selections(&[(start, start, false)], None, cx);
    state.transact(window, cx, |state, window, cx| state.edit(edits, window, cx));
    let new_text = state.text().clone();
    vim.cursors = vec![Cursor::at(text::clamp_normal(&new_text, start), &new_text)];
    vim.block_to_end = false;
}

/// `~` in normal mode: the case of `count` characters flips and the cursor
/// moves past them.
pub(super) fn toggle_case_chars(vim: &mut Vim, state: &mut EditorState, count: usize, window: &mut Window, cx: &mut Context<EditorState>) {
    let text = state.text().clone();
    let mut edits = vec![];
    let mut ends = vec![];
    for cursor in &vim.cursors {
        let end = text::line_end(&text, text::row(&text, cursor.head));
        let mut to = cursor.head;
        for _ in 0..count {
            if to >= end {
                break;
            }
            to = text::next(&text, to);
        }
        if to > cursor.head {
            edits.push((cursor.head..to, case(Operator::ToggleCase, &text.slice(cursor.head..to).to_string())));
        }
        ends.push(to);
    }
    let heads: Vec<(usize, usize, bool)> = vim.cursors.iter().map(|c| (c.head, c.head, false)).collect();
    state.vim_write_selections(&heads, None, cx);
    if !edits.is_empty() {
        state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx));
    }
    let new_text = state.text().clone();
    vim.cursors = ends.into_iter().map(|end| Cursor::at(text::clamp_normal(&new_text, map_offset(&edits, end)), &new_text)).collect();
}

/// `⌃A` / `⌃X`: the number at or after the cursor on its line plus `delta`.
pub(super) fn increment(vim: &mut Vim, state: &mut EditorState, delta: i64, window: &mut Window, cx: &mut Context<EditorState>) -> bool {
    let text = state.text().clone();
    let mut edits = vec![];
    for cursor in &vim.cursors {
        let row = text::row(&text, cursor.head);
        let start = text::line_start(&text, row);
        let line = text::line_text(&text, row);
        let col = cursor.head - start;
        let Some((range, value, hex)) = number_at(&line, col) else { continue };
        let new = if hex {
            let width = range.len() - 2;
            format!("0x{:0width$x}", (value as u64).wrapping_add(delta as u64), width = width)
        } else {
            (value + delta).to_string()
        };
        edits.push((start + range.start..start + range.end, new));
    }
    if edits.is_empty() {
        return false;
    }
    let heads: Vec<(usize, usize, bool)> = vim.cursors.iter().map(|c| (c.head, c.head, false)).collect();
    state.vim_write_selections(&heads, None, cx);
    if state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx)).is_none() {
        return false;
    }
    let new_text = state.text().clone();
    vim.cursors = edits
        .iter()
        .map(|(range, new)| Cursor::at(map_offset(&edits, range.start) + new.len() - 1, &new_text))
        .collect();
    true
}

/// The number under or after byte `col` of `line`: its range, value and
/// whether it's hexadecimal.
fn number_at(line: &str, col: usize) -> Option<(Range<usize>, i64, bool)> {
    let bytes = line.as_bytes();
    // A hex number the cursor is on or before.
    let lower = line.to_ascii_lowercase();
    let mut search = 0;
    while let Some(i) = lower[search..].find("0x").map(|i| i + search) {
        let end = i + 2 + bytes[i + 2..].iter().take_while(|b| b.is_ascii_hexdigit()).count();
        if end > i + 2 && end > col {
            let value = i64::from_str_radix(&line[i + 2..end], 16).ok()?;
            return Some((i..end, value, true));
        }
        search = i + 2;
    }
    let mut start = col.min(bytes.len());
    // Back to the number's start when the cursor is inside one.
    while start > 0 && bytes[start - 1].is_ascii_digit() && start <= bytes.len() && (start == bytes.len() || bytes[start].is_ascii_digit()) {
        start -= 1;
    }
    let first = (start..bytes.len()).find(|&i| bytes[i].is_ascii_digit())?;
    let mut begin = first;
    while begin > 0 && bytes[begin - 1].is_ascii_digit() {
        begin -= 1;
    }
    let end = first + bytes[first..].iter().take_while(|b| b.is_ascii_digit()).count();
    let negative = begin > 0 && bytes[begin - 1] == b'-';
    let begin = if negative { begin - 1 } else { begin };
    let value: i64 = line[begin..end].parse().ok()?;
    Some((begin..end, value, false))
}

/// `o` / `O`: a new line below (indented as Return would) or above (as the
/// current line); where each cursor goes in it.
pub(super) fn open_lines(vim: &mut Vim, state: &mut EditorState, above: bool, window: &mut Window, cx: &mut Context<EditorState>) -> Option<Vec<usize>> {
    let text = state.text().clone();
    let mut edits: Vec<(Range<usize>, String)> = vim
        .cursors
        .iter()
        .map(|cursor| {
            let row = text::row(&text, cursor.head);
            if above {
                let start = text::line_start(&text, row);
                (start..start, format!("{}\n", text::indentation(&text, row)))
            } else {
                let end = text::line_end(&text, row);
                (end..end, format!("\n{}", state.indent_of_next_line_at(end)))
            }
        })
        .collect();
    edits.sort_by_key(|(r, _)| r.start);
    edits.dedup_by_key(|(r, _)| r.start);
    let heads: Vec<(usize, usize, bool)> = vim.cursors.iter().map(|c| (c.head, c.head, false)).collect();
    state.vim_write_selections(&heads, None, cx);
    state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx))?;
    Some(
        edits
            .iter()
            .map(|(range, new)| map_offset(&edits, range.start) + if above { new.len() - 1 } else { new.len() })
            .collect(),
    )
}

/// What `.` replays of an insert, at every cursor.
pub(super) fn apply_typed(state: &mut EditorState, typed: &Typed, window: &mut Window, cx: &mut Context<EditorState>) {
    let text = state.text().clone();
    let mut edits: Vec<(Range<usize>, String)> = state
        .selections
        .iter()
        .map(|s| {
            let at = s.cursor_offset();
            let start = text.floor_char_boundary(at.saturating_sub(typed.before));
            let end = text.ceil_char_boundary((at + typed.after).min(text.len()));
            (start..end, typed.text.clone())
        })
        .collect();
    edits.sort_by_key(|(r, _)| r.start);
    if state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx)).is_none() {
        return;
    }
    let ends: Vec<(usize, usize, bool)> = edits
        .iter()
        .map(|(range, new)| {
            let at = map_offset(&edits, range.start) + typed.caret.min(new.len());
            (at, at, false)
        })
        .collect();
    state.vim_write_selections(&ends, None, cx);
}

/// Insert mode's `⌃W` and `⌃U`: from where `from` says to each cursor.
pub(super) fn delete_before_cursors(
    state: &mut EditorState,
    from: impl Fn(&Rope, usize) -> usize,
    window: &mut Window,
    cx: &mut Context<EditorState>,
) {
    let text = state.text().clone();
    let mut edits: Vec<(Range<usize>, String)> = state
        .selections
        .iter()
        .map(|s| {
            let at = s.cursor_offset();
            (from(&text, at).min(at)..at, String::new())
        })
        .filter(|(r, _)| !r.is_empty())
        .collect();
    edits.sort_by_key(|(r, _)| r.start);
    if edits.is_empty() {
        return;
    }
    if state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx)).is_some() {
        let at: Vec<(usize, usize, bool)> = edits.iter().map(|(r, _)| map_offset(&edits, r.start)).map(|o| (o, o, false)).collect();
        state.vim_write_selections(&at, None, cx);
    }
}

/// Insert mode's `⌃T` and `⌃D`: the cursors' lines indented or outdented.
pub(super) fn shift_cursor_lines(state: &mut EditorState, right: bool, window: &mut Window, cx: &mut Context<EditorState>) {
    let text = state.text().clone();
    let cursors: Vec<usize> = state.selections.iter().map(|s| s.cursor_offset()).collect();
    let mut rows: Vec<usize> = cursors.iter().map(|&c| text::row(&text, c)).collect();
    rows.sort_unstable();
    rows.dedup();
    let edits: Vec<(Range<usize>, String)> = rows
        .iter()
        .filter_map(|&row| {
            if right {
                let start = text::line_start(&text, row);
                Some((start..start, shift_width(state)))
            } else {
                shift_edit(state, &text, row, false)
            }
        })
        .collect();
    if edits.is_empty() {
        return;
    }
    if state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx)).is_some() {
        // The cursors stay on their characters, after an indent added at the
        // line's start.
        let at: Vec<(usize, usize, bool)> = cursors
            .iter()
            .map(|&c| {
                let shift: isize = edits
                    .iter()
                    .take_while(|(r, _)| r.start <= c)
                    .map(|(r, new)| new.len() as isize - (r.end.min(c) - r.start) as isize)
                    .sum();
                (c as isize + shift) as usize
            })
            .map(|o| (o, o, false))
            .collect();
        state.vim_write_selections(&at, None, cx);
    }
}

/// Replace mode: `c` over the character at each cursor (or after the end
/// of the line). Returns what each typed character overwrote: where it is
/// now, its length and the character it replaced.
pub(super) fn overwrite(state: &mut EditorState, c: char, window: &mut Window, cx: &mut Context<EditorState>) -> Vec<(usize, Option<char>)> {
    let text = state.text().clone();
    let mut cursors: Vec<usize> = state.selections.iter().map(|s| s.cursor_offset()).collect();
    cursors.sort_unstable();
    let mut old = vec![];
    let edits: Vec<(Range<usize>, String)> = cursors
        .iter()
        .map(|&at| {
            let ch = text::char_at(&text, at).filter(|ch| *ch != '\n');
            old.push(ch);
            let end = if ch.is_some() { text::next(&text, at) } else { at };
            (at..end, c.to_string())
        })
        .collect();
    if state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx)).is_none() {
        return vec![];
    }
    let starts: Vec<usize> = edits.iter().map(|(r, _)| map_offset(&edits, r.start)).collect();
    let after: Vec<(usize, usize, bool)> = starts.iter().map(|&o| (o + c.len_utf8(), o + c.len_utf8(), false)).collect();
    state.vim_write_selections(&after, None, cx);
    starts.into_iter().zip(old).collect()
}

/// Backspace in Replace mode: what the last typed characters overwrote
/// comes back.
pub(super) fn restore_overwritten(state: &mut EditorState, replaced: Vec<(usize, Option<char>)>, window: &mut Window, cx: &mut Context<EditorState>) {
    let text = state.text().clone();
    let edits: Vec<(Range<usize>, String)> = replaced
        .iter()
        .map(|&(at, old)| (at..text::next(&text, at), old.map(String::from).unwrap_or_default()))
        .collect();
    if state.transact(window, cx, |state, window, cx| state.edit(edits.clone(), window, cx)).is_some() {
        let at: Vec<(usize, usize, bool)> = edits.iter().map(|(r, _)| map_offset(&edits, r.start)).map(|o| (o, o, false)).collect();
        state.vim_write_selections(&at, None, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_motions_ending_at_a_line_start() {
        let text = Rope::from_str("one\ntwo\n\nthree");
        let target = |offset| Target { offset, kind: Kind::Exclusive };
        // `d}` from the first line's start: lines.
        assert_eq!(motion_span(&text, 0, target(8)), Some(Span::lines(0..8)));
        // From the middle of a line: up to that line's end.
        assert_eq!(motion_span(&text, 1, target(8)), Some(Span::chars(1..7)));
        assert_eq!(motion_span(&text, 4, target(4)), None);
    }

    #[test]
    fn offsets_move_through_edits() {
        let edits = vec![(2..4, "xyz".to_string()), (6..6, "!".to_string())];
        assert_eq!(map_offset(&edits, 1), 1);
        assert_eq!(map_offset(&edits, 2), 2);
        assert_eq!(map_offset(&edits, 3), 3, "inside a replaced range");
        assert_eq!(map_offset(&edits, 5), 6);
        assert_eq!(map_offset(&edits, 7), 9);
    }

    #[test]
    fn numbers_under_or_after_the_cursor() {
        assert_eq!(number_at("x = 41;", 0), Some((4..6, 41, false)));
        assert_eq!(number_at("x = 41;", 5), Some((4..6, 41, false)));
        assert_eq!(number_at("a -5 b", 0), Some((2..4, -5, false)));
        assert_eq!(number_at("c 0xff", 0), Some((2..6, 255, true)));
        assert_eq!(number_at("none", 0), None);
    }
}

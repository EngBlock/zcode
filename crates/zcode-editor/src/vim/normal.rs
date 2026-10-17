//! Normal and visual mode: counts, registers, operators with motions and
//! text objects, and the commands that aren't either.

use gpui::{Context, Window, point};

use super::keys::Key;
use super::motion::{self, Kind, LastFind, Motion};
use super::object::{self, Object};
use super::ops::{self, Operator, Span};
use super::text;
use super::{Change, Cursor, Repeat, Vim, VimCommand, VimMode, VisualExtent, registers, search};
use crate::EditorState;

/// A command being typed.
#[derive(Clone, Debug, Default)]
pub(super) struct Pending {
    pub(super) register: Option<char>,
    pub(super) count: Option<usize>,
    pub(super) operator: Option<Operator>,
    /// A count typed after the operator (`d3w`); it multiplies the first.
    pub(super) operator_count: Option<usize>,
    pub(super) prefix: Option<Prefix>,
    /// The command's keys without its counts and register, for `.`.
    pub(super) keys: Vec<Key>,
    /// The keys as typed, for the status bar.
    pub(super) shown: String,
    /// Started in visual mode: the selection's size, for `.`.
    pub(super) visual: Option<VisualExtent>,
}

impl Pending {
    pub(super) fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    pub(super) fn count(&self) -> Option<usize> {
        match (self.count, self.operator_count) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(1) * b.unwrap_or(1)),
        }
    }

    fn n(&self) -> usize {
        self.count().unwrap_or(1).max(1)
    }
}

/// A key that waits for another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Prefix {
    G,
    Z,
    BigZ,
    Register,
    Find { forward: bool, till: bool },
    Replace,
    SetMark,
    Mark { line: bool },
    Object { around: bool },
    Bracket,
}

fn simple_motion(key: Key) -> Option<Motion> {
    Some(match key {
        Key::Char('h') | Key::Left | Key::Backspace | Key::Ctrl('h') => Motion::Left,
        Key::Char('l') | Key::Right | Key::Char(' ') => Motion::Right,
        Key::Char('j') | Key::Down | Key::Ctrl('n') | Key::Ctrl('j') => Motion::Down,
        Key::Char('k') | Key::Up | Key::Ctrl('p') => Motion::Up,
        Key::Char('w') => Motion::WordStart { big: false },
        Key::Char('W') => Motion::WordStart { big: true },
        Key::Char('b') => Motion::WordBack { big: false },
        Key::Char('B') => Motion::WordBack { big: true },
        Key::Char('e') => Motion::WordEnd { big: false },
        Key::Char('E') => Motion::WordEnd { big: true },
        Key::Char('0') | Key::Home => Motion::LineStart,
        Key::Char('^') => Motion::FirstNonBlank,
        Key::Char('$') | Key::End => Motion::LineEnd,
        Key::Char('|') => Motion::Column,
        Key::Char('G') => Motion::LastLine,
        Key::Char('}') => Motion::ParagraphForward,
        Key::Char('{') => Motion::ParagraphBack,
        Key::Char(';') => Motion::RepeatFind { reverse: false },
        Key::Char(',') => Motion::RepeatFind { reverse: true },
        Key::Char('%') => Motion::Match,
        Key::Char('H') => Motion::ScreenTop,
        Key::Char('M') => Motion::ScreenMiddle,
        Key::Char('L') => Motion::ScreenBottom,
        Key::Char('+') | Key::Enter | Key::Ctrl('m') => Motion::NextLine,
        Key::Char('-') => Motion::PreviousLine,
        Key::Char('_') => Motion::CurrentLine,
        Key::Char('n') => Motion::Search { reverse: false },
        Key::Char('N') => Motion::Search { reverse: true },
        _ => return None,
    })
}

impl Vim {
    pub(super) fn normal_key(
        &mut self,
        state: &mut EditorState,
        key: Key,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) -> bool {
        if matches!(key, Key::Escape | Key::Ctrl('c')) {
            return self.escape(state, cx);
        }
        if self.pending.is_empty() && self.mode.is_visual() {
            self.pending.visual = Some(self.visual_extent(state));
        }
        self.pending.shown.push_str(&key.label());
        let handled = if let Some(prefix) = self.pending.prefix.take() {
            self.pending.keys.push(key);
            self.prefixed(state, prefix, key, window, cx)
        } else {
            match key {
                Key::Char(c @ '0'..='9')
                    if c != '0'
                        || (self.pending.operator.is_none() && self.pending.count.is_some())
                        || self.pending.operator_count.is_some() =>
                {
                    let digit = c as usize - '0' as usize;
                    let count = if self.pending.operator.is_some() {
                        &mut self.pending.operator_count
                    } else {
                        &mut self.pending.count
                    };
                    *count = Some(count.unwrap_or(0).saturating_mul(10).saturating_add(digit).min(99_999));
                    true
                }
                Key::Char('"') => {
                    self.pending.prefix = Some(Prefix::Register);
                    true
                }
                _ => {
                    self.pending.keys.push(key);
                    self.command(state, key, window, cx)
                }
            }
        };
        if matches!(self.mode, VimMode::Insert | VimMode::Replace) || self.command_line.is_some() {
            self.apply_mode(state, cx);
        } else {
            self.show(state, cx);
        }
        handled
    }

    fn escape(&mut self, state: &mut EditorState, cx: &mut Context<EditorState>) -> bool {
        if !self.pending.is_empty() {
            self.pending = Default::default();
            self.apply_mode(state, cx);
            return true;
        }
        if self.mode.is_visual() {
            self.exit_visual(state);
            self.show(state, cx);
            return true;
        }
        if self.cursors.len() > 1 {
            self.cursors.truncate(1);
            self.show(state, cx);
            return true;
        }
        if state.search_session.is_active() {
            state.close_search(cx);
            return true;
        }
        // Nothing of Vim's: popovers and the window get it.
        false
    }

    pub(super) fn exit_visual(&mut self, state: &mut EditorState) {
        self.remember_visual(state);
        self.mode = VimMode::Normal;
        let text = state.text().clone();
        for cursor in &mut self.cursors {
            *cursor = Cursor { anchor: cursor.head, ..*cursor };
            cursor.head = text::clamp_normal(&text, cursor.head);
            cursor.anchor = cursor.head;
        }
        if self.cursors.len() > 1 && self.block_to_end {
            self.cursors.truncate(1);
        }
        self.block_to_end = false;
    }

    fn visual_extent(&self, state: &EditorState) -> VisualExtent {
        let text = state.text();
        let cursor = self.primary();
        let (start, end) = (cursor.anchor.min(cursor.head), cursor.anchor.max(cursor.head));
        let rows = text::row(text, end) - text::row(text, start) + 1;
        let width = match self.mode {
            VimMode::VisualBlock => text::column(text, cursor.anchor).abs_diff(text::column(text, cursor.head)) + 1,
            _ if rows == 1 => text.slice(start..text::next(text, end)).chars().count(),
            _ => text::column(text, end) + 1,
        };
        VisualExtent { mode: self.mode, rows, width }
    }

    /// The change being finished, for `.`.
    fn change(&self) -> Option<Change> {
        (!self.replaying).then(|| Change {
            keys: self.pending.keys.clone(),
            count: self.pending.count(),
            register: self.pending.register,
            typed: None,
            visual: self.pending.visual,
        })
    }

    fn finish(&mut self, changed: bool) {
        if changed && let Some(change) = self.change() {
            self.last_change = Some(change);
        }
        self.pending = Default::default();
    }

    fn command(&mut self, state: &mut EditorState, key: Key, window: &mut Window, cx: &mut Context<EditorState>) -> bool {
        let visual = self.mode.is_visual();
        let operator = self.pending.operator;
        if let Some(motion) = simple_motion(key) {
            // `cw` on a word is `ce`.
            let motion = match (operator, motion) {
                (Some(Operator::Change), Motion::WordStart { big }) => {
                    let head = self.primary().head;
                    let on_blank = text::char_at(state.text(), head).is_none_or(|c| c.is_whitespace());
                    if on_blank { motion } else { Motion::WordEnd { big } }
                }
                _ => motion,
            };
            self.run_motion(state, motion, window, cx);
            return true;
        }
        let set_prefix = |vim: &mut Vim, prefix| {
            vim.pending.prefix = Some(prefix);
            true
        };
        match key {
            Key::Char('g') => return set_prefix(self, Prefix::G),
            Key::Char('z') if operator.is_none() => return set_prefix(self, Prefix::Z),
            Key::Char('Z') if operator.is_none() && !visual => return set_prefix(self, Prefix::BigZ),
            Key::Char(c @ ('f' | 'F' | 't' | 'T')) => {
                return set_prefix(self, Prefix::Find { forward: c.is_lowercase(), till: c.eq_ignore_ascii_case(&'t') });
            }
            Key::Char('`') => return set_prefix(self, Prefix::Mark { line: false }),
            Key::Char('\'') => return set_prefix(self, Prefix::Mark { line: true }),
            Key::Char(c @ ('i' | 'a')) if operator.is_some() || visual => {
                return set_prefix(self, Prefix::Object { around: c == 'a' });
            }
            Key::Char('[' | ']') => return set_prefix(self, Prefix::Bracket),
            Key::Char(c @ ('*' | '#')) => {
                self.search_word(state, c == '*', true, window, cx);
                return true;
            }
            Key::Char(c @ ('/' | '?')) => {
                let pending = operator.map(|_| std::mem::take(&mut self.pending));
                if pending.is_none() && !visual {
                    self.pending = Default::default();
                }
                if visual {
                    let keep = std::mem::take(&mut self.pending);
                    self.open_command_line(c, "", Some(keep), cx);
                } else {
                    self.open_command_line(c, "", pending, cx);
                }
                return true;
            }
            _ => {}
        }
        if let Some(op) = operator_for(key) {
            self.operator_key(state, op, window, cx);
            return true;
        }
        match (operator, key) {
            (Some(Operator::ToggleCase), Key::Char('~'))
            | (Some(Operator::Lowercase), Key::Char('u'))
            | (Some(Operator::Uppercase), Key::Char('U')) => {
                let op = operator.unwrap();
                self.operator_key(state, op, window, cx);
                return true;
            }
            (Some(_), _) => {
                // Not a motion or object: the operator is canceled.
                self.pending = Default::default();
                return true;
            }
            _ => {}
        }
        if visual {
            self.visual_command(state, key, window, cx)
        } else {
            self.normal_command(state, key, window, cx)
        }
    }

    fn operator_key(&mut self, state: &mut EditorState, op: Operator, window: &mut Window, cx: &mut Context<EditorState>) {
        if self.mode.is_visual() {
            let times = if matches!(op, Operator::Indent | Operator::Outdent) { self.pending.n() } else { 1 };
            self.operate_visual(state, op, false, times, window, cx);
            return;
        }
        match self.pending.operator {
            Some(pending) if pending == op => {
                // `dd`, `3yy`, `>>`: whole lines.
                let text = state.text().clone();
                let n = self.pending.n();
                let spans: Vec<Span> = self
                    .cursors
                    .iter()
                    .map(|c| {
                        let row = text::row(&text, c.head);
                        Span::lines(ops::line_range(&text, row, (row + n - 1).min(text::last_row(&text))))
                    })
                    .collect();
                let changed = op != Operator::Yank;
                let entered = self.operate_spans(state, op, spans, false, window, cx);
                if !entered {
                    self.finish(changed);
                }
            }
            Some(_) => self.pending = Default::default(),
            None => {
                self.pending.operator = Some(op);
                self.apply_mode(state, cx);
            }
        }
    }

    /// Moves the cursors, extends the visual selection, or hands the motion
    /// to the pending operator.
    pub(super) fn run_motion(
        &mut self,
        state: &mut EditorState,
        motion: Motion,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        let text = state.text().clone();
        let count = self.pending.count();
        let operator = self.pending.operator;
        let regex = match motion {
            Motion::Search { .. } => match &self.last_search {
                Some((pattern, _)) => match search::compile(pattern) {
                    Ok(regex) => Some(regex),
                    Err(message) => {
                        self.error(message);
                        self.pending = Default::default();
                        return;
                    }
                },
                None => {
                    self.error("E35: No previous regular expression");
                    self.pending = Default::default();
                    return;
                }
            },
            _ => None,
        };
        let forward = self.last_search.as_ref().is_none_or(|(_, forward)| *forward);
        let mut wrapped = false;
        let wrapped_cell = std::cell::Cell::new(false);
        let search = |from: usize, reverse: bool| {
            let regex = regex.as_ref()?;
            let (range, wrap) = search::find(&text, regex, from, forward != reverse)?;
            if wrap {
                wrapped_cell.set(true);
            }
            Some(range.start)
        };
        let visible = state.visible_rows().map(|r| r.start as usize..r.end as usize);
        let hidden = |row: usize| state.display_map.is_buffer_line_hidden(row);
        let marks = |name: char| self.mark(state, name);
        let targets: Vec<Option<motion::Target>> = self
            .cursors
            .iter()
            .map(|cursor| {
                let env = motion::Env {
                    text: &text,
                    count,
                    column: cursor.column,
                    visible_rows: visible.clone(),
                    last_find: self.last_find,
                    hidden: &hidden,
                    search: &search,
                    mark: &marks,
                    operator: operator.is_some(),
                };
                let target = motion::eval(motion, cursor.head, &env)?;
                // `dw` over the last word of a line stops at the line's end.
                if operator.is_some()
                    && let Motion::WordStart { .. } = motion
                    && target.offset > cursor.head
                {
                    let row = text::row(&text, cursor.head);
                    let target_row = text::row(&text, target.offset);
                    let end = text::line_end(&text, row);
                    if target_row > row && end > cursor.head && !text::is_blank_line(&text, row) {
                        let last_word_row = text::row(&text, text::prev(&text, target.offset));
                        let end = text::line_end(&text, last_word_row.max(row));
                        return Some(motion::Target { offset: end, kind: Kind::Exclusive });
                    }
                }
                Some(target)
            })
            .collect();
        wrapped |= wrapped_cell.get();
        if let Motion::Find { c, forward, till } = motion {
            self.last_find = Some(LastFind { c, forward, till });
        }
        if targets.iter().all(Option::is_none) {
            if let (Motion::Search { .. }, Some((pattern, _))) = (motion, &self.last_search) {
                self.error(format!("E486: Pattern not found: {pattern}"));
            }
            self.pending = Default::default();
            return;
        }
        if matches!(motion, Motion::Search { .. }) {
            self.highlight_search(state, true, cx);
            if wrapped {
                let bottom = forward != matches!(motion, Motion::Search { reverse: true });
                self.say(if bottom { "search hit BOTTOM, continuing at TOP" } else { "search hit TOP, continuing at BOTTOM" });
            }
        }
        if let Some(op) = operator {
            let spans: Vec<Span> = self
                .cursors
                .iter()
                .zip(&targets)
                .filter_map(|(cursor, target)| ops::motion_span(&text, cursor.head, (*target)?))
                .collect();
            let changed = op != Operator::Yank;
            let entered = self.operate_spans(state, op, spans, false, window, cx);
            if !entered {
                self.finish(changed);
            }
            return;
        }
        if motion.is_jump() && !self.cursors.is_empty() {
            let head = self.primary().head;
            self.set_mark(state, '\'', head);
        }
        let block = self.mode == VimMode::VisualBlock;
        for (cursor, target) in self.cursors.iter_mut().zip(targets) {
            let Some(target) = target else { continue };
            cursor.head = target.offset;
            if !motion.keeps_column() {
                cursor.column = if motion == Motion::LineEnd { usize::MAX } else { text::column(&text, target.offset) };
            }
            if !self.mode.is_visual() {
                cursor.anchor = cursor.head;
            }
        }
        if block {
            self.block_to_end = motion == Motion::LineEnd || (self.block_to_end && motion.keeps_column());
        }
        self.pending = Default::default();
    }

    fn prefixed(
        &mut self,
        state: &mut EditorState,
        prefix: Prefix,
        key: Key,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) -> bool {
        let c = match key {
            Key::Char(c) => Some(c),
            Key::Enter => Some('\n'),
            _ => None,
        };
        match (prefix, c) {
            (Prefix::Register, Some(c)) if registers::is_register(c) => {
                self.pending.register = Some(c);
            }
            (Prefix::Find { forward, till }, Some(c)) if c != '\n' => {
                self.run_motion(state, Motion::Find { c, forward, till }, window, cx);
            }
            (Prefix::Mark { line }, Some(name)) => self.run_motion(state, Motion::Mark { name, line }, window, cx),
            (Prefix::SetMark, Some(name)) if name.is_ascii_alphabetic() || "'`<>[]".contains(name) => {
                let head = self.primary().head;
                self.set_mark(state, if name == '`' { '\'' } else { name }, head);
                self.pending = Default::default();
            }
            (Prefix::Object { around }, Some(c)) => match Object::from_char(c) {
                Some(object) => self.select_object(state, object, around, window, cx),
                None => self.pending = Default::default(),
            },
            (Prefix::Replace, Some(c)) => {
                if self.mode.is_visual() {
                    ops::replace_selection(self, state, c, window, cx);
                    self.finish(true);
                } else {
                    let n = self.pending.n();
                    if ops::replace_chars(self, state, c, n, window, cx) {
                        self.finish(true);
                    } else {
                        self.pending = Default::default();
                    }
                }
            }
            (Prefix::G, Some(c)) => return self.g_command(state, c, window, cx),
            (Prefix::Z, Some(c)) => {
                self.z_command(state, c, cx);
                self.pending = Default::default();
            }
            (Prefix::BigZ, Some('Z')) => {
                cx.emit(VimCommand::Write { all: false, quit: true });
                self.pending = Default::default();
            }
            (Prefix::BigZ, Some('Q')) => {
                cx.emit(VimCommand::Quit { all: false, force: true });
                self.pending = Default::default();
            }
            _ => self.pending = Default::default(),
        }
        true
    }

    fn g_command(&mut self, state: &mut EditorState, c: char, window: &mut Window, cx: &mut Context<EditorState>) -> bool {
        let motion = match c {
            'g' => Some(Motion::FirstLine),
            'e' => Some(Motion::WordEndBack { big: false }),
            'E' => Some(Motion::WordEndBack { big: true }),
            '_' => Some(Motion::LastNonBlank),
            'j' => Some(Motion::Down),
            'k' => Some(Motion::Up),
            '0' => Some(Motion::LineStart),
            '^' => Some(Motion::FirstNonBlank),
            '$' => Some(Motion::LineEnd),
            _ => None,
        };
        if let Some(motion) = motion {
            self.run_motion(state, motion, window, cx);
            return true;
        }
        match c {
            '~' => self.operator_key(state, Operator::ToggleCase, window, cx),
            'u' => self.operator_key(state, Operator::Lowercase, window, cx),
            'U' => self.operator_key(state, Operator::Uppercase, window, cx),
            _ if self.pending.operator.is_some() => self.pending = Default::default(),
            'v' => {
                if let Some((mode, cursors, to_end)) = self.last_visual.clone() {
                    let len = state.text().len();
                    self.mode = mode;
                    self.cursors = cursors
                        .into_iter()
                        .map(|c| Cursor { anchor: c.anchor.min(len), head: c.head.min(len), column: c.column })
                        .collect();
                    self.block_to_end = to_end;
                }
                self.pending = Default::default();
            }
            'J' => {
                let text = state.text().clone();
                let (top, bottom) = self.join_rows(&text);
                ops::join(self, state, top, bottom, false, window, cx);
                self.finish(true);
            }
            'p' | 'P' if !self.mode.is_visual() => {
                let n = self.pending.n();
                ops::paste(self, state, c == 'P', true, n, window, cx);
                self.finish(true);
            }
            'i' if !self.mode.is_visual() => {
                let at = self.mark(state, '^').unwrap_or(self.primary().head);
                self.insert_at(state, vec![at], Repeat::InPlace, window, cx);
            }
            'I' if !self.mode.is_visual() => {
                let text = state.text().clone();
                let at = self.cursors.iter().map(|c| text::line_start(&text, text::row(&text, c.head))).collect();
                self.insert_at(state, at, Repeat::InPlace, window, cx);
            }
            'd' | 'D' => {
                window.dispatch_action(Box::new(crate::GoToDefinition), cx);
                self.pending = Default::default();
            }
            't' => {
                cx.emit(VimCommand::NextTab);
                self.pending = Default::default();
            }
            'T' => {
                cx.emit(VimCommand::PreviousTab);
                self.pending = Default::default();
            }
            '*' | '#' => self.search_word(state, c == '*', false, window, cx),
            '&' => {
                let last = text::last_row(state.text());
                self.ex(state, &format!("0,{}&&", last + 1), window, cx);
                self.pending = Default::default();
            }
            _ => self.pending = Default::default(),
        }
        true
    }

    fn select_object(
        &mut self,
        state: &mut EditorState,
        object: Object,
        around: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        let text = state.text().clone();
        let n = self.pending.n();
        if let Some(op) = self.pending.operator {
            let spans: Vec<Span> = self
                .cursors
                .iter()
                .filter_map(|c| object::select(&text, c.head, object, around, n, None))
                .map(Span::object)
                .collect();
            if spans.is_empty() {
                self.pending = Default::default();
                return;
            }
            let changed = op != Operator::Yank;
            let entered = self.operate_spans(state, op, spans, false, window, cx);
            if !entered {
                self.finish(changed);
            }
            return;
        }
        // Visual mode: the selection becomes the object (or grows to the
        // next one out).
        let mut linewise = false;
        for cursor in &mut self.cursors {
            let current = (cursor.anchor != cursor.head || self.mode != VimMode::Visual)
                .then(|| cursor.anchor.min(cursor.head)..text::next(&text, cursor.anchor.max(cursor.head)));
            let Some(selected) = object::select(&text, cursor.head, object, around, n, current) else { continue };
            if selected.range.is_empty() {
                continue;
            }
            linewise |= selected.linewise;
            cursor.anchor = selected.range.start;
            cursor.head = text::prev(&text, selected.range.end);
            cursor.column = text::column(&text, cursor.head);
        }
        if linewise {
            self.mode = VimMode::VisualLine;
        } else if self.mode == VimMode::VisualLine {
            self.mode = VimMode::Visual;
        }
        self.pending = Default::default();
    }

    fn normal_command(&mut self, state: &mut EditorState, key: Key, window: &mut Window, cx: &mut Context<EditorState>) -> bool {
        let text = state.text().clone();
        let n = self.pending.n();
        let heads: Vec<usize> = self.cursors.iter().map(|c| c.head).collect();
        let line_of = |o: usize| text::row(&text, o);
        match key {
            Key::Char('i') => self.insert_at(state, heads, Repeat::InPlace, window, cx),
            Key::Char('a') => {
                let at = heads.iter().map(|&h| text::next(&text, h).min(text::line_end(&text, line_of(h)))).collect();
                self.insert_at(state, at, Repeat::InPlace, window, cx);
            }
            Key::Char('I') => {
                let at = heads.iter().map(|&h| text::first_non_blank(&text, line_of(h))).collect();
                self.insert_at(state, at, Repeat::InPlace, window, cx);
            }
            Key::Char('A') => {
                let at = heads.iter().map(|&h| text::line_end(&text, line_of(h))).collect();
                self.insert_at(state, at, Repeat::InPlace, window, cx);
            }
            Key::Char(c @ ('o' | 'O')) => {
                let undo_mark = state.last_transaction();
                let Some(at) = ops::open_lines(self, state, c == 'O', window, cx) else {
                    self.pending = Default::default();
                    return true;
                };
                self.begin_insert(state, at, undo_mark, Repeat::Lines, false, cx);
            }
            Key::Char('x') | Key::Delete => self.operator_motion(state, Operator::Delete, Motion::Right, window, cx),
            Key::Char('X') => self.operator_motion(state, Operator::Delete, Motion::Left, window, cx),
            Key::Char('s') => self.operator_motion(state, Operator::Change, Motion::Right, window, cx),
            Key::Char('C') => self.operator_motion(state, Operator::Change, Motion::LineEnd, window, cx),
            Key::Char('D') => self.operator_motion(state, Operator::Delete, Motion::LineEnd, window, cx),
            Key::Char('S') => {
                self.pending.operator = Some(Operator::Change);
                self.operator_key(state, Operator::Change, window, cx);
            }
            Key::Char('Y') => {
                self.pending.operator = Some(Operator::Yank);
                self.operator_key(state, Operator::Yank, window, cx);
            }
            Key::Char(c @ ('p' | 'P')) => {
                ops::paste(self, state, c == 'P', false, n, window, cx);
                self.finish(true);
            }
            Key::Char('J') => {
                let (top, bottom) = self.join_rows(&text);
                ops::join(self, state, top, bottom, true, window, cx);
                self.finish(true);
            }
            Key::Char('r') => self.pending.prefix = Some(Prefix::Replace),
            Key::Char('R') => {
                let undo_mark = state.last_transaction();
                self.begin_insert(state, heads, undo_mark, Repeat::InPlace, true, cx);
            }
            Key::Char('~') => {
                ops::toggle_case_chars(self, state, n, window, cx);
                self.finish(true);
            }
            Key::Char('u') => {
                undo(self, state, n, false, window, cx);
                self.pending = Default::default();
            }
            Key::Ctrl('r') => {
                undo(self, state, n, true, window, cx);
                self.pending = Default::default();
            }
            Key::Char('.') => {
                let count = self.pending.count();
                self.pending = Default::default();
                self.repeat(state, count, window, cx);
            }
            Key::Ctrl(c @ ('a' | 'x')) => {
                let delta = if c == 'a' { n as i64 } else { -(n as i64) };
                if ops::increment(self, state, delta, window, cx) {
                    self.finish(true);
                } else {
                    self.pending = Default::default();
                }
            }
            Key::Char('m') => self.pending.prefix = Some(Prefix::SetMark),
            Key::Char(':') => {
                let range = match self.pending.count() {
                    Some(1) | None => String::new(),
                    Some(n) => format!(".,.+{}", n - 1),
                };
                self.pending = Default::default();
                self.open_command_line(':', range, None, cx);
            }
            Key::Char('v') => self.enter_visual(VimMode::Visual),
            Key::Char('V') => self.enter_visual(VimMode::VisualLine),
            Key::Ctrl('v') => self.enter_visual(VimMode::VisualBlock),
            Key::Char('&') => {
                self.pending = Default::default();
                self.ex(state, "&", window, cx);
            }
            Key::Ctrl('d' | 'u' | 'f' | 'b' | 'e' | 'y') | Key::PageDown | Key::PageUp => {
                let count = self.pending.count();
                self.scroll(state, key, count, cx);
                self.pending = Default::default();
            }
            Key::Ctrl('o') => {
                cx.emit(VimCommand::GoBack);
                self.pending = Default::default();
            }
            Key::Ctrl('i') => {
                cx.emit(VimCommand::GoForward);
                self.pending = Default::default();
            }
            Key::Tab => {
                self.pending = Default::default();
                // Tab still jumps to a placeholder on the line (Xcode's).
                let row = line_of(self.primary().head);
                return !text::line_text(&text, row).contains("<#");
            }
            _ => self.pending = Default::default(),
        }
        true
    }

    fn visual_command(&mut self, state: &mut EditorState, key: Key, window: &mut Window, cx: &mut Context<EditorState>) -> bool {
        let text = state.text().clone();
        match key {
            Key::Char('o') => {
                for cursor in &mut self.cursors {
                    std::mem::swap(&mut cursor.anchor, &mut cursor.head);
                    cursor.column = text::column(&text, cursor.head);
                }
                self.pending = Default::default();
            }
            Key::Char('O') if self.mode == VimMode::VisualBlock => {
                let cursor = &mut self.cursors[0];
                let (ar, hr) = (text::row(&text, cursor.anchor), text::row(&text, cursor.head));
                let (ac, hc) = (text::column(&text, cursor.anchor), text::column(&text, cursor.head));
                cursor.anchor = text::at_column(&text, ar, hc, true);
                cursor.head = text::at_column(&text, hr, ac, true);
                cursor.column = ac;
                self.pending = Default::default();
            }
            Key::Char('O') => return self.visual_command(state, Key::Char('o'), window, cx),
            Key::Char('x') | Key::Delete => self.operate_visual(state, Operator::Delete, false, 1, window, cx),
            Key::Char('X') | Key::Char('D') => self.operate_visual(state, Operator::Delete, true, 1, window, cx),
            Key::Char('s') => self.operate_visual(state, Operator::Change, false, 1, window, cx),
            Key::Char('C') | Key::Char('S') | Key::Char('R') => {
                self.operate_visual(state, Operator::Change, true, 1, window, cx)
            }
            Key::Char('Y') => self.operate_visual(state, Operator::Yank, true, 1, window, cx),
            Key::Char('~') => self.operate_visual(state, Operator::ToggleCase, false, 1, window, cx),
            Key::Char('u') => self.operate_visual(state, Operator::Lowercase, false, 1, window, cx),
            Key::Char('U') => self.operate_visual(state, Operator::Uppercase, false, 1, window, cx),
            Key::Char('J') => {
                let (top, bottom) = self.join_rows(&text);
                self.exit_visual(state);
                ops::join(self, state, top, bottom, true, window, cx);
                self.finish(true);
            }
            Key::Char('r') => self.pending.prefix = Some(Prefix::Replace),
            Key::Char(c @ ('p' | 'P')) => {
                ops::visual_paste(self, state, c == 'p', window, cx);
                self.finish(true);
            }
            Key::Char(c @ ('I' | 'A')) => self.visual_insert(state, c == 'A', window, cx),
            Key::Char(':') => {
                self.exit_visual(state);
                self.pending = Default::default();
                self.open_command_line(':', "'<,'>", None, cx);
            }
            Key::Char('v') => self.switch_visual(state, VimMode::Visual),
            Key::Char('V') => self.switch_visual(state, VimMode::VisualLine),
            Key::Ctrl('v') => self.switch_visual(state, VimMode::VisualBlock),
            Key::Ctrl('d' | 'u' | 'f' | 'b' | 'e' | 'y') | Key::PageDown | Key::PageUp => {
                let count = self.pending.count();
                self.scroll(state, key, count, cx);
                self.pending = Default::default();
            }
            _ => self.pending = Default::default(),
        }
        true
    }

    fn enter_visual(&mut self, mode: VimMode) {
        self.mode = mode;
        self.block_to_end = false;
        if mode == VimMode::VisualBlock {
            self.cursors.truncate(1);
        }
        self.pending = Default::default();
    }

    fn switch_visual(&mut self, state: &mut EditorState, mode: VimMode) {
        if self.mode == mode {
            self.exit_visual(state);
        } else {
            self.mode = mode;
            if mode == VimMode::VisualBlock {
                self.cursors.truncate(1);
            }
        }
        self.pending = Default::default();
    }

    /// The rows `J` joins: the selection's, or the cursor's and as many
    /// more as the count says (at least two).
    fn join_rows(&self, text: &ropey::Rope) -> (usize, usize) {
        let cursor = self.primary();
        if self.mode.is_visual() {
            let (a, h) = (text::row(text, cursor.anchor), text::row(text, cursor.head));
            let (top, bottom) = (a.min(h), a.max(h));
            return (top, bottom.max(top + 1).min(text::last_row(text)));
        }
        let top = text::row(text, cursor.head);
        let n = self.pending.n().max(2);
        (top, (top + n - 1).min(text::last_row(text)))
    }

    fn operator_motion(&mut self, state: &mut EditorState, op: Operator, motion: Motion, window: &mut Window, cx: &mut Context<EditorState>) {
        self.pending.operator = Some(op);
        self.run_motion(state, motion, window, cx);
    }

    /// An operator over the visual selection; `lines` makes it linewise
    /// (`X`, `Y`, `S`; `D` and `C` to the end of each row in a block).
    pub(super) fn operate_visual(
        &mut self,
        state: &mut EditorState,
        op: Operator,
        lines: bool,
        times: usize,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        let text = state.text().clone();
        let mode = self.mode;
        self.remember_visual(state);
        let block = mode == VimMode::VisualBlock && !(lines && !matches!(op, Operator::Delete | Operator::Change));
        let spans: Vec<Span> = if block {
            let to_end = self.block_to_end || lines;
            super::block_rows(&text, self.primary(), to_end).into_iter().map(Span::chars).collect()
        } else {
            self.cursors
                .iter()
                .map(|c| {
                    let (start, end) = (c.anchor.min(c.head), c.anchor.max(c.head));
                    if mode == VimMode::VisualLine || lines || mode == VimMode::VisualBlock {
                        Span::lines(ops::line_range(&text, text::row(&text, start), text::row(&text, end)))
                    } else {
                        Span::chars(start..text::next(&text, end).max(start))
                    }
                })
                .collect()
        };
        // The cursor goes to the selection's start, in normal mode, before
        // the edit, so undo puts it back there.
        let top_left = if block {
            let cursor = self.primary();
            let top = text::row(&text, cursor.anchor).min(text::row(&text, cursor.head));
            let left = text::column(&text, cursor.anchor).min(text::column(&text, cursor.head));
            text::at_column(&text, top, left, true)
        } else {
            spans.iter().map(|s| s.range.start).min().unwrap_or(0)
        };
        self.mode = VimMode::Normal;
        self.block_to_end = false;
        if block {
            self.cursors = vec![Cursor::at(top_left, &text)];
        } else {
            for (cursor, span) in self.cursors.iter_mut().zip(&spans) {
                *cursor = Cursor::at(span.range.start, &text);
            }
        }
        let mut entered = false;
        for _ in 0..times.max(1) {
            entered = self.operate_spans(state, op, spans.clone(), block, window, cx);
        }
        if entered && let Some(session) = &mut self.insert {
            session.block = block;
        }
        if !entered {
            self.finish(op != Operator::Yank);
        }
    }

    /// Visual `I` and `A`: insert at the start (or after the end) of the
    /// selection; in a block, on every row at once.
    fn visual_insert(&mut self, state: &mut EditorState, append: bool, window: &mut Window, cx: &mut Context<EditorState>) {
        let text = state.text().clone();
        let undo_mark = state.last_transaction();
        let at: Vec<usize> = match self.mode {
            VimMode::VisualBlock => {
                let cursor = self.primary();
                let (ar, hr) = (text::row(&text, cursor.anchor), text::row(&text, cursor.head));
                let (ac, hc) = (text::column(&text, cursor.anchor), text::column(&text, cursor.head));
                let (left, right) = (ac.min(hc), ac.max(hc));
                let rows = ar.min(hr)..=ar.max(hr);
                if append && !self.block_to_end {
                    // Short rows are padded to reach the block's right edge.
                    let padding: Vec<(std::ops::Range<usize>, String)> = rows
                        .clone()
                        .filter_map(|row| {
                            let len = text::line_text(&text, row).chars().count();
                            (len <= right).then(|| {
                                let end = text::line_end(&text, row);
                                (end..end, " ".repeat(right + 1 - len))
                            })
                        })
                        .collect();
                    if !padding.is_empty() {
                        self.remember_visual(state);
                        self.mode = VimMode::Normal;
                        self.show(state, cx);
                        state.transact(window, cx, |state, window, cx| state.edit(padding, window, cx));
                    }
                }
                let text = state.text().clone();
                rows.filter_map(|row| {
                    if append {
                        Some(if self.block_to_end {
                            text::line_end(&text, row)
                        } else {
                            text::at_column(&text, row, right + 1, true)
                        })
                    } else {
                        let len = text::line_text(&text, row).chars().count();
                        (len >= left).then(|| text::at_column(&text, row, left, true))
                    }
                })
                .collect()
            }
            VimMode::VisualLine => self
                .cursors
                .iter()
                .map(|c| {
                    let row = if append { text::row(&text, c.anchor.max(c.head)) } else { text::row(&text, c.anchor.min(c.head)) };
                    if append { text::line_end(&text, row) } else { text::first_non_blank(&text, row) }
                })
                .collect(),
            _ => self
                .cursors
                .iter()
                .map(|c| if append { text::next(&text, c.anchor.max(c.head)) } else { c.anchor.min(c.head) })
                .collect(),
        };
        let block = self.mode == VimMode::VisualBlock;
        self.remember_visual(state);
        self.block_to_end = false;
        self.begin_insert(state, at, undo_mark, Repeat::InPlace, false, cx);
        if let Some(session) = &mut self.insert {
            session.block = block;
        }
    }

    /// Starts inserting at `at` (one cursor each) with the command typed so
    /// far as the change `.` repeats.
    pub(super) fn insert_at(&mut self, state: &mut EditorState, at: Vec<usize>, repeat: Repeat, _window: &mut Window, cx: &mut Context<EditorState>) {
        let undo_mark = state.last_transaction();
        self.begin_insert(state, at, undo_mark, repeat, false, cx);
    }

    pub(super) fn begin_insert(
        &mut self,
        state: &mut EditorState,
        at: Vec<usize>,
        undo_mark: Option<crate::TransactionId>,
        repeat: Repeat,
        replace: bool,
        cx: &mut Context<EditorState>,
    ) {
        let text = state.text().clone();
        let count = self.pending.n();
        let change = self.change();
        self.pending = Default::default();
        self.mode = VimMode::Insert;
        self.cursors = at.into_iter().map(|o| Cursor::at(o.min(text.len()), &text)).collect();
        self.show(state, cx);
        self.start_insert(state, undo_mark, count, repeat, change, replace, cx);
    }

    /// `.`: the last change again, at the cursor (with a new count if one
    /// was typed).
    fn repeat(&mut self, state: &mut EditorState, count: Option<usize>, window: &mut Window, cx: &mut Context<EditorState>) {
        let Some(change) = self.last_change.clone() else { return };
        self.replaying = true;
        if let Some(extent) = change.visual {
            let text = state.text().clone();
            self.mode = extent.mode;
            for cursor in &mut self.cursors {
                let row = text::row(&text, cursor.head);
                let last = (row + extent.rows - 1).min(text::last_row(&text));
                cursor.anchor = cursor.head;
                cursor.head = match extent.mode {
                    VimMode::VisualLine => text::line_start(&text, last),
                    VimMode::VisualBlock => {
                        text::at_column(&text, last, text::column(&text, cursor.head) + extent.width - 1, false)
                    }
                    _ if extent.rows == 1 => {
                        let mut head = cursor.head;
                        for _ in 1..extent.width {
                            head = text::next(&text, head).min(text::last_char(&text, row));
                        }
                        head
                    }
                    _ => text::at_column(&text, last, extent.width - 1, false),
                };
            }
        }
        self.pending = super::normal::Pending { register: change.register, count: count.or(change.count), ..Default::default() };
        for key in change.keys {
            self.normal_key(state, key, window, cx);
            if matches!(self.mode, VimMode::Insert | VimMode::Replace) {
                break;
            }
        }
        if matches!(self.mode, VimMode::Insert | VimMode::Replace) {
            if let Some(typed) = &change.typed {
                ops::apply_typed(state, typed, window, cx);
            }
            self.leave_insert(state, window, cx);
        }
        self.replaying = false;
    }

    /// `*` and `#`: the word under (or after) the cursor, as a search.
    fn search_word(&mut self, state: &mut EditorState, forward: bool, whole: bool, window: &mut Window, cx: &mut Context<EditorState>) {
        let text = state.text().clone();
        let head = self.primary().head;
        let row = text::row(&text, head);
        let end = text::line_end(&text, row);
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let start = text::forward(&text, head).take_while(|(o, _)| *o < end).find(|(_, c)| is_word(*c)).map(|(o, _)| o);
        let Some(start) = start else {
            self.error("E348: No string under cursor");
            self.pending = Default::default();
            return;
        };
        let word_start = text::backward(&text, start)
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(start, |(o, _)| o);
        let word_end = text::forward(&text, start).take_while(|(o, c)| *o < end && is_word(*c)).last().map_or(start, |(o, c)| o + c.len_utf8());
        let word = text.slice(word_start..word_end).to_string();
        let pattern = if whole { search::word_pattern(&word) } else { word.chars().map(|c| if "\\/.*$^~[]".contains(c) { format!("\\{c}") } else { c.to_string() }).collect() };
        registers::set_special(cx, '/', pattern.clone());
        self.last_search = Some((pattern, forward));
        // From the word's start, so `#` skips the word itself.
        if !forward {
            for cursor in &mut self.cursors {
                cursor.head = word_start;
            }
        }
        self.run_motion(state, Motion::Search { reverse: false }, window, cx);
    }

    // ------------------------------------------------------------------
    // Scrolling and folds

    fn scroll(&mut self, state: &mut EditorState, key: Key, count: Option<usize>, cx: &mut Context<EditorState>) {
        let (Some(line_height), Some(bounds)) = (state.line_height(), state.last_bounds) else { return };
        let screen = ((bounds.size.height / line_height).floor() as usize).max(1);
        let text = state.text().clone();
        let last = text::last_row(&text);
        let offset = state.scroll_offset();
        let top = (-offset.y / line_height).max(0.).round() as usize;
        let head = self.primary().head;
        let row = text::row(&text, head);
        let (scroll_rows, cursor_rows): (isize, isize) = match key {
            Key::Ctrl('e') => (count.unwrap_or(1) as isize, 0),
            Key::Ctrl('y') => (-(count.unwrap_or(1) as isize), 0),
            Key::Ctrl('d') => {
                let half = count.unwrap_or(screen / 2).max(1) as isize;
                (half, half)
            }
            Key::Ctrl('u') => {
                let half = count.unwrap_or(screen / 2).max(1) as isize;
                (-half, -half)
            }
            Key::Ctrl('f') | Key::PageDown => {
                let page = (screen.saturating_sub(2).max(1) * count.unwrap_or(1)) as isize;
                (page, page)
            }
            _ => {
                let page = (screen.saturating_sub(2).max(1) * count.unwrap_or(1)) as isize;
                (-page, -page)
            }
        };
        let new_top = (top as isize + scroll_rows).clamp(0, last as isize) as usize;
        let mut new_row = (row as isize + cursor_rows).clamp(0, last as isize) as usize;
        // The cursor stays on screen.
        new_row = new_row.clamp(new_top.min(last), (new_top + screen.saturating_sub(1)).min(last));
        if new_row != row {
            let at = text::first_non_blank(&text, new_row);
            for cursor in &mut self.cursors {
                *cursor = Cursor::at(at, &text);
            }
            self.cursors.truncate(1);
            self.show(state, cx);
        }
        state.set_scroll_offset(point(offset.x, -line_height * new_top as f32), cx);
    }

    fn z_command(&mut self, state: &mut EditorState, c: char, cx: &mut Context<EditorState>) {
        let text = state.text().clone();
        let row = text::row(&text, self.primary().head);
        match c {
            'z' | '.' | 't' | '\n' | 'b' | '-' => {
                let (Some(line_height), Some(bounds)) = (state.line_height(), state.last_bounds) else { return };
                let y = line_height * (state.display_map.buffer_line_to_display_row(row) + state.block_rows_before(row)) as f32;
                let target = match c {
                    'z' | '.' => y - (bounds.size.height - line_height) / 2.,
                    't' | '\n' => y,
                    _ => y - (bounds.size.height - line_height),
                };
                let offset = state.scroll_offset();
                state.set_scroll_offset(point(offset.x, -target.max(gpui::px(0.))), cx);
                if matches!(c, '.' | '\n' | '-') {
                    self.cursors = vec![Cursor::at(text::first_non_blank(&text, row), &text)];
                }
            }
            'o' | 'O' | 'v' => {
                let folds: Vec<_> = state.display_map.folded_ranges().to_vec();
                for fold in folds.iter().filter(|f| f.start_line <= row && row <= f.end_line) {
                    state.display_map.set_folded(fold.start_line, false);
                }
            }
            'c' | 'C' => {
                if let Some(start) = innermost_fold(state, row) {
                    state.display_map.set_folded(start, true);
                    self.cursors = vec![Cursor::at(text::first_non_blank(&text, start), &text)];
                }
            }
            'a' | 'A' => {
                let folded = state.display_map.folded_ranges().iter().find(|f| f.start_line <= row && row <= f.end_line).map(|f| f.start_line);
                match folded {
                    Some(start) => state.display_map.set_folded(start, false),
                    None => {
                        if let Some(start) = innermost_fold(state, row) {
                            state.display_map.set_folded(start, true);
                            self.cursors = vec![Cursor::at(text::first_non_blank(&text, start), &text)];
                        }
                    }
                }
            }
            'R' => state.display_map.clear_folds(),
            'M' => {
                let starts: Vec<usize> = state.display_map.fold_candidates().iter().map(|f| f.start_line).collect();
                for start in starts {
                    state.display_map.set_folded(start, true);
                }
                if state.display_map.is_buffer_line_hidden(row)
                    && let Some(fold) = state.display_map.folded_ranges().iter().filter(|f| f.start_line < row && row <= f.end_line).min_by_key(|f| f.start_line)
                {
                    let start = fold.start_line;
                    self.cursors = vec![Cursor::at(text::first_non_blank(&text, start), &text)];
                }
            }
            _ => {}
        }
        cx.notify();
    }
}

/// The smallest fold that can close around `row`.
fn innermost_fold(state: &EditorState, row: usize) -> Option<usize> {
    state
        .display_map
        .fold_candidates()
        .iter()
        .filter(|f| f.start_line <= row && row <= f.end_line && f.end_line > f.start_line)
        .min_by_key(|f| f.end_line - f.start_line)
        .map(|f| f.start_line)
}

fn operator_for(key: Key) -> Option<Operator> {
    Some(match key {
        Key::Char('d') => Operator::Delete,
        Key::Char('c') => Operator::Change,
        Key::Char('y') => Operator::Yank,
        Key::Char('>') => Operator::Indent,
        Key::Char('<') => Operator::Outdent,
        Key::Char('=') => Operator::Reindent,
        _ => return None,
    })
}

/// `u` and `⌃R` (and `:undo`, `:redo`): the cursor goes to where the change
/// was, in normal mode.
pub(super) fn undo(vim: &mut Vim, state: &mut EditorState, count: usize, redo: bool, window: &mut Window, cx: &mut Context<EditorState>) {
    let before = state.text().clone();
    for _ in 0..count.max(1) {
        if redo {
            state.redo(window, cx);
        } else {
            state.undo(window, cx);
        }
    }
    if *state.text() == before {
        if !state.external_history {
            vim.say(if redo { "Already at newest change" } else { "Already at oldest change" });
        }
        return;
    }
    vim.mode = VimMode::Normal;
    let text = state.text().clone();
    // Undo puts back the cursors the change started from; redo goes to
    // the start of what it changed.
    vim.cursors = if redo && state.selections.len() == 1 {
        let changed = before.bytes().zip(text.bytes()).take_while(|(a, b)| a == b).count();
        vec![Cursor::at(text::clamp_normal(&text, text.floor_char_boundary(changed.min(text.len()))), &text)]
    } else {
        state.selections.iter().map(|s| Cursor::at(text::clamp_normal(&text, s.start), &text)).collect()
    };
}

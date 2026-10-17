//! Vim mode (roadmap 2.7): modal editing as a layer over the editor.
//!
//! The layer is a keystroke interceptor (`modal.rs`): it sees every
//! keystroke before the keymap and text input do, and in normal and visual
//! modes takes the plain and ⌃ keys, so `j` moves instead of typing. In
//! insert mode it lets typing, completion, auto-closing pairs and
//! placeholders work as without Vim, and only takes Escape (after the
//! completion menu, signature help and find have had it) and a few ⌃ keys.
//! ⌘ keys always reach the keymap.
//!
//! Modes are states of the layer, not of the engine: normal mode's cursor
//! is a collapsed selection on a character (drawn as a block), visual
//! modes are selections (with the caret drawn on the character Vim's cursor
//! is on, `caret_heads`), visual block is one selection per row. Every
//! cursor of a multi-cursor editor moves and operates; a selection made
//! with the mouse or a command (⌘D, a placeholder) switches to visual mode.
//!
//! Each command is one transaction, so `u` and ⌘Z undo it in one step; an
//! insert, with the change that started it (`cw`), joins into one
//! (`UndoManager::merge_after`). `.` repeats the last change with the text
//! its insert typed, worked out from the text before and after the insert
//! rather than from keystrokes, so completions and auto-closed pairs repeat
//! too. `:w`, `:q` and the like are [`VimCommand`] events for the host,
//! which knows what a tab is.

mod command;
mod keys;
mod motion;
mod normal;
mod object;
mod ops;
mod registers;
mod search;
mod text;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui::{Context, EventEmitter, HighlightStyle, Hsla, Keystroke, SharedString, Subscription, Window};
use ropey::Rope;

use crate::cursor::CursorSelection;
use crate::{Anchor, AnchorBias, CursorShape, EditorState, InputBaseState, InputModeKind, TransactionId};

use keys::Key;
use motion::LastFind;

/// Vim's modes, as the status bar names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VimMode {
    Normal,
    Insert,
    Replace,
    Visual,
    VisualLine,
    VisualBlock,
}

impl VimMode {
    pub fn label(self) -> &'static str {
        match self {
            VimMode::Normal => "NORMAL",
            VimMode::Insert => "INSERT",
            VimMode::Replace => "REPLACE",
            VimMode::Visual => "VISUAL",
            VimMode::VisualLine => "VISUAL LINE",
            VimMode::VisualBlock => "VISUAL BLOCK",
        }
    }

    pub fn is_visual(self) -> bool {
        matches!(self, VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock)
    }

    fn key_context(self) -> &'static str {
        match self {
            VimMode::Normal => "vim_mode=normal",
            VimMode::Insert => "vim_mode=insert",
            VimMode::Replace => "vim_mode=replace",
            _ => "vim_mode=visual",
        }
    }
}

/// What the status bar shows for an editor in Vim mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VimStatus {
    pub mode: VimMode,
    /// Keys of a command not finished yet (`2d`, `"a`), as Vim's showcmd.
    pub pending: SharedString,
    /// The `:` or `/` line being typed, with its prompt.
    pub command_line: Option<SharedString>,
    /// The last command's message (`3 substitutions on 3 lines`, an error).
    pub message: Option<SharedString>,
    pub error: bool,
}

/// What a command line asks of the editor's host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VimCommand {
    /// `:w`; with `all`, `:wa`. With `quit`, `:wq` / `:x` (and `:wqa`).
    Write { all: bool, quit: bool },
    /// `:q`, `:q!`; with `all`, `:qa`.
    Quit { all: bool, force: bool },
    /// `gt`, `gT`
    NextTab,
    PreviousTab,
    /// `⌃O`, `⌃I`: the navigation history.
    GoBack,
    GoForward,
    /// `=`: re-indent these bytes (whole lines).
    Reindent(Range<usize>),
}

impl<M: InputModeKind> EventEmitter<VimCommand> for InputBaseState<M> {}

/// A cursor: in normal mode `anchor == head`; in visual modes the
/// selection runs between them, both characters included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cursor {
    anchor: usize,
    head: usize,
    /// The character column `j` and `k` aim for; `usize::MAX` after `$`.
    column: usize,
}

impl Cursor {
    fn at(offset: usize, text: &Rope) -> Self {
        Cursor { anchor: offset, head: offset, column: text::column(text, offset) }
    }
}

/// The text an insert typed, relative to where it started: it removed
/// `before` bytes before the cursor and `after` bytes after it, and put
/// `text` there, leaving the cursor `caret` bytes into it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Typed {
    before: usize,
    after: usize,
    text: String,
    caret: usize,
}

/// How many times an insert repeats, and how (`3ix<Esc>`, `3ox<Esc>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Repeat {
    InPlace,
    Lines,
}

/// The last change, for `.`.
#[derive(Clone, Debug)]
struct Change {
    keys: Vec<Key>,
    count: Option<usize>,
    register: Option<char>,
    typed: Option<Typed>,
    /// Made in visual mode: the size of the selection to repeat it over.
    visual: Option<VisualExtent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VisualExtent {
    mode: VimMode,
    rows: usize,
    /// Characters on the last row (char visual) or columns (block).
    width: usize,
}

/// An insert in progress.
struct InsertSession {
    before: Rope,
    cursors: Vec<usize>,
    /// The last transaction before the command that started the insert;
    /// everything after it joins into one undo step at Escape.
    undo_mark: Option<TransactionId>,
    count: usize,
    repeat: Repeat,
    change: Option<Change>,
    /// The cursor was moved (arrows, a click): what's typed from now on is
    /// what `.` repeats.
    moved: bool,
    /// What Replace mode overwrote, a keystroke at a time, for Backspace.
    replaced: Vec<Vec<(usize, Option<char>)>>,
    /// `⌃R`: the next key names a register to insert.
    register: bool,
    /// On every row of a visual block: Escape leaves one cursor, at the
    /// top.
    block: bool,
}

/// A `:` or `/` line being typed.
struct CommandLine {
    prompt: char,
    text: String,
    /// `d/foo`: the operator waiting for the search.
    pending: Option<normal::Pending>,
    history: Option<usize>,
}

pub(crate) struct Vim {
    mode: VimMode,
    cursors: Vec<Cursor>,
    /// Visual block after `$`: every row to its end.
    block_to_end: bool,
    pending: normal::Pending,
    /// The engine's selections as the layer last left them, to notice
    /// changes made elsewhere (the mouse, ⌘D, another pane's edit).
    expected: Vec<(usize, usize, bool)>,
    last_find: Option<LastFind>,
    /// The last search pattern and whether it went forward.
    last_search: Option<(String, bool)>,
    highlight_search: bool,
    search_style: HighlightStyle,
    last_change: Option<Change>,
    last_substitution: Option<command::Substitution>,
    insert: Option<InsertSession>,
    /// `⌃O` in insert mode: the insert to go back to after one command.
    one_shot: Option<InsertSession>,
    marks: HashMap<char, Anchor>,
    last_visual: Option<(VimMode, Vec<Cursor>, bool)>,
    command_line: Option<CommandLine>,
    history: Vec<String>,
    search_history: Vec<String>,
    message: Option<(SharedString, bool)>,
    replaying: bool,
    _observer: Subscription,
}

const SEARCH_LAYER: &str = "vim-search";

impl Vim {
    fn new(observer: Subscription) -> Self {
        Vim {
            mode: VimMode::Normal,
            cursors: vec![],
            block_to_end: false,
            pending: Default::default(),
            expected: vec![],
            last_find: None,
            last_search: None,
            highlight_search: false,
            search_style: HighlightStyle {
                background_color: Some(Hsla { h: 50. / 360., s: 0.9, l: 0.5, a: 0.35 }),
                ..Default::default()
            },
            last_change: None,
            last_substitution: None,
            insert: None,
            one_shot: None,
            marks: HashMap::new(),
            last_visual: None,
            command_line: None,
            history: vec![],
            search_history: vec![],
            message: None,
            replaying: false,
            _observer: observer,
        }
    }

    fn status(&self) -> VimStatus {
        VimStatus {
            mode: self.mode,
            pending: self.pending.shown.clone().into(),
            command_line: self.command_line.as_ref().map(|line| format!("{}{}", line.prompt, line.text).into()),
            message: self.message.as_ref().map(|(m, _)| m.clone()),
            error: self.message.as_ref().is_some_and(|(_, e)| *e),
        }
    }

    fn say(&mut self, message: impl Into<SharedString>) {
        self.message = Some((message.into(), false));
    }

    fn error(&mut self, message: impl Into<SharedString>) {
        self.message = Some((message.into(), true));
    }

    // ------------------------------------------------------------------
    // Cursors and the engine's selections

    fn primary(&self) -> Cursor {
        self.cursors.first().copied().unwrap_or(Cursor { anchor: 0, head: 0, column: 0 })
    }

    /// Reads the cursors from the engine (after an insert, or when the
    /// selections changed under the layer).
    fn read_cursors(&mut self, state: &EditorState) {
        let text = state.text();
        self.cursors = state
            .selections
            .iter()
            .map(|sel| {
                if sel.is_empty() {
                    Cursor::at(sel.start, text)
                } else if sel.reversed {
                    let anchor = text::prev(text, sel.end);
                    Cursor { anchor, head: sel.start, column: text::column(text, sel.start) }
                } else {
                    let head = text::prev(text, sel.end);
                    Cursor { anchor: sel.start, head, column: text::column(text, head) }
                }
            })
            .collect();
    }

    /// Puts the cursors into the engine as Vim shows them in the mode.
    fn show(&mut self, state: &mut EditorState, cx: &mut Context<EditorState>) {
        let text = state.text().clone();
        let len = text.len();
        let mut heads = vec![];
        let selections: Vec<(usize, usize, bool)> = match self.mode {
            VimMode::Normal | VimMode::Replace | VimMode::Insert => {
                if self.mode == VimMode::Normal {
                    for cursor in &mut self.cursors {
                        cursor.head = text::clamp_normal(&text, cursor.head);
                        cursor.anchor = cursor.head;
                    }
                }
                self.cursors.iter().map(|c| (c.head.min(len), c.head.min(len), false)).collect()
            }
            VimMode::Visual => self
                .cursors
                .iter()
                .map(|c| {
                    heads.push(c.head.min(len));
                    let (start, end) = (c.anchor.min(c.head), c.anchor.max(c.head));
                    (start.min(len), text::next(&text, end).max(start).min(len), c.head < c.anchor)
                })
                .collect(),
            VimMode::VisualLine => self
                .cursors
                .iter()
                .map(|c| {
                    heads.push(c.head.min(len));
                    let (a, h) = (text::row(&text, c.anchor), text::row(&text, c.head));
                    let (top, bottom) = (a.min(h), a.max(h));
                    (text::line_start(&text, top), text::line_full_end(&text, bottom), h < a)
                })
                .collect(),
            VimMode::VisualBlock => {
                let cursor = self.primary();
                heads.push(cursor.head.min(len));
                let rows = block_rows(&text, cursor, self.block_to_end);
                let mut selections: Vec<(usize, usize, bool)> =
                    rows.into_iter().filter(|r| !r.is_empty()).map(|r| (r.start, r.end, false)).collect();
                // The head's row first: it's the one the view follows.
                let head_row = text::row(&text, cursor.head);
                if let Some(ix) = selections.iter().position(|(s, _, _)| text::row(&text, *s) == head_row) {
                    let row = selections.remove(ix);
                    selections.insert(0, row);
                }
                if selections.is_empty() {
                    selections.push((cursor.head, cursor.head, false));
                }
                selections
            }
        };
        let selections = if selections.is_empty() { vec![(0, 0, false)] } else { selections };
        state.vim_write_selections(&selections, heads.first().copied(), cx);
        state.caret_heads = (!heads.is_empty()).then_some(heads);
        self.apply_mode(state, cx);
        self.capture(state);
    }

    /// Remembers the engine's selections as the layer left them.
    fn capture(&mut self, state: &EditorState) {
        self.expected = state.selections.iter().map(|s| (s.start, s.end, s.reversed)).collect();
    }

    fn apply_mode(&self, state: &mut EditorState, cx: &mut Context<EditorState>) {
        let shape = match self.mode {
            VimMode::Insert => CursorShape::Bar,
            VimMode::Replace => CursorShape::Underline,
            VimMode::Normal if self.pending.operator.is_some() => CursorShape::Underline,
            _ => CursorShape::Block,
        };
        state.set_cursor_shape(shape, cx);
        if state.extra_key_context.as_deref() != Some(self.mode.key_context()) {
            state.set_extra_key_context(Some(self.mode.key_context().into()), cx);
        }
        if !self.mode.is_visual() && state.caret_heads.is_some() {
            state.caret_heads = None;
        }
    }

    /// Follows selection changes made outside the layer: a click puts a
    /// normal-mode cursor back on a character, a selection made with the
    /// mouse or a command (⌘D, a placeholder) is visual mode, and a click in
    /// visual mode ends it.
    fn sync(&mut self, state: &mut EditorState, cx: &mut Context<EditorState>) {
        if matches!(self.mode, VimMode::Insert | VimMode::Replace) || self.command_line.is_some() {
            self.capture(state);
            return;
        }
        let current: Vec<(usize, usize, bool)> = state.selections.iter().map(|s| (s.start, s.end, s.reversed)).collect();
        if current == self.expected {
            return;
        }
        let selected = state.selections.iter().any(|s| !s.is_empty());
        self.read_cursors(state);
        self.pending = Default::default();
        let mode = match (self.mode, selected) {
            (_, false) => VimMode::Normal,
            (VimMode::VisualLine, true) if current.len() == 1 => VimMode::Visual,
            (_, true) => VimMode::Visual,
        };
        if mode == VimMode::Normal && self.mode.is_visual() {
            self.remember_visual(state);
        }
        self.mode = mode;
        self.show(state, cx);
        cx.notify();
    }

    fn remember_visual(&mut self, state: &mut EditorState) {
        if !self.mode.is_visual() || self.cursors.is_empty() {
            return;
        }
        self.last_visual = Some((self.mode, self.cursors.clone(), self.block_to_end));
        let text = state.text().clone();
        let cursor = self.primary();
        let (mut start, mut end) = (cursor.anchor.min(cursor.head), cursor.anchor.max(cursor.head));
        if self.mode == VimMode::VisualLine {
            start = text::line_start(&text, text::row(&text, start));
            end = text::last_char(&text, text::row(&text, end));
        }
        self.set_mark(state, '<', start);
        self.set_mark(state, '>', end);
    }

    // ------------------------------------------------------------------
    // Marks

    fn set_mark(&mut self, state: &mut EditorState, name: char, offset: usize) {
        let anchor = state.anchor(offset, AnchorBias::Left);
        if let Some(old) = self.marks.insert(name, anchor) {
            state.remove_anchor(old);
        }
    }

    fn mark(&self, state: &EditorState, name: char) -> Option<usize> {
        let name = if name == '`' { '\'' } else { name };
        state.anchor_offset(*self.marks.get(&name)?)
    }

    // ------------------------------------------------------------------
    // Keystrokes

    fn keystroke(
        &mut self,
        state: &mut EditorState,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) -> bool {
        let had_message = self.message.take().is_some();
        if had_message {
            cx.notify();
        }
        if self.command_line.is_some() {
            return self.command_line_key(state, keystroke, window, cx);
        }
        let key = Key::from_keystroke(keystroke);
        let handled = match self.mode {
            VimMode::Insert => self.insert_key(state, key, window, cx),
            VimMode::Replace => self.replace_key(state, key, window, cx),
            _ => match key {
                Some(key) => {
                    let handled = self.normal_key(state, key, window, cx);
                    if self.mode != VimMode::Insert && self.one_shot.is_some() && self.pending.is_empty() {
                        self.resume_insert(state, cx);
                    }
                    handled
                }
                None => {
                    // ⌘ and ⌥ chords are the keymap's; whatever was pending is dropped.
                    if !self.pending.is_empty() {
                        self.pending = Default::default();
                        self.apply_mode(state, cx);
                        cx.notify();
                    }
                    false
                }
            },
        };
        if handled {
            cx.notify();
        }
        handled
    }

    // ------------------------------------------------------------------
    // Insert and replace

    /// Starts an insert at the engine's cursors (already placed).
    fn start_insert(
        &mut self,
        state: &mut EditorState,
        undo_mark: Option<TransactionId>,
        count: usize,
        repeat: Repeat,
        change: Option<Change>,
        replace: bool,
        cx: &mut Context<EditorState>,
    ) {
        self.mode = if replace { VimMode::Replace } else { VimMode::Insert };
        let mut cursors: Vec<usize> = state.selections.iter().map(|s| s.cursor_offset()).collect();
        cursors.sort_unstable();
        self.insert = Some(InsertSession {
            before: state.text().clone(),
            cursors,
            undo_mark,
            count: count.max(1),
            repeat,
            change,
            moved: false,
            replaced: vec![],
            register: false,
            block: false,
        });
        state.caret_heads = None;
        self.highlight_search(state, false, cx);
        self.apply_mode(state, cx);
        self.capture(state);
    }

    /// Whether Escape has something to close before it leaves insert mode.
    fn escape_closes_something(state: &EditorState, cx: &mut Context<EditorState>) -> bool {
        state.is_context_menu_open(cx) || state.signature_help().is_some() || state.has_inline_completion()
    }

    fn insert_key(
        &mut self,
        state: &mut EditorState,
        key: Option<Key>,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) -> bool {
        self.resnapshot_if_moved(state);
        if self.insert.as_ref().is_some_and(|s| s.register) {
            if let Some(session) = &mut self.insert {
                session.register = false;
            }
            if let Some(Key::Char(name)) = key
                && let Some(register) = registers::read(cx, Some(name))
            {
                let text = register.text();
                state.vim_type(&text, window, cx);
            }
            return true;
        }
        match key {
            Some(Key::Escape) | Some(Key::Ctrl('c')) => {
                if key == Some(Key::Escape) && Self::escape_closes_something(state, cx) {
                    return false;
                }
                if key == Some(Key::Escape) && state.search_session.is_active() {
                    state.close_search(cx);
                    return true;
                }
                self.leave_insert(state, window, cx);
                true
            }
            Some(Key::Ctrl('w')) => {
                // The word before the cursor, or the line break at its start.
                ops::delete_before_cursors(state, |text, cursor| {
                    let start = text::line_start(text, text::row(text, cursor));
                    if cursor == start { text::prev(text, cursor) } else { motion::previous_word_start(text, cursor, false).max(start) }
                }, window, cx);
                true
            }
            Some(Key::Ctrl('u')) => {
                ops::delete_before_cursors(state, |text, cursor| {
                    let row = text::row(text, cursor);
                    let first = text::first_non_blank(text, row);
                    if cursor > first { first } else { text::line_start(text, row) }
                }, window, cx);
                true
            }
            Some(Key::Ctrl('r')) => {
                if let Some(session) = &mut self.insert {
                    session.register = true;
                }
                true
            }
            Some(Key::Ctrl('o')) => {
                // One normal-mode command, then back to inserting.
                self.one_shot = self.insert.take();
                self.mode = VimMode::Normal;
                self.read_cursors(state);
                for cursor in &mut self.cursors {
                    cursor.head = cursor.head.min(text::line_end(state.text(), text::row(state.text(), cursor.head)));
                }
                self.apply_mode(state, cx);
                self.capture(state);
                true
            }
            Some(Key::Ctrl('t')) | Some(Key::Ctrl('d')) => {
                ops::shift_cursor_lines(state, key == Some(Key::Ctrl('t')), window, cx);
                true
            }
            Some(Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End | Key::PageUp | Key::PageDown) => {
                if let Some(session) = &mut self.insert {
                    session.moved = true;
                }
                false
            }
            _ => false,
        }
    }

    /// After the cursor moved during an insert, what `.` repeats starts
    /// from where it is now.
    fn resnapshot_if_moved(&mut self, state: &EditorState) {
        if let Some(session) = &mut self.insert
            && session.moved
        {
            session.moved = false;
            session.before = state.text().clone();
            session.cursors = state.selections.iter().map(|s| s.cursor_offset()).collect();
            session.cursors.sort_unstable();
            session.count = 1;
        }
    }

    pub(crate) fn mouse_down(&mut self) {
        if let Some(session) = &mut self.insert {
            session.moved = true;
        }
        self.pending = Default::default();
    }

    fn replace_key(
        &mut self,
        state: &mut EditorState,
        key: Option<Key>,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) -> bool {
        self.resnapshot_if_moved(state);
        match key {
            Some(Key::Escape) | Some(Key::Ctrl('c')) => {
                self.leave_insert(state, window, cx);
                true
            }
            Some(Key::Char(c)) => {
                let replaced = ops::overwrite(state, c, window, cx);
                if let Some(session) = &mut self.insert {
                    session.replaced.push(replaced);
                }
                true
            }
            Some(Key::Backspace) => {
                let restore = self.insert.as_mut().and_then(|s| s.replaced.pop());
                match restore {
                    Some(replaced) => ops::restore_overwritten(state, replaced, window, cx),
                    None => {
                        let text = state.text().clone();
                        self.read_cursors(state);
                        for cursor in &mut self.cursors {
                            cursor.head = text::prev(&text, cursor.head)
                                .max(text::line_start(&text, text::row(&text, cursor.head)));
                        }
                        self.show(state, cx);
                    }
                }
                true
            }
            _ => self.insert_key(state, key, window, cx),
        }
    }

    fn leave_insert(&mut self, state: &mut EditorState, window: &mut Window, cx: &mut Context<EditorState>) {
        self.resnapshot_if_moved(state);
        let Some(session) = self.insert.take() else {
            self.mode = VimMode::Normal;
            self.read_cursors(state);
            self.show(state, cx);
            return;
        };
        let caret = state.selections.iter().map(|s| s.cursor_offset()).min().unwrap_or(0);
        let typed = typed(&session.before, state.text(), &session.cursors, caret);
        if let Some(typed) = &typed
            && session.count > 1
            && !typed.text.is_empty()
        {
            let extra = match session.repeat {
                Repeat::InPlace => typed.text.repeat(session.count - 1),
                Repeat::Lines => format!("\n{}", typed.text).repeat(session.count - 1),
            };
            let text = state.text().clone();
            let at: Vec<usize> = state
                .selections
                .iter()
                .map(|s| match session.repeat {
                    Repeat::InPlace => s.cursor_offset(),
                    Repeat::Lines => text::line_end(&text, text::row(&text, s.cursor_offset())),
                })
                .collect();
            state.transact(window, cx, |state, window, cx| {
                state.edit(at.iter().map(|&o| (o..o, extra.clone())), window, cx)
            });
        }
        if !state.external_history {
            state.undo_manager.merge_after(session.undo_mark.map(|t| t.0));
        }
        if let Some(typed) = &typed {
            registers::set_special(cx, '.', typed.text.clone());
        }
        if let Some(mut change) = session.change
            && !self.replaying
        {
            change.typed = typed;
            self.last_change = Some(change);
        }
        self.mode = VimMode::Normal;
        self.read_cursors(state);
        let text = state.text().clone();
        if let Some(cursor) = self.cursors.first() {
            let at = cursor.head;
            self.set_mark(state, '^', at);
            self.set_mark(state, '.', at);
        }
        for cursor in &mut self.cursors {
            let start = text::line_start(&text, text::row(&text, cursor.head));
            if cursor.head > start {
                cursor.head = text::prev(&text, cursor.head);
            }
            *cursor = Cursor::at(text::clamp_normal(&text, cursor.head), &text);
        }
        if session.block {
            let top = session.cursors.first().copied().unwrap_or(0).min(caret);
            self.cursors = vec![Cursor::at(text::clamp_normal(&text, top), &text)];
        }
        self.show(state, cx);
    }

    fn resume_insert(&mut self, state: &mut EditorState, cx: &mut Context<EditorState>) {
        let Some(session) = self.one_shot.take() else { return };
        self.mode = if session.replaced.is_empty() { VimMode::Insert } else { VimMode::Replace };
        self.insert = Some(InsertSession { moved: true, ..session });
        state.caret_heads = None;
        self.apply_mode(state, cx);
        self.capture(state);
    }

    // ------------------------------------------------------------------
    // The command line

    fn open_command_line(
        &mut self,
        prompt: char,
        text: impl Into<String>,
        pending: Option<normal::Pending>,
        cx: &mut Context<EditorState>,
    ) {
        self.command_line = Some(CommandLine { prompt, text: text.into(), pending, history: None });
        cx.notify();
    }

    fn command_line_key(
        &mut self,
        state: &mut EditorState,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) -> bool {
        let m = &keystroke.modifiers;
        if m.platform && keystroke.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
                && let Some(line) = &mut self.command_line
            {
                line.text.push_str(text.lines().next().unwrap_or(""));
            }
            self.preview_search(state, cx);
            return true;
        }
        let Some(key) = Key::from_keystroke(keystroke) else {
            return !m.platform;
        };
        let Some(line) = &mut self.command_line else { return false };
        match key {
            Key::Escape | Key::Ctrl('c') => {
                self.command_line = None;
                self.highlight_search(state, self.highlight_search, cx);
            }
            Key::Enter => {
                let line = self.command_line.take().unwrap();
                self.run_command_line(state, line, window, cx);
                if !matches!(self.mode, VimMode::Insert | VimMode::Replace) {
                    self.show(state, cx);
                }
                cx.notify();
                return true;
            }
            Key::Backspace if line.text.is_empty() => {
                self.command_line = None;
                self.highlight_search(state, self.highlight_search, cx);
            }
            Key::Backspace => {
                line.text.pop();
            }
            Key::Ctrl('u') => line.text.clear(),
            Key::Ctrl('w') => {
                let trimmed = line.text.trim_end().len();
                let keep = line.text[..trimmed]
                    .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .map_or(0, |i| i + 1);
                line.text.truncate(keep.min(trimmed));
            }
            Key::Up | Key::Down => {
                let history = if line.prompt == ':' { &self.history } else { &self.search_history };
                if !history.is_empty() {
                    let ix = match (line.history, key) {
                        (None, Key::Up) => history.len() - 1,
                        (Some(ix), Key::Up) => ix.saturating_sub(1),
                        (Some(ix), _) if ix + 1 < history.len() => ix + 1,
                        _ => {
                            line.history = None;
                            line.text.clear();
                            return true;
                        }
                    };
                    line.history = Some(ix);
                    line.text = history[ix].clone();
                }
            }
            Key::Char(c) => line.text.push(c),
            Key::Tab => line.text.push(' '),
            _ => {}
        }
        self.preview_search(state, cx);
        cx.notify();
        true
    }

    /// While a search is typed, its matches are highlighted.
    fn preview_search(&mut self, state: &mut EditorState, cx: &mut Context<EditorState>) {
        let Some(line) = &self.command_line else { return };
        if line.prompt == ':' {
            return;
        }
        match search::compile(&line.text).ok().filter(|_| !line.text.is_empty()) {
            Some(regex) => {
                let ranges = search::all(state.text(), &regex);
                state.set_highlight_layer(
                    SEARCH_LAYER,
                    crate::HighlightPriority::SEARCH,
                    ranges.into_iter().map(|r| (r, self.search_style)).collect(),
                    cx,
                );
            }
            None => state.clear_highlight_layer(SEARCH_LAYER, cx),
        }
    }

    /// Shows (or clears) the matches of the last search.
    fn highlight_search(&mut self, state: &mut EditorState, on: bool, cx: &mut Context<EditorState>) {
        self.highlight_search = on;
        let regex = self.last_search.as_ref().and_then(|(pattern, _)| search::compile(pattern).ok());
        match regex.filter(|_| on) {
            Some(regex) => {
                let ranges = search::all(state.text(), &regex);
                state.set_highlight_layer(
                    SEARCH_LAYER,
                    crate::HighlightPriority::SEARCH,
                    ranges.into_iter().map(|r| (r, self.search_style)).collect(),
                    cx,
                );
            }
            None => state.clear_highlight_layer(SEARCH_LAYER, cx),
        }
    }

    fn run_command_line(
        &mut self,
        state: &mut EditorState,
        line: CommandLine,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        if line.prompt == ':' {
            if !line.text.trim().is_empty() {
                self.history.retain(|h| *h != line.text);
                self.history.push(line.text.clone());
                registers::set_special(cx, ':', line.text.clone());
            }
            self.ex(state, &line.text, window, cx);
            return;
        }
        let pattern = if line.text.is_empty() {
            match &self.last_search {
                Some((pattern, _)) => pattern.clone(),
                None => {
                    self.error("E35: No previous regular expression");
                    self.highlight_search(state, false, cx);
                    return;
                }
            }
        } else {
            line.text.clone()
        };
        if let Err(message) = search::compile(&pattern) {
            self.error(message);
            self.highlight_search(state, false, cx);
            return;
        }
        self.search_history.retain(|h| *h != pattern);
        self.search_history.push(pattern.clone());
        registers::set_special(cx, '/', pattern.clone());
        self.last_search = Some((pattern, line.prompt == '/'));
        // The search is a motion: of the cursor, a visual selection, or an
        // operator that was waiting for it (`d/foo`).
        if let Some(pending) = line.pending {
            self.pending = pending;
        }
        self.pending.keys.push(Key::Char('n'));
        self.run_motion(state, motion::Motion::Search { reverse: false }, window, cx);
        self.highlight_search(state, true, cx);
    }

    /// `:` commands.
    fn ex(&mut self, state: &mut EditorState, line: &str, window: &mut Window, cx: &mut Context<EditorState>) {
        let text = state.text().clone();
        let current = text::row(&text, self.primary().head);
        let marks = |name: char| self.mark(state, name).map(|offset| text::row(&text, offset));
        let parsed = command::parse(line, &command::Context { current, last: text::last_row(&text), mark: &marks });
        let command = match parsed {
            Ok(command) => command,
            Err(message) => {
                if !message.is_empty() {
                    self.error(message);
                }
                self.show(state, cx);
                return;
            }
        };
        use command::Command;
        match command {
            Command::Goto(row) => {
                self.set_mark(state, '\'', self.primary().head);
                self.cursors = vec![Cursor::at(text::first_non_blank(&text, row), &text)];
            }
            Command::Write { all, quit } => cx.emit(VimCommand::Write { all, quit }),
            Command::Quit { all, force } => cx.emit(VimCommand::Quit { all, force }),
            Command::Substitute { rows, with } => {
                let substitution = match with {
                    Some(s) => s,
                    None => match &self.last_substitution {
                        Some(last) => command::Substitution { flags: Default::default(), ..last.clone() },
                        None => {
                            self.error("E35: No previous regular expression");
                            self.show(state, cx);
                            return;
                        }
                    },
                };
                self.substitute(state, rows, substitution, window, cx);
            }
            Command::Delete { rows, register } => {
                let range = ops::line_range(&text, *rows.start(), *rows.end());
                self.pending.register = register;
                self.cursors = vec![Cursor::at(range.start.min(text.len()), &text)];
                self.operate_spans(state, ops::Operator::Delete, vec![ops::Span::lines(range)], false, window, cx);
                self.pending = Default::default();
            }
            Command::Yank { rows, register } => {
                let range = ops::line_range(&text, *rows.start(), *rows.end());
                self.pending.register = register;
                let keep = self.cursors.clone();
                self.operate_spans(state, ops::Operator::Yank, vec![ops::Span::lines(range)], false, window, cx);
                self.cursors = keep;
                self.pending = Default::default();
            }
            Command::Join { rows } => {
                self.cursors = vec![Cursor::at(text::line_start(&text, *rows.start()), &text)];
                ops::join(self, state, *rows.start(), *rows.end(), true, window, cx);
            }
            Command::Shift { rows, right, times } => {
                let range = ops::line_range(&text, *rows.start(), *rows.end());
                let op = if right { ops::Operator::Indent } else { ops::Operator::Outdent };
                for _ in 0..times {
                    self.operate_spans(state, op, vec![ops::Span::lines(range.clone())], false, window, cx);
                }
            }
            Command::NoHighlight => self.highlight_search(state, false, cx),
            Command::Undo => normal::undo(self, state, 1, false, window, cx),
            Command::Redo => normal::undo(self, state, 1, true, window, cx),
        }
        self.show(state, cx);
    }

    fn substitute(
        &mut self,
        state: &mut EditorState,
        rows: std::ops::RangeInclusive<usize>,
        substitution: command::Substitution,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        let pattern = if substitution.pattern.is_empty() {
            match &self.last_search {
                Some((pattern, _)) => pattern.clone(),
                None => return self.error("E35: No previous regular expression"),
            }
        } else {
            substitution.pattern.clone()
        };
        let pattern = match substitution.flags.ignore_case {
            Some(true) => format!("\\c{pattern}"),
            Some(false) => format!("\\C{pattern}"),
            None => pattern,
        };
        let regex = match search::compile(&pattern) {
            Ok(regex) => regex,
            Err(message) => return self.error(message),
        };
        self.last_search = Some((substitution.pattern.clone(), true)).filter(|(p, _)| !p.is_empty()).or(self.last_search.take());
        self.last_substitution = Some(substitution.clone());
        let text = state.text().clone();
        let mut edits = vec![];
        let mut lines = vec![];
        for row in rows {
            let (start, end) = (text::line_start(&text, row), text::line_end(&text, row));
            let line = text.slice(start..end).to_string();
            for captures in regex.captures_iter(&line) {
                let whole = captures.get(0).unwrap();
                edits.push((start + whole.start()..start + whole.end(), command::expand(&substitution.replacement, &captures)));
                if lines.last() != Some(&row) {
                    lines.push(row);
                }
                if !substitution.flags.global {
                    break;
                }
            }
        }
        if edits.is_empty() {
            return self.error(format!("E486: Pattern not found: {}", substitution.pattern));
        }
        let count = edits.len();
        let plural = |n: usize, word: &str| if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") };
        if substitution.flags.count_only {
            return self.say(format!("{} on {}", plural(count, "match"), plural(lines.len(), "line")));
        }
        let last_row = *lines.last().unwrap();
        let line_delta: isize = edits
            .iter()
            .map(|(range, new)| new.matches('\n').count() as isize - text.slice(range.clone()).to_string().matches('\n').count() as isize)
            .sum();
        self.cursors = vec![Cursor::at(edits[0].0.start, &text)];
        self.show(state, cx);
        if state.transact(window, cx, |state, window, cx| state.edit(edits, window, cx)).is_none() {
            return;
        }
        let text = state.text().clone();
        let row = (last_row as isize + line_delta).max(0) as usize;
        self.cursors = vec![Cursor::at(text::first_non_blank(&text, row.min(text::last_row(&text))), &text)];
        if count > 1 {
            self.say(format!("{} on {}", plural(count, "substitution"), plural(lines.len(), "line")));
        }
    }
}

/// The ranges a visual block covers, a row each (empty for rows too short
/// to reach it).
fn block_rows(text: &Rope, cursor: Cursor, to_end: bool) -> Vec<Range<usize>> {
    let (a, h) = (text::row(text, cursor.anchor), text::row(text, cursor.head));
    let (ac, hc) = (text::column(text, cursor.anchor), text::column(text, cursor.head));
    let (left, right) = (ac.min(hc), ac.max(hc));
    (a.min(h)..=a.max(h))
        .map(|row| {
            let end_of_line = text::line_end(text, row);
            let start = text::at_column(text, row, left, true);
            let end = if to_end {
                end_of_line
            } else {
                let last = text::at_column(text, row, right, true);
                if last < end_of_line { text::next(text, last) } else { end_of_line }
            };
            start..end.max(start)
        })
        .collect()
}

/// What an insert typed: the text before it and now compared around the
/// first cursor (each cursor typed the same, so the text between the first
/// and second cursor is otherwise unchanged).
fn typed(before: &Rope, now: &Rope, cursors: &[usize], caret: usize) -> Option<Typed> {
    let first = *cursors.first()?;
    let n = cursors.len() as isize;
    let delta = now.len() as isize - before.len() as isize;
    if delta % n != 0 {
        return None;
    }
    let region_end = cursors.get(1).copied().unwrap_or(before.len());
    let now_end = usize::try_from(region_end as isize + delta / n).ok()?.min(now.len());
    let old = before.slice(..region_end);
    let new = now.slice(..now_end);
    let mut prefix = old.bytes().zip(new.bytes()).take(first).take_while(|(a, b)| a == b).count();
    prefix = old.floor_char_boundary(prefix);
    let max_suffix = (region_end - first).min(now_end - prefix);
    let mut old_bytes = old.bytes_at(old.len());
    let mut new_bytes = new.bytes_at(new.len());
    let mut suffix = 0;
    while suffix < max_suffix {
        match (old_bytes.prev(), new_bytes.prev()) {
            (Some(a), Some(b)) if a == b => suffix += 1,
            _ => break,
        }
    }
    let suffix = region_end - old.ceil_char_boundary(region_end - suffix);
    let text = now.slice(prefix..now_end - suffix).to_string();
    let caret = caret.saturating_sub(prefix).min(text.len());
    Some(Typed { before: first - prefix, after: region_end - suffix - first, text, caret })
}

// ----------------------------------------------------------------------
// The editor's side

impl EditorState {
    /// Turns Vim mode on or off for this editor. It starts in normal mode.
    pub fn set_vim_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if enabled == self.extras.vim.is_some() {
            return;
        }
        if !enabled {
            if let Some(vim) = self.extras.vim.take() {
                for (_, anchor) in vim.marks {
                    self.remove_anchor(anchor);
                }
            }
            self.set_keystroke_interceptor(None, cx);
            self.set_cursor_shape(CursorShape::Bar, cx);
            self.set_extra_key_context(None, cx);
            self.caret_heads = None;
            self.clear_highlight_layer(SEARCH_LAYER, cx);
            cx.notify();
            return;
        }
        let observer = cx.observe_self(|state, cx| state.vim_sync(cx));
        let mut vim = Box::new(Vim::new(observer));
        vim.read_cursors(self);
        if self.selections.iter().any(|s| !s.is_empty()) {
            vim.mode = VimMode::Visual;
        }
        vim.show(self, cx);
        self.extras.vim = Some(vim);
        self.set_keystroke_interceptor(
            Some(Rc::new(|state: &mut EditorState, keystroke, window, cx| state.vim_keystroke(keystroke, window, cx))),
            cx,
        );
        cx.notify();
    }

    pub fn is_vim_enabled(&self) -> bool {
        self.extras.vim.is_some()
    }

    /// The mode and what the status bar shows, when Vim mode is on.
    pub fn vim_status(&self) -> Option<VimStatus> {
        self.extras.vim.as_ref().map(|vim| vim.status())
    }

    /// How the matches of a search are highlighted.
    pub fn set_vim_search_style(&mut self, style: HighlightStyle, cx: &mut Context<Self>) {
        if let Some(vim) = &mut self.extras.vim {
            vim.search_style = style;
            if vim.highlight_search {
                let mut vim = self.extras.vim.take().unwrap();
                vim.highlight_search(self, true, cx);
                self.extras.vim = Some(vim);
            }
        }
    }

    fn vim_keystroke(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(mut vim) = self.extras.vim.take() else { return false };
        let handled = vim.keystroke(self, keystroke, window, cx);
        vim.capture(self);
        self.extras.vim = Some(vim);
        handled
    }

    fn vim_sync(&mut self, cx: &mut Context<Self>) {
        let Some(mut vim) = self.extras.vim.take() else { return };
        vim.sync(self, cx);
        self.extras.vim = Some(vim);
    }

    /// A press in the text: an insert's repeat starts over from where it
    /// lands, and a half-typed command is dropped.
    pub(crate) fn vim_mouse_down(&mut self) {
        if let Some(vim) = &mut self.extras.vim {
            vim.mouse_down();
        }
    }

    /// Replaces the selections (the first is the active one) as a motion
    /// does: the view follows `follow` (or the first selection's caret).
    fn vim_write_selections(
        &mut self,
        selections: &[(usize, usize, bool)],
        follow: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let len = self.text.len();
        let current: Vec<(usize, usize, bool)> = self.selections.iter().map(|s| (s.start, s.end, s.reversed)).collect();
        if current == selections {
            return;
        }
        self.undo_manager.break_transaction_coalescing();
        let active = self.active_selection().id;
        let list: Vec<CursorSelection> = selections
            .iter()
            .enumerate()
            .map(|(i, &(start, end, reversed))| {
                let id = if i == 0 { active } else { self.selections.generate_id() };
                let mut selection = CursorSelection::new(id, start.min(len), end.min(len));
                selection.reversed = reversed && start != end;
                selection
            })
            .collect();
        self.selections.replace_all(list);
        self.selections.merge_overlapping();
        self.cursor_line_end_affinity = false;
        self.selected_word_range = None;
        let follow = follow.unwrap_or_else(|| self.cursor());
        self.scroll_to(follow, None, cx);
        self.pause_blink_cursor(cx);
        self.update_preferred_column();
        self.hide_context_menu(cx);
        self.retrigger_signature_help(cx);
        self.clear_inline_completion(cx);
        cx.notify();
    }

    /// Types `text` at every cursor, as the keyboard would.
    fn vim_type(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        use gpui::EntityInputHandler as _;
        self.replace_text_in_range(None, text, window, cx);
    }
}

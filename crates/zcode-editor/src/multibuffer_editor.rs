//! A multibuffer in an editor (roadmap 2.5): one editor showing excerpts of
//! many files, editable in place, every edit reaching its file and every
//! change to a file reaching the excerpts that show it.
//!
//! The editor (the host) shows the model's composed text
//! (`multibuffer.rs`). Each file is a source: an editor showing it, a view
//! linked to the file's buffer, so it shares the file's text, history and
//! anchors. An edit typed in the host goes to its source as an edit there,
//! joining the file's undo history and making it dirty; an edit made to the
//! file anywhere else (its tab, a reload, git) comes back into its
//! excerpts. The newlines between excerpts are read-only, so no edit spans
//! two of them, and so are the excerpts of read-only files.
//!
//! The host's history is external: ⌘Z undoes one step, made of the
//! transactions the step added to each file's history (several files for a
//! replacement across them), by undoing exactly those transactions in the
//! files; the files' changes then come back into the excerpts like any
//! other. A file whose last change isn't the step's any more (it was edited
//! since) keeps its part. A step is what the host records as one
//! transaction: a typed word, a multi-cursor edit, a Replace All.
//!
//! Each excerpt has a header above its first row, the file's name and
//! folder (`⋯` for the file's next excerpt), whose click opens the file
//! there, or shows more lines around the excerpt on its `↕`; the gutter
//! shows each line's number in its file, and a click on one opens the file
//! at that line.

use std::collections::BTreeMap;
use std::ops::Range;
use std::rc::Rc;

use gpui::{Context, Entity, EventEmitter, FontWeight, Hsla, SharedString, Subscription, Window};
use ropey::Rope;

use crate::multibuffer::{ComposedEdit, ExcerptId, MultiBuffer, SourceEdit, SourceId};
use crate::{Block, BlockClicked, BufferEdit, EditorState, GutterLane, HistoryRequest, RopeExt as _, TransactionId};

const HEADERS: &str = "multibuffer-headers";
/// What a header starts with: a click on it shows more lines.
const EXPAND: &str = "↕";
/// Lines of context ⇧↩ and a header's `↕` add above and below an excerpt.
pub const EXPAND_LINES: u32 = 3;

/// How headers look.
#[derive(Clone, Copy, Debug)]
pub struct HeaderStyle {
    pub color: Hsla,
    pub background: Hsla,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MultiBufferEvent {
    /// Open the file at `offset`: a header's click, a line number's, or
    /// [`MultiBufferEditor::open_at_cursor`].
    Open { source: SourceId, offset: usize },
    /// Excerpts were added, removed or grown, or their text changed.
    Changed,
}

struct Source {
    editor: Entity<EditorState>,
    /// The text as of the last edit seen, for rows before a reload.
    text: Rope,
    title: SharedString,
    detail: SharedString,
    _subscription: Subscription,
}

/// One undo step: the transactions it added to each file's history.
struct Step {
    /// The host's transaction the step's edits were typed in.
    host: Option<TransactionId>,
    edits: Vec<(SourceId, TransactionId)>,
}

pub struct MultiBufferEditor {
    host: Entity<EditorState>,
    model: MultiBuffer,
    sources: BTreeMap<SourceId, Source>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Whether the next edit typed in the host's last transaction joins the
    /// last step (a change from elsewhere, undo and redo end it).
    step_open: bool,
    /// Put the host's cursor at the next change coming back from a file
    /// (after undo or redo).
    reveal: bool,
    style: HeaderStyle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<MultiBufferEvent> for MultiBufferEditor {}

impl MultiBufferEditor {
    /// Hosts a multibuffer in `host`, a new, empty editor.
    pub fn new(host: Entity<EditorState>, style: HeaderStyle, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.weak_entity();
        host.update(cx, |state, cx| {
            state.set_external_history(true);
            state.set_extra_key_context(Some("multibuffer".into()), cx);
            let numbers = this.clone();
            state.set_line_number_labels(
                Some(Rc::new(move |row, text, cx| {
                    let this = numbers.upgrade()?;
                    let this = this.read(cx);
                    let (source, offset) = this.source_position(text.line_start_offset(row as usize))?;
                    Some(this.sources.get(&source)?.text.offset_to_point(offset).row as u32 + 1)
                })),
                cx,
            );
            // A click on a line number opens the file there.
            let open = this.clone();
            let lane = GutterLane::new("multibuffer-lines", |_, _| vec![]).order(100).on_click(move |event, _, cx| {
                let row = event.row;
                open.update(cx, |this, cx| this.open_row(row, cx)).is_ok()
            });
            state.set_gutter_lane(lane, cx);
        });
        let subscriptions = vec![
            cx.subscribe_in(&host, window, |this, _, edit: &BufferEdit, window, cx| this.host_edited(edit, window, cx)),
            cx.subscribe_in(&host, window, |this, _, request: &HistoryRequest, window, cx| match request {
                HistoryRequest::Undo => this.undo(window, cx),
                HistoryRequest::Redo => this.redo(window, cx),
            }),
            cx.subscribe_in(&host, window, |this, _, click: &BlockClicked, window, cx| this.header_clicked(click, window, cx)),
        ];
        Self {
            host,
            model: MultiBuffer::new(),
            sources: BTreeMap::new(),
            undo: vec![],
            redo: vec![],
            step_open: false,
            reveal: false,
            style,
            _subscriptions: subscriptions,
        }
    }

    /// The editor the excerpts are shown in.
    pub fn host(&self) -> &Entity<EditorState> {
        &self.host
    }

    pub fn model(&self) -> &MultiBuffer {
        &self.model
    }

    /// Adds a file to take excerpts from: `editor` shows it, and should be a
    /// view linked to the file's buffer that only the multibuffer uses.
    pub fn add_source(
        &mut self,
        editor: Entity<EditorState>,
        title: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SourceId {
        let id = SourceId(editor.entity_id().as_u64());
        if self.sources.contains_key(&id) {
            return id;
        }
        let subscription =
            cx.subscribe_in(&editor, window, move |this, _, edit: &BufferEdit, window, cx| this.source_edited(id, edit, window, cx));
        let text = editor.read(cx).text().clone();
        self.sources.insert(id, Source { editor, text, title: title.into(), detail: detail.into(), _subscription: subscription });
        id
    }

    pub fn source_editor(&self, source: SourceId) -> Option<&Entity<EditorState>> {
        self.sources.get(&source).map(|s| &s.editor)
    }

    /// Every source's editor.
    pub fn source_editors(&self) -> impl Iterator<Item = (SourceId, &Entity<EditorState>)> {
        self.sources.iter().map(|(id, source)| (*id, &source.editor))
    }

    /// Adds excerpts of `source` at the end: its rows `rows` (0-based, end
    /// exclusive). Overlapping or touching ranges become one excerpt, and
    /// one touching the last excerpt (of the same file) grows it.
    pub fn push_excerpts(&mut self, source: SourceId, rows: Vec<Range<u32>>, window: &mut Window, cx: &mut Context<Self>) -> Vec<ExcerptId> {
        let Some(src) = self.sources.get(&source) else { return vec![] };
        let (text, title) = (src.text.clone(), src.title.clone());
        let mut ids = vec![];
        let mut edits = vec![];
        for rows in merge_rows(rows) {
            let range = byte_range(&text, &rows);
            if let Some(last) = self.model.excerpts().last().filter(|last| last.source == source) {
                let last_rows = rows_of(&text, &last.range);
                if rows.start <= last_rows.end {
                    let (id, union) = (last.id, last_rows.start.min(rows.start)..last_rows.end.max(rows.end));
                    let range = byte_range(&text, &union);
                    edits.extend(self.model.set_excerpt_range(id, range.clone(), &text.slice(range).to_string()));
                    continue;
                }
            }
            let at = self.model.len();
            let separator = if self.model.is_empty() { "" } else { "\n" };
            ids.push(self.model.push_excerpt(source, range.clone(), title.clone()));
            edits.push(ComposedEdit { range: at..at, new_text: format!("{separator}{}", text.slice(range)) });
        }
        self.apply_to_host(edits, window, cx);
        self.structure_changed(cx);
        ids
    }

    /// Removes every excerpt and source.
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.host.read(cx).text().len();
        self.apply_to_host(vec![ComposedEdit { range: 0..len, new_text: String::new() }], window, cx);
        self.model = MultiBuffer::new();
        self.sources.clear();
        self.undo.clear();
        self.redo.clear();
        self.step_open = false;
        self.structure_changed(cx);
    }

    /// The file and offset in it of the host's offset `offset`.
    pub fn source_position(&self, offset: usize) -> Option<(SourceId, usize)> {
        self.model.to_source(offset).map(|(_, source, offset)| (source, offset))
    }

    /// Where a file's offset shows in the host, if an excerpt shows it.
    pub fn host_position(&self, source: SourceId, offset: usize) -> Option<usize> {
        self.model.to_composed(source, offset)
    }

    /// The file and 0-based row of the host's row `row`.
    pub fn source_row(&self, row: u32, cx: &gpui::App) -> Option<(SourceId, u32)> {
        let offset = self.host.read(cx).text().line_start_offset(row as usize);
        let (source, offset) = self.source_position(offset)?;
        Some((source, self.sources.get(&source)?.text.offset_to_point(offset).row as u32))
    }

    /// Excerpt `id`'s rows in its file (0-based, end exclusive).
    pub fn excerpt_rows(&self, id: ExcerptId) -> Option<Range<u32>> {
        let excerpt = self.model.excerpt(id)?;
        Some(rows_of(&self.sources.get(&excerpt.source)?.text, &excerpt.range))
    }

    /// The excerpts as shown: file title, rows in the file (1-based, end
    /// inclusive) and the host row of each one's first line.
    pub fn summary(&self, cx: &gpui::App) -> Vec<(SharedString, Range<u32>, u32)> {
        let host_text = self.host.read(cx).text().clone();
        self.model
            .composed_ranges()
            .into_iter()
            .filter_map(|(id, range)| {
                let excerpt = self.model.excerpt(id)?;
                let rows = self.excerpt_rows(id)?;
                let title = self.sources.get(&excerpt.source)?.title.clone();
                Some((title, rows.start + 1..rows.end, host_text.offset_to_point(range.start).row as u32))
            })
            .collect()
    }

    /// Undo steps and redo steps.
    pub fn history_len(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    /// ⌥↩: open the file where the host's cursor is.
    pub fn open_at_cursor(&mut self, cx: &mut Context<Self>) {
        let cursor = self.host.read(cx).cursor();
        if let Some((source, offset)) = self.source_position(cursor) {
            cx.emit(MultiBufferEvent::Open { source, offset });
        }
    }

    fn open_row(&mut self, row: u32, cx: &mut Context<Self>) {
        let offset = self.host.read(cx).text().line_start_offset(row as usize);
        if let Some((source, offset)) = self.source_position(offset) {
            cx.emit(MultiBufferEvent::Open { source, offset });
        }
    }

    /// ⇧↩: more lines around the excerpts the host's cursors are in.
    pub fn expand_at_cursors(&mut self, lines: u32, window: &mut Window, cx: &mut Context<Self>) {
        let ranges = self.host.read(cx).selections.iter().map(|s| s.start.min(s.end)).collect::<Vec<_>>();
        let mut ids: Vec<ExcerptId> = ranges.into_iter().filter_map(|offset| self.model.excerpt_at(offset).map(|e| e.id)).collect();
        ids.dedup();
        for id in ids {
            self.expand(id, lines, lines, window, cx);
        }
    }

    /// Shows `up` more lines above excerpt `id` and `down` below it, as far
    /// as its file goes; it merges with the file's excerpt before or after
    /// it once they touch.
    pub fn expand(&mut self, id: ExcerptId, up: u32, down: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.model.index_of(id) else { return };
        let source = self.model.excerpts()[ix].source;
        let Some(text) = self.sources.get(&source).map(|s| s.text.clone()) else { return };
        let rows = rows_of(&text, &self.model.excerpts()[ix].range);
        let mut wanted = rows.start.saturating_sub(up)..(rows.end + down).min(text.lines_len() as u32);
        let mut edits = vec![];
        let neighbor = |at: Option<usize>, model: &MultiBuffer| {
            let excerpt = model.excerpts().get(at?)?;
            (excerpt.source == source).then(|| (excerpt.id, rows_of(&text, &excerpt.range)))
        };
        if let Some((next, next_rows)) = neighbor(ix.checked_add(1), &self.model)
            && next_rows.start <= wanted.end
        {
            wanted.end = wanted.end.max(next_rows.end);
            edits.extend(self.model.remove_excerpt(next));
        }
        if let Some((previous, previous_rows)) = neighbor(ix.checked_sub(1), &self.model)
            && previous_rows.end >= wanted.start
        {
            wanted.start = wanted.start.min(previous_rows.start);
            edits.extend(self.model.remove_excerpt(previous));
        }
        let range = byte_range(&text, &wanted);
        edits.extend(self.model.set_excerpt_range(id, range.clone(), &text.slice(range).to_string()));
        self.apply_to_host(edits, window, cx);
        self.structure_changed(cx);
    }

    fn header_clicked(&mut self, click: &BlockClicked, window: &mut Window, cx: &mut Context<Self>) {
        if click.key.as_ref() != HEADERS {
            return;
        }
        let offset = self.host.read(cx).text().line_start_offset(click.row as usize);
        let Some(excerpt) = self.model.excerpt_at(offset).filter(|e| self.model.composed_range(e.id).is_some_and(|r| r.start == offset)) else {
            return;
        };
        let (id, source, start) = (excerpt.id, excerpt.source, excerpt.range.start);
        if click.column <= EXPAND.len() {
            self.expand(id, EXPAND_LINES, EXPAND_LINES, window, cx);
        } else {
            cx.emit(MultiBufferEvent::Open { source, offset: start });
        }
    }

    // ---------------------------------------------------------------------
    // Edits both ways

    /// An edit in the host: the user's (the multibuffer's own are quiet).
    fn host_edited(&mut self, edit: &BufferEdit, window: &mut Window, cx: &mut Context<Self>) {
        let composed = ComposedEdit { range: edit.range.clone(), new_text: edit.new_text.clone() };
        let writable = self
            .model
            .to_source(edit.range.start)
            .and_then(|(_, source, _)| self.sources.get(&source))
            .is_some_and(|source| !source.editor.read(cx).is_read_only());
        let source_edit = match writable.then(|| self.model.edit_composed(&composed)) {
            Some(Ok(source_edit)) => source_edit,
            // Across excerpts, or into a read-only file: put the text back.
            _ => return self.revert_host_edit(edit, window, cx),
        };
        let SourceEdit { source: id, range, new_text } = source_edit;
        let Some(source) = self.sources.get_mut(&id) else { return };
        let lines_changed = new_text.contains('\n') || source.text.slice(range.clone()).bytes().any(|b| b == b'\n');
        let host_transaction = self.host.read(cx).last_transaction();
        if !(self.step_open && self.undo.last().is_some_and(|step| step.host == host_transaction)) {
            self.undo.push(Step { host: host_transaction, edits: vec![] });
        }
        self.step_open = true;
        self.redo.clear();
        let step = self.undo.last_mut().expect("a step");
        let joined = step.edits.iter().rev().find(|(s, _)| *s == id).map(|(_, transaction)| *transaction);
        let transaction = source.editor.update(cx, |editor, cx| {
            editor.quiet_edits = true;
            let transaction = editor.edit_joining(joined, [(range, new_text)], window, cx);
            editor.quiet_edits = false;
            transaction
        });
        if let Some(transaction) = transaction.filter(|t| Some(*t) != joined) {
            step.edits.push((id, transaction));
        }
        source.text = source.editor.read(cx).text().clone();
        if lines_changed {
            self.refresh_headers(cx);
        }
        cx.emit(MultiBufferEvent::Changed);
    }

    /// An edit of a file, from anywhere but this multibuffer.
    fn source_edited(&mut self, id: SourceId, edit: &BufferEdit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.sources.get(&id) else { return };
        let old_text = source.text.clone();
        let new_text = source.editor.read(cx).text().clone();
        self.step_open = false;
        let lines_changed = edit.new_text.contains('\n') || old_text.slice(edit.range.clone()).bytes().any(|b| b == b'\n');
        let edits = if edit.range == (0..old_text.len()) && old_text.len() > 0 {
            // Replaced whole (a reload): the excerpts keep their rows.
            self.reload_source(id, &old_text, &new_text)
        } else {
            self.model.edit_source(&SourceEdit { source: id, range: edit.range.clone(), new_text: edit.new_text.clone() })
        };
        if let Some(source) = self.sources.get_mut(&id) {
            source.text = new_text;
        }
        let reveal = std::mem::take(&mut self.reveal)
            .then(|| edits.iter().min_by_key(|e| e.range.start).map(|e| e.range.start + e.new_text.len()))
            .flatten();
        self.apply_to_host(edits, window, cx);
        if let Some(offset) = reveal {
            self.host.update(cx, |host, cx| host.set_selected_range(offset..offset, cx));
        }
        if lines_changed {
            self.refresh_headers(cx);
        }
        cx.emit(MultiBufferEvent::Changed);
    }

    /// A file replaced whole: each excerpt shows the same rows of its new text.
    fn reload_source(&mut self, id: SourceId, old: &Rope, new: &Rope) -> Vec<ComposedEdit> {
        let excerpts: Vec<(ExcerptId, Range<u32>)> = self
            .model
            .excerpts()
            .iter()
            .filter(|excerpt| excerpt.source == id)
            .map(|excerpt| (excerpt.id, rows_of(old, &excerpt.range)))
            .collect();
        let lines = new.lines_len() as u32;
        let mut edits = vec![];
        for (excerpt, rows) in excerpts {
            let rows = rows.start.min(lines.saturating_sub(1))..rows.end.min(lines).max(rows.start.min(lines.saturating_sub(1)) + 1);
            let range = byte_range(new, &rows);
            edits.extend(self.model.reset_excerpt(excerpt, range.clone(), &new.slice(range).to_string()));
        }
        edits
    }

    /// Undoes the last step in the files it edited. A file whose part is
    /// undone already (from its own tab) keeps it in the step; one edited
    /// since loses it.
    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replay(true, window, cx);
    }

    /// Redoes the last undone step.
    pub fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replay(false, window, cx);
    }

    fn replay(&mut self, undo: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.step_open = false;
        loop {
            let Some(step) = (if undo { self.undo.pop() } else { self.redo.pop() }) else { return };
            let mut kept = vec![];
            let mut changed = false;
            let parts: Vec<(SourceId, TransactionId)> =
                if undo { step.edits.iter().rev().copied().collect() } else { step.edits.clone() };
            for (source, transaction) in parts {
                let Some(editor) = self.sources.get(&source).map(|s| s.editor.clone()) else { continue };
                let (replayed, already) = editor.update(cx, |editor, cx| {
                    if undo {
                        (editor.undo_transaction(transaction, window, cx), editor.next_redo_transaction() == Some(transaction))
                    } else {
                        (editor.redo_transaction(transaction, window, cx), editor.last_transaction() == Some(transaction))
                    }
                });
                changed |= replayed;
                if replayed || already {
                    kept.push((source, transaction));
                }
            }
            if undo {
                kept.reverse();
            }
            if !kept.is_empty() {
                let step = Step { host: step.host, edits: kept };
                if undo { self.redo.push(step) } else { self.undo.push(step) }
            }
            if changed {
                self.reveal = true;
                return;
            }
        }
    }

    /// Puts back the text of an edit the files can't take.
    fn revert_host_edit(&mut self, edit: &BufferEdit, window: &mut Window, cx: &mut Context<Self>) {
        let old = self.composed_text(edit.range.clone());
        let range = edit.range.start..edit.range.start + edit.new_text.len();
        self.apply_to_host(vec![ComposedEdit { range, new_text: old }], window, cx);
    }

    /// The composed text of `range`, from the sources.
    fn composed_text(&self, range: Range<usize>) -> String {
        let mut text = String::new();
        let ranges = self.model.composed_ranges();
        for (ix, (id, composed)) in ranges.iter().enumerate() {
            let Some(excerpt) = self.model.excerpt(*id) else { continue };
            let start = range.start.max(composed.start);
            let end = range.end.min(composed.end);
            if start < end
                && let Some(source) = self.sources.get(&excerpt.source)
            {
                let at = excerpt.range.start - composed.start;
                text.push_str(&source.text.slice(start + at..end + at).to_string());
            }
            if ix + 1 < ranges.len() && range.start <= composed.end && composed.end < range.end {
                text.push('\n');
            }
        }
        text
    }

    /// Applies edits of the composed text to the host, in order, without
    /// taking them for the user's.
    fn apply_to_host(&mut self, edits: Vec<ComposedEdit>, window: &mut Window, cx: &mut Context<Self>) {
        if edits.is_empty() {
            return;
        }
        self.host.update(cx, |host, cx| {
            host.quiet_edits = true;
            for edit in edits {
                host.edit_keeping_selections(edit.range, &edit.new_text, window, cx);
            }
            host.quiet_edits = false;
        });
    }

    /// After excerpts were added, removed or grown.
    fn structure_changed(&mut self, cx: &mut Context<Self>) {
        let mut read_only = self.model.separators();
        for (id, range) in self.model.composed_ranges() {
            let read_only_file = self
                .model
                .excerpt(id)
                .and_then(|excerpt| self.sources.get(&excerpt.source))
                .is_some_and(|source| source.editor.read(cx).is_read_only());
            if read_only_file {
                read_only.push(range);
            }
        }
        self.host.update(cx, |host, cx| host.set_read_only_ranges(read_only, cx));
        self.refresh_headers(cx);
        cx.emit(MultiBufferEvent::Changed);
    }

    /// A header above each excerpt's first row.
    fn refresh_headers(&mut self, cx: &mut Context<Self>) {
        let host_text = self.host.read(cx).text().clone();
        let mut previous = None;
        let blocks: Vec<Block> = self
            .model
            .composed_ranges()
            .into_iter()
            .filter_map(|(id, range)| {
                let excerpt = self.model.excerpt(id)?;
                let source = self.sources.get(&excerpt.source)?;
                let text = if previous == Some(excerpt.source) {
                    format!("{EXPAND} ⋯")
                } else if source.detail.is_empty() {
                    format!("{EXPAND} {}", source.title)
                } else {
                    format!("{EXPAND} {}  {}", source.title, source.detail)
                };
                previous = Some(excerpt.source);
                let row = host_text.offset_to_point(range.start).row as u32;
                Some(
                    Block::new(row, [text])
                        .above()
                        .clickable()
                        .font_weight(FontWeight::SEMIBOLD)
                        .color(self.style.color)
                        .background(self.style.background),
                )
            })
            .collect();
        self.host.update(cx, |host, cx| host.set_blocks(HEADERS, blocks, cx));
    }
}

/// Sorted row ranges with the overlapping and touching ones merged.
fn merge_rows(mut rows: Vec<Range<u32>>) -> Vec<Range<u32>> {
    rows.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<u32>> = vec![];
    for rows in rows.into_iter().filter(|r| !r.is_empty()) {
        match merged.last_mut() {
            Some(last) if rows.start <= last.end => last.end = last.end.max(rows.end),
            _ => merged.push(rows),
        }
    }
    merged
}

/// The bytes of rows `rows` (end exclusive) of `text`, without the last
/// row's newline.
fn byte_range(text: &Rope, rows: &Range<u32>) -> Range<usize> {
    let lines = text.lines_len().max(1);
    let first = (rows.start as usize).min(lines - 1);
    let last = (rows.end.max(rows.start + 1) as usize - 1).min(lines - 1).max(first);
    text.line_start_offset(first)..text.line_end_offset(last)
}

/// The rows (end exclusive) bytes `range` of `text` cover.
fn rows_of(text: &Rope, range: &Range<usize>) -> Range<u32> {
    let start = text.offset_to_point(range.start.min(text.len())).row as u32;
    let end = text.offset_to_point(range.end.min(text.len())).row as u32;
    start..end + 1
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{AppContext as _, TestAppContext, VisualTestContext, hsla};

    use super::*;
    use crate::test_support::open_editor;

    const A: &str = "fn a() {\n    one();\n    two();\n}\n\nfn b() {\n    three();\n}\n";
    const B: &str = "struct B;\nimpl B {\n    fn two() {}\n}\n";

    struct Fixture {
        mb: Entity<MultiBufferEditor>,
        host: Entity<EditorState>,
        a: Entity<EditorState>,
        b: Entity<EditorState>,
        ids: (SourceId, SourceId),
        events: Rc<RefCell<Vec<MultiBufferEvent>>>,
        _subscription: Subscription,
    }

    /// A multibuffer showing rows 1–2 of A and row 2 of B.
    fn fixture(cx: &mut TestAppContext) -> (Fixture, VisualTestContext) {
        let (host, mut cx) = open_editor(cx, "");
        let (mb, a, b, ids, events, subscription) = cx.update(|window, cx| {
            let a = cx.new(|cx| EditorState::new(window, cx).default_value(A));
            let b = cx.new(|cx| EditorState::new(window, cx).default_value(B));
            let style = HeaderStyle { color: hsla(0., 0., 0.5, 1.), background: hsla(0., 0., 0.9, 1.) };
            let mb = cx.new(|cx| MultiBufferEditor::new(host.clone(), style, window, cx));
            let ids = mb.update(cx, |mb, cx| {
                let ids = (mb.add_source(a.clone(), "a.rs", "src", window, cx), mb.add_source(b.clone(), "b.rs", "src", window, cx));
                mb.push_excerpts(ids.0, vec![1..3], window, cx);
                mb.push_excerpts(ids.1, vec![2..3], window, cx);
                ids
            });
            let events: Rc<RefCell<Vec<MultiBufferEvent>>> = Default::default();
            let log = events.clone();
            let subscription = cx.subscribe(&mb, move |_, event: &MultiBufferEvent, _| log.borrow_mut().push(event.clone()));
            (mb, a, b, ids, events, subscription)
        });
        cx.run_until_parked();
        (Fixture { mb, host, a, b, ids, events, _subscription: subscription }, cx)
    }

    fn text(editor: &Entity<EditorState>, cx: &mut VisualTestContext) -> String {
        editor.read_with(cx, |state, _| state.text().to_string())
    }

    #[gpui::test]
    fn excerpts_show_their_files_lines_with_headers_and_numbers(cx: &mut TestAppContext) {
        let (f, mut cx) = fixture(cx);
        assert_eq!(text(&f.host, &mut cx), "    one();\n    two();\n    fn two() {}");
        let headers: Vec<(u32, SharedString)> = f.host.read_with(&cx, |host, _| host.blocks(HEADERS).into_iter().map(|b| (b.row, b.lines[0].clone())).collect());
        assert_eq!(headers, [(0, "↕ a.rs  src".into()), (2, "↕ b.rs  src".into())]);
        let numbers: Vec<Option<u32>> = cx.update(|_, cx| (0..3).map(|row| f.host.read(cx).gutter.line_number(row, f.host.read(cx).text(), cx)).collect());
        assert_eq!(numbers, [Some(2), Some(3), Some(3)]);
        let summary = cx.update(|_, cx| f.mb.read(cx).summary(cx));
        assert_eq!(summary, [("a.rs".into(), 2..3, 0), ("b.rs".into(), 3..3, 2)]);
    }

    #[gpui::test]
    fn edits_reach_the_files_and_the_files_edits_come_back(cx: &mut TestAppContext) {
        let (f, mut cx) = fixture(cx);
        // Typing in the multibuffer edits the file, in the file's own history.
        cx.update(|window, cx| f.host.update(cx, |host, cx| host.edit([(4..7, "uno")], window, cx)));
        cx.run_until_parked();
        assert_eq!(text(&f.a, &mut cx), A.replace("one", "uno"));
        assert!(f.a.read_with(&cx, |a, _| a.can_undo()));
        // Editing the file elsewhere comes back, above the excerpt or in it.
        cx.update(|window, cx| f.a.update(cx, |a, cx| a.edit([(0..0, "// top\n")], window, cx)));
        cx.run_until_parked();
        assert_eq!(text(&f.host, &mut cx), "    uno();\n    two();\n    fn two() {}");
        let two = text(&f.a, &mut cx).find("two").unwrap();
        cx.update(|window, cx| f.a.update(cx, |a, cx| a.edit([(two..two + 3, "dos")], window, cx)));
        cx.run_until_parked();
        assert_eq!(text(&f.host, &mut cx), "    uno();\n    dos();\n    fn two() {}");
        // The numbers follow the line inserted above.
        let first = cx.update(|_, cx| f.host.read(cx).gutter.line_number(0, f.host.read(cx).text(), cx));
        assert_eq!(first, Some(3));
        // Backspace at an excerpt's start would join it to the one before: refused.
        cx.update(|window, cx| {
            f.host.update(cx, |host, cx| {
                host.focus(window, cx);
                host.set_selected_range(22..22, cx);
            })
        });
        cx.simulate_keystrokes("backspace");
        assert_eq!(text(&f.host, &mut cx), "    uno();\n    dos();\n    fn two() {}");
        assert_eq!(text(&f.b, &mut cx), B);
    }

    #[gpui::test]
    fn one_undo_step_for_an_edit_across_files(cx: &mut TestAppContext) {
        let (f, mut cx) = fixture(cx);
        // One transaction in the multibuffer touching both files.
        cx.update(|window, cx| {
            f.host.update(cx, |host, cx| {
                host.transact(window, cx, |host, window, cx| host.edit([(4..7, "ONE"), (29..32, "TWO")], window, cx));
            })
        });
        cx.run_until_parked();
        assert_eq!(text(&f.a, &mut cx), A.replace("one", "ONE"));
        assert_eq!(text(&f.b, &mut cx), B.replace("two", "TWO"));
        assert_eq!(f.mb.read_with(&cx, |mb, _| mb.history_len()), (1, 0));
        // ⌘Z in the multibuffer undoes it in both files, as their own undo.
        cx.update(|window, cx| f.host.update(cx, |host, cx| host.undo(window, cx)));
        cx.run_until_parked();
        assert_eq!((text(&f.a, &mut cx), text(&f.b, &mut cx)), (A.to_string(), B.to_string()));
        assert_eq!(text(&f.host, &mut cx), "    one();\n    two();\n    fn two() {}");
        assert!(f.a.read_with(&cx, |a, _| !a.can_undo() && a.can_redo()), "the file's history agrees");
        // Redo from the file itself, then undo it from the multibuffer.
        cx.update(|window, cx| f.a.update(cx, |a, cx| a.redo(window, cx)));
        cx.run_until_parked();
        assert_eq!(text(&f.host, &mut cx), "    ONE();\n    two();\n    fn two() {}");
        cx.update(|window, cx| f.host.update(cx, |host, cx| host.redo(window, cx)));
        cx.run_until_parked();
        assert_eq!(text(&f.b, &mut cx), B.replace("two", "TWO"));
        assert_eq!(text(&f.a, &mut cx), A.replace("one", "ONE"), "the part redone already stays");
        cx.update(|window, cx| f.host.update(cx, |host, cx| host.undo(window, cx)));
        cx.run_until_parked();
        assert_eq!((text(&f.a, &mut cx), text(&f.b, &mut cx)), (A.to_string(), B.to_string()));
    }

    #[gpui::test]
    fn typing_is_one_step_and_a_file_edited_since_keeps_its_part(cx: &mut TestAppContext) {
        let (f, mut cx) = fixture(cx);
        cx.update(|window, cx| {
            f.host.update(cx, |host, cx| {
                host.focus(window, cx);
                host.set_selected_range(10..10, cx);
            })
        });
        cx.simulate_input("abc");
        assert_eq!(text(&f.a, &mut cx), A.replacen("one();", "one();abc", 1));
        assert_eq!(f.mb.read_with(&cx, |mb, _| mb.history_len()), (1, 0));
        // Then B's line, and B edited in its own tab.
        let end = text(&f.host, &mut cx).len();
        cx.update(|_, cx| f.host.update(cx, |host, cx| host.set_selected_range(end..end, cx)));
        cx.simulate_input("!");
        assert_eq!(text(&f.b, &mut cx), B.replacen("{}", "{}!", 1));
        assert_eq!(f.mb.read_with(&cx, |mb, _| mb.history_len()), (2, 0));
        cx.update(|window, cx| f.b.update(cx, |b, cx| b.edit([(0..0, "// b\n")], window, cx)));
        cx.run_until_parked();
        // B was edited since: undoing the step leaves B's part.
        cx.update(|window, cx| f.host.update(cx, |host, cx| host.undo(window, cx)));
        cx.run_until_parked();
        assert!(text(&f.b, &mut cx).contains("{}!"));
        assert_eq!(f.mb.read_with(&cx, |mb, _| mb.history_len()), (0, 1), "the stale step is dropped, the typing undone");
        assert_eq!(text(&f.a, &mut cx), A);
    }

    #[gpui::test]
    fn excerpts_grow_merge_and_reload(cx: &mut TestAppContext) {
        let (f, mut cx) = fixture(cx);
        cx.update(|window, cx| {
            f.mb.update(cx, |mb, cx| {
                let first = mb.model().excerpts()[0].id;
                mb.expand(first, 1, 3, window, cx);
            })
        });
        cx.run_until_parked();
        assert_eq!(text(&f.host, &mut cx), "fn a() {\n    one();\n    two();\n}\n\nfn b() {\n    fn two() {}");
        // A later excerpt of A, after B's.
        let a_id = f.ids.0;
        cx.update(|window, cx| f.mb.update(cx, |mb, cx| mb.push_excerpts(a_id, vec![7..8], window, cx)));
        cx.run_until_parked();
        let summary = cx.update(|_, cx| f.mb.read(cx).summary(cx));
        assert_eq!(summary.iter().map(|(t, r, _)| (t.to_string(), r.clone())).collect::<Vec<_>>(), [("a.rs".into(), 1..6), ("b.rs".into(), 3..3), ("a.rs".into(), 8..8)]);
        // Reloaded whole (another tool rewrote it): the excerpts keep their rows.
        cx.update(|window, cx| f.a.update(cx, |a, cx| a.set_value(A.to_uppercase(), window, cx)));
        cx.run_until_parked();
        assert_eq!(text(&f.host, &mut cx), "FN A() {\n    ONE();\n    TWO();\n}\n\nFN B() {\n    fn two() {}\n}");
        let headers: Vec<u32> = f.host.read_with(&cx, |host, _| host.blocks(HEADERS).into_iter().map(|b| b.row).collect());
        assert_eq!(headers, [0, 6, 7]);
    }

    #[gpui::test]
    fn headers_and_line_numbers_open_the_file(cx: &mut TestAppContext) {
        let (f, mut cx) = fixture(cx);
        let painted = f.host.read_with(&cx, |host, _| host.painted_blocks());
        let b_header = painted.iter().find(|(_, row, ..)| *row == 2).unwrap().3;
        let press = |cx: &mut VisualTestContext, position| {
            cx.simulate_event(gpui::MouseDownEvent { position, modifiers: gpui::Modifiers::none(), button: gpui::MouseButton::Left, click_count: 1, first_mouse: false });
            cx.simulate_event(gpui::MouseUpEvent { position, modifiers: gpui::Modifiers::none(), button: gpui::MouseButton::Left, click_count: 1 });
            cx.run_until_parked();
        };
        // The header's title opens the file at the excerpt.
        press(&mut cx, gpui::point(b_header.origin.x + gpui::px(200.), b_header.center().y));
        let b_start = B.find("    fn two").unwrap();
        assert_eq!(f.events.borrow().last(), Some(&MultiBufferEvent::Open { source: f.ids.1, offset: b_start }));
        // A line number: the file at that line.
        let (numbers, top) = f.host.read_with(&cx, |host, _| (host.gutter_geometry().numbers, host.gutter_geometry().rows[1].1));
        press(&mut cx, gpui::point(numbers.center().x, top + gpui::px(4.)));
        assert_eq!(f.events.borrow().last(), Some(&MultiBufferEvent::Open { source: f.ids.0, offset: A.find("    two").unwrap() }));
        // The header's ↕ shows more lines.
        press(&mut cx, gpui::point(b_header.origin.x + gpui::px(10.), b_header.center().y));
        assert_eq!(text(&f.host, &mut cx), format!("    one();\n    two();\n{B}"), "to the file's (empty) last row");
    }

    #[gpui::test]
    fn read_only_files_refuse_edits(cx: &mut TestAppContext) {
        let (f, mut cx) = fixture(cx);
        cx.update(|window, cx| {
            f.b.update(cx, |b, cx| b.set_read_only(true, cx));
            f.mb.update(cx, |mb, cx| mb.expand_at_cursors(0, window, cx));
            f.host.update(cx, |host, cx| {
                host.focus(window, cx);
                let end = host.text().len();
                host.set_selected_range(end..end, cx);
            });
        });
        cx.simulate_input("x");
        assert_eq!(text(&f.b, &mut cx), B);
        assert_eq!(text(&f.host, &mut cx), "    one();\n    two();\n    fn two() {}");
    }
}

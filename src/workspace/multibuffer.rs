//! Multibuffers: Find in Workspace's results, the issues with their
//! context, references and rename previews as a tab of excerpts of many
//! files, edited in place (`zcode_editor::MultiBufferEditor`).
//!
//! Each file an excerpt shows is a buffer like any open file's: the tab
//! takes a view linked to it, so typing in an excerpt edits the file
//! (dirty, saved with ⌘S from the tab, undone in its own tab too) and the
//! file's edits show in the excerpt. A file nobody had open is read
//! without a language server document or highlighting until it's edited or
//! shown in a pane. Find results are added a page of files at a time as the
//! tab scrolls, so a search with thousands of matches opens as fast as one
//! with ten.
//!
//! Over the excerpts: match highlights, each file's diagnostics, issue
//! messages under their lines, change bars and breakpoints in the gutter
//! (a breakpoint toggles there too). Bookmarks and test diamonds aren't
//! shown. ⌥↩ opens the file at the cursor, ⇧↩ shows more lines around the
//! excerpts at the cursors.

use std::collections::{BTreeMap, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, notification::Notification, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zcode_editor::multibuffer::SourceId;
use zcode_editor::{
    Block, Diagnostic as EditorDiagnostic, Editor, EditorState, GutterItem, GutterLane, GutterPlacement, HeaderStyle,
    HighlightPriority, MultiBufferEditor, MultiBufferEvent, Rope, RopeExt as _,
};

use super::pane::{Item, MultiBufferItem};
use super::{Target, Workspace};
use crate::build::log::Severity;
use crate::git::diff::HunkKind;
use crate::lsp;

/// Lines shown above and below a match, a reference or an issue.
const CONTEXT_LINES: u32 = 1;
/// Files a tab adds at a time, as it scrolls.
const PAGE_FILES: usize = 25;
const MATCHES: &str = "multibuffer-matches";
const MESSAGES: &str = "multibuffer-issue-messages";

#[derive(Clone, Debug, PartialEq)]
pub enum MultiBufferKind {
    /// Find in Workspace's results for a query.
    Find { query: String },
    /// Find Selected Symbol in Workspace (the Find navigator's title).
    References { title: String },
    Issues,
    Rename { from: String, to: String },
}

impl MultiBufferKind {
    pub fn title(&self) -> String {
        match self {
            Self::Find { query } => format!("Find “{query}”"),
            Self::References { title } => title.clone(),
            Self::Issues => "Issues".into(),
            Self::Rename { from, to } => format!("Rename “{from}” to “{to}”"),
        }
    }

    fn icon(&self) -> IconName {
        match self {
            Self::Find { .. } | Self::References { .. } => IconName::Search,
            Self::Issues => IconName::TriangleAlert,
            Self::Rename { .. } => IconName::Pencil,
        }
    }
}

/// A file a tab shows, with the places in it to show: 0-based lines and
/// the bytes to highlight in each (UTF-16 column and byte length).
#[derive(Clone, Debug)]
pub(super) struct FilePlaces {
    pub path: PathBuf,
    pub places: Vec<Place>,
}

#[derive(Clone, Debug)]
pub(super) struct Place {
    pub line: u32,
    pub column: u32,
    pub len: usize,
    /// An issue's message, shown under its line.
    pub message: Option<(Severity, String)>,
}

pub struct MultiBufferView {
    pub kind: MultiBufferKind,
    pub editor: Entity<MultiBufferEditor>,
    pub host: Entity<EditorState>,
    /// The file each source is.
    pub paths: BTreeMap<SourceId, PathBuf>,
    /// Files still to add, as the tab scrolls.
    pending: VecDeque<FilePlaces>,
    /// How many pages were added (each page's matches are a layer).
    pages: usize,
    /// The issue messages, by file and 0-based line.
    messages: Vec<(PathBuf, u32, Severity, String)>,
    /// What the tab's bar says.
    pub summary: String,
    /// How long adding the last page took, in milliseconds.
    last_page_ms: f64,
    font_size: f32,
}

impl Focusable for MultiBufferView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.host.read(cx).focus_handle(cx)
    }
}

impl MultiBufferView {
    pub fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        if self.font_size != size {
            self.font_size = size;
            cx.notify();
        }
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The source showing `path`.
    pub fn source_of(&self, path: &Path) -> Option<SourceId> {
        self.paths.iter().find(|(_, p)| p.as_path() == path).map(|(id, _)| *id)
    }

    /// Every file and the editor the tab shows it through.
    pub fn source_views(&self, cx: &App) -> Vec<(PathBuf, Entity<EditorState>)> {
        let editor = self.editor.read(cx);
        self.paths.iter().filter_map(|(id, path)| Some((path.clone(), editor.source_editor(*id)?.clone()))).collect()
    }

    pub fn summary_json(&self, cx: &App) -> serde_json::Value {
        let editor = self.editor.read(cx);
        let host = self.host.read(cx);
        let line_height = host.line_height().map(f32::from).unwrap_or(16.);
        let rows = host.gutter_geometry().rows.iter().take(60).map(|&(row, top, _)| (row, f32::from(top) + line_height / 2.)).collect::<Vec<_>>();
        let numbers = host.gutter_geometry().numbers;
        let painted_numbers: Vec<serde_json::Value> = host
            .gutter_geometry()
            .rows
            .iter()
            .take(60)
            .filter_map(|&(row, top, _)| {
                let (source, line) = editor.source_row(row, cx)?;
                let path = self.paths.get(&source)?;
                Some(serde_json::json!({
                    "row": row,
                    "file": super::pane::file_title(path),
                    "line": line + 1,
                    "x": f32::from(numbers.center().x),
                    "y": f32::from(top) + line_height / 2.,
                }))
            })
            .collect();
        let (undo, redo) = editor.history_len();
        let position = host.cursor_position();
        let mut summary = serde_json::json!({
            "kind": format!("{:?}", self.kind).split([' ', '{']).next().unwrap_or_default(),
            "title": self.kind.title(),
            "summary": self.summary,
            "excerpts": editor.summary(cx).iter().map(|(title, rows, row)| serde_json::json!({ "file": title.to_string(), "lines": [rows.start, rows.end], "row": row })).collect::<Vec<_>>(),
            "files": self.paths.values().map(|p| super::pane::file_title(p)).collect::<Vec<_>>(),
            "pending_files": self.pending.len(),
            "pages": self.pages,
            "last_page_ms": self.last_page_ms,
            "text": (host.text().len() < 16000).then(|| host.text().to_string()),
            "line": position.line + 1,
            "column": position.character + 1,
            "undo": undo,
            "redo": redo,
            "matches": (1..=self.pages).map(|page| format!("{MATCHES}-{page}")).chain([format!("{MATCHES}-renamed")]).map(|key| host.highlight_layer_ranges(&key).len()).sum::<usize>(),
            "diagnostics": host.diagnostics().map(|d| d.len()).unwrap_or(0),
            "messages": host.blocks(MESSAGES).iter().map(|b| (b.row + 1, b.lines[0].to_string())).collect::<Vec<_>>(),
            "headers": host.painted_blocks().iter().map(|(key, row, _, bounds)| serde_json::json!({
                "key": key.to_string(),
                "row": row,
                "x": f32::from(bounds.origin.x),
                "y": f32::from(bounds.center().y),
            })).collect::<Vec<_>>(),
        });
        // Added separately: one `json!` this size reaches the macro recursion limit.
        summary["rows"] = serde_json::json!(rows);
        summary["numbers"] = painted_numbers.into();
        summary["painted_gutter"] = serde_json::json!(host.painted_gutter_items().iter().map(|item| (item.lane.to_string(), item.row)).collect::<Vec<_>>());
        summary["read_only_ranges"] = host.read_only_ranges().len().into();
        summary
    }
}

impl Render for MultiBufferView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let issues = self.kind == MultiBufferKind::Issues;
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(px(28.))
                    .px_2()
                    .gap_1()
                    .text_xs()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().flex_1().truncate().text_color(theme.muted_foreground).child(self.summary.clone()))
                    .when(self.has_pending(), |el| {
                        el.child(Button::new("multibuffer-more").label("Show All").ghost().xsmall().on_click(cx.listener(|this, _, _, cx| {
                            cx.emit(MultiBufferViewEvent::ShowAll);
                            let _ = this;
                        })))
                    })
                    .when(issues, |el| {
                        el.child(
                            Button::new("multibuffer-refresh")
                                .icon(IconName::RefreshCw)
                                .ghost()
                                .xsmall()
                                .tooltip("Refresh")
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(MultiBufferViewEvent::Refresh))),
                        )
                    }),
            )
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(&self.host)
                        .bordered(false)
                        .p_0()
                        .h_full()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(px(self.font_size)),
                ),
            )
    }
}

pub enum MultiBufferViewEvent {
    /// Add every file still pending.
    ShowAll,
    /// Build the issues' excerpts again.
    Refresh,
}

impl EventEmitter<MultiBufferViewEvent> for MultiBufferView {}

impl Workspace {
    /// The focused pane's multibuffer, when it shows one.
    pub fn active_multibuffer(&self) -> Option<&Entity<MultiBufferView>> {
        match self.pane(self.active_pane)?.active_item()? {
            Item::MultiBuffer(item) => Some(&item.view),
            _ => None,
        }
    }

    /// The file and 0-based line at the active multibuffer's cursor, and
    /// the multibuffer's editor (where the cursor is on that line).
    pub(super) fn multibuffer_cursor_line(&self, cx: &App) -> Option<(PathBuf, u32, Entity<EditorState>)> {
        let view = self.active_multibuffer()?.read(cx);
        let row = view.host.read(cx).cursor_position().line;
        let (source, line) = view.editor.read(cx).source_row(row, cx)?;
        Some((view.paths.get(&source)?.clone(), line, view.host.clone()))
    }

    /// Every open multibuffer.
    pub(super) fn multibuffers(&self) -> Vec<Entity<MultiBufferView>> {
        self.panes
            .iter()
            .flat_map(|pane| pane.items.iter())
            .filter_map(|item| match item {
                Item::MultiBuffer(item) => Some(item.view.clone()),
                _ => None,
            })
            .collect()
    }

    /// The views multibuffers show `path` through.
    pub(super) fn multibuffer_views_of(&self, path: &Path, cx: &App) -> Vec<Entity<EditorState>> {
        self.multibuffers()
            .iter()
            .filter_map(|view| {
                let view = view.read(cx);
                let source = view.source_of(path)?;
                view.editor.read(cx).source_editor(source).cloned()
            })
            .collect()
    }

    // ---------------------------------------------------------------------
    // Opening one

    /// Find in Workspace's results (or the references the Find navigator
    /// shows) in a tab, to edit in place.
    pub fn show_find_results_in_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<MultiBufferView>> {
        if self.find.searching || self.find.results.is_empty() {
            return None;
        }
        let kind = match &self.find.title {
            Some(title) => MultiBufferKind::References { title: title.clone() },
            None => MultiBufferKind::Find { query: self.find.input.read(cx).value().to_string() },
        };
        let files = self
            .find
            .results
            .iter()
            .map(|file| FilePlaces {
                path: file.path.clone(),
                places: file.matches.iter().map(|m| Place { line: m.line, column: m.column, len: m.range.len(), message: None }).collect(),
            })
            .collect();
        let total: usize = self.find.results.iter().map(|f| f.matches.len()).sum();
        let summary = format!("{} in {} · ⌥↩ opens the file at the cursor, ⇧↩ shows more lines", plural(total, "result"), plural(self.find.results.len(), "file"));
        self.open_multibuffer(kind, files, summary, window, cx)
    }

    /// Every issue with the lines around it in a tab, its message under it.
    pub fn show_issues_in_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<MultiBufferView>> {
        let (files, count) = self.issue_places(cx);
        let summary = format!("{} in {}", plural(count, "issue"), plural(files.len(), "file"));
        self.open_multibuffer(MultiBufferKind::Issues, files, summary, window, cx)
    }

    fn issue_places(&self, cx: &App) -> (Vec<FilePlaces>, usize) {
        let mut files: Vec<FilePlaces> = vec![];
        let mut count = 0;
        for issue in self.all_issues(cx) {
            let Some(path) = issue.path.clone().filter(|p| p.is_file()) else { continue };
            let place = Place {
                line: issue.line.unwrap_or(1).saturating_sub(1),
                column: issue.column.unwrap_or(1).saturating_sub(1),
                len: 0,
                message: Some((issue.severity, issue.message.clone())),
            };
            count += 1;
            match files.iter_mut().find(|f| f.path == path) {
                Some(file) => file.places.push(place),
                None => files.push(FilePlaces { path, places: vec![place] }),
            }
        }
        for file in &mut files {
            file.places.sort_by_key(|p| p.line);
        }
        (files, count)
    }

    /// Opens a multibuffer tab of `kind` (Find, references and Issues reuse
    /// theirs) in the focused pane with the first page of `files`.
    pub(super) fn open_multibuffer(
        &mut self,
        kind: MultiBufferKind,
        files: Vec<FilePlaces>,
        summary: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<MultiBufferView>> {
        let pane = self.active_pane;
        let reuse = !matches!(kind, MultiBufferKind::Rename { .. });
        let same_kind = |a: &MultiBufferKind, b: &MultiBufferKind| std::mem::discriminant(a) == std::mem::discriminant(b);
        // A tab of the same kind shows the new places instead, unless it has
        // the only view of a file with unsaved changes.
        let existing = self.pane(pane).and_then(|p| {
            p.items.iter().position(|item| {
                matches!(item, Item::MultiBuffer(mb) if reuse && same_kind(&mb.view.read(cx).kind, &kind))
                    && self.unsaved_if_closed([item], cx).is_empty()
            })
        });
        if let Some(ix) = existing {
            let view = match &mut self.pane_mut(pane)?.items[ix] {
                Item::MultiBuffer(item) => {
                    item.title = kind.title();
                    item.view.clone()
                }
                _ => return None,
            };
            let closed = self.clear_multibuffer(&view, window, cx);
            view.update(cx, |view, cx| {
                view.kind = kind;
                view.summary = summary;
                view.pending = files.into();
                view.pages = 0;
                view.messages.clear();
                cx.notify();
            });
            for (path, editor) in closed {
                self.view_closed(&path, editor, cx);
            }
            self.load_multibuffer_page(&view, window, cx);
            self.activate_item(pane, ix, window, cx);
            return Some(view);
        }
        let language = majority_language(&files);
        let config = super::settings::EditorConfig { soft_wrap: false, ..self.editor_config() };
        let host = cx.new(|cx| Self::new_editor(&config, language, window, cx));
        let theme = cx.theme();
        let style = HeaderStyle { color: theme.foreground, background: theme.muted.opacity(0.6) };
        let editor = cx.new(|cx| MultiBufferEditor::new(host.clone(), style, window, cx));
        let font_size = self.font_size;
        let view = cx.new(|_| MultiBufferView {
            kind,
            editor: editor.clone(),
            host: host.clone(),
            paths: BTreeMap::new(),
            pending: files.into(),
            pages: 0,
            messages: vec![],
            summary,
            last_page_ms: 0.,
            font_size,
        });
        self.install_multibuffer_lanes(&view, cx);
        let view_id = view.entity_id();
        let focus = host.read(cx).focus_handle(cx);
        let subscriptions = vec![
            cx.observe(&view, |_, _, cx| cx.notify()),
            cx.on_focus_in(&focus, window, move |this, window, cx| this.view_focused(view_id, window, cx)),
            cx.subscribe_in(&editor, window, {
                let view = view.clone();
                move |this, _, event: &MultiBufferEvent, window, cx| this.on_multibuffer_event(&view, event, window, cx)
            }),
            cx.subscribe_in(&view, window, |this, view, event: &MultiBufferViewEvent, window, cx| match event {
                MultiBufferViewEvent::ShowAll => {
                    while view.read(cx).has_pending() {
                        this.load_multibuffer_page(view, window, cx);
                    }
                }
                MultiBufferViewEvent::Refresh => {
                    this.show_issues_in_editor(window, cx);
                }
            }),
            // More files as it scrolls near the end.
            cx.subscribe_in(&host, window, {
                let view = view.clone();
                move |this, host, event: &zcode_editor::ViewportChanged, window, cx| {
                    let lines = host.read(cx).text().lines_len() as u32;
                    let screen = event.rows.len() as u32;
                    if view.read(cx).has_pending() && event.rows.end + 2 * screen >= lines {
                        this.load_multibuffer_page(&view, window, cx);
                    }
                }
            }),
        ];
        let mut subscriptions = subscriptions;
        subscriptions.extend(self.install_vim(&host, window, cx));
        let title = view.read(cx).kind.title();
        let p = self.pane_mut(pane)?;
        let ix = (p.active + 1).min(p.items.len());
        p.items.insert(ix, Item::MultiBuffer(MultiBufferItem { view: view.clone(), host, title, _subscriptions: subscriptions }));
        self.load_multibuffer_page(&view, window, cx);
        self.activate_item(pane, ix, window, cx);
        Some(view)
    }

    /// Adds the next page of a tab's files.
    pub(super) fn load_multibuffer_page(&mut self, view: &Entity<MultiBufferView>, window: &mut Window, cx: &mut Context<Self>) {
        let files: Vec<FilePlaces> = view.update(cx, |view, _| {
            let n = PAGE_FILES.min(view.pending.len());
            view.pending.drain(..n).collect()
        });
        if files.is_empty() {
            return;
        }
        let started = std::time::Instant::now();
        let editor = view.read(cx).editor.clone();
        let mut highlights = vec![];
        let mut messages = vec![];
        let mut new_paths = vec![];
        let style = HighlightStyle { background_color: Some(cx.theme().warning.opacity(0.35)), ..Default::default() };
        for file in files {
            let Some((source, created)) = self.multibuffer_source(view, &file.path, window, cx) else { continue };
            if created {
                new_paths.push(file.path.clone());
            }
            let text = editor.read(cx).source_editor(source).map(|e| e.read(cx).text().clone()).unwrap_or_default();
            let places = self.places_now(&file, &text, cx);
            let rows = places.iter().map(|(line, ..)| line.saturating_sub(CONTEXT_LINES)..line + CONTEXT_LINES + 1).collect();
            editor.update(cx, |editor, cx| editor.push_excerpts(source, rows, window, cx));
            let editor = editor.read(cx);
            for (line, range, message) in places {
                let start = text.line_start_offset(line as usize);
                if let (Some(start), Some(end)) =
                    (editor.host_position(source, start + range.start), editor.host_position(source, start + range.end))
                {
                    highlights.push((start..end, style));
                }
                if let Some((severity, message)) = message {
                    messages.push((file.path.clone(), line, severity, message));
                }
            }
        }
        let page = view.update(cx, |view, _| {
            view.pages += 1;
            view.messages.extend(messages);
            view.pages
        });
        let host = view.read(cx).host.clone();
        host.update(cx, |host, cx| host.set_highlight_layer(format!("{MATCHES}-{page}"), HighlightPriority::SEARCH, highlights, cx));
        self.refresh_issue_messages(view, cx);
        self.refresh_view_diagnostics(view, cx);
        self.load_git_bases(new_paths, cx);
        view.update(cx, |view, _| view.last_page_ms = started.elapsed().as_secs_f64() * 1000.);
    }

    /// `file`'s places as they are in the buffer: line, the bytes to
    /// highlight in it, and the message. Find results are of the text when
    /// the search ran, so in a file with unsaved changes (perhaps made
    /// since) the query runs again.
    fn places_now(&self, file: &FilePlaces, text: &Rope, cx: &App) -> Vec<(u32, Range<usize>, Option<(Severity, String)>)> {
        let dirty = self.buffer(&file.path).is_some_and(|b| b.dirty);
        let query = self.find.input.read(cx).value().to_string();
        if dirty
            && self.find.title.is_none()
            && file.places.iter().all(|p| p.message.is_none())
            && let Ok(regex) = crate::search::build_regex(&query, &self.find.options)
        {
            let mut places = vec![];
            for row in 0..text.lines_len() {
                let line = text.slice_line(row).to_string();
                for m in regex.find_iter(&line) {
                    places.push((row as u32, m.range(), None));
                }
            }
            return places;
        }
        file.places
            .iter()
            .filter(|place| (place.line as usize) < text.lines_len())
            .map(|place| {
                let line_text = text.slice_line(place.line as usize).to_string();
                let start = utf16_to_byte(&line_text, place.column as usize);
                let end = (start + place.len).min(line_text.len());
                (place.line, start..end, place.message.clone())
            })
            .collect()
    }

    /// The source showing `path` in `view`, made (and the file's buffer
    /// opened) if needed; whether it was made.
    fn multibuffer_source(&mut self, view: &Entity<MultiBufferView>, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Option<(SourceId, bool)> {
        if let Some(source) = view.read(cx).source_of(path) {
            return Some((source, false));
        }
        if self.buffer(path).is_none() && !self.open_buffer_with(path, true, window, cx) {
            return None;
        }
        let buffer = self.buffer(path)?;
        let (shared, read_only) = (buffer.editor.clone(), buffer.read_only);
        let linked = cx.new(|cx| EditorState::new(window, cx));
        EditorState::link(&shared, &linked, window, cx);
        linked.update(cx, |state, cx| state.set_read_only(read_only, cx));
        let title = super::pane::file_title(path);
        let detail = self.relative(path.parent().unwrap_or(path));
        let editor = view.read(cx).editor.clone();
        let source = editor.update(cx, |editor, cx| editor.add_source(linked, title, detail, window, cx));
        view.update(cx, |view, _| view.paths.insert(source, path.to_path_buf()));
        Some((source, true))
    }

    /// Takes every excerpt and source out of a tab; returns the files and
    /// views it let go of, for `view_closed`.
    fn clear_multibuffer(&mut self, view: &Entity<MultiBufferView>, window: &mut Window, cx: &mut Context<Self>) -> Vec<(PathBuf, EntityId)> {
        let views: Vec<(PathBuf, EntityId)> = view.read(cx).source_views(cx).into_iter().map(|(path, editor)| (path, editor.entity_id())).collect();
        let (editor, host) = (view.read(cx).editor.clone(), view.read(cx).host.clone());
        editor.update(cx, |editor, cx| editor.clear(window, cx));
        view.update(cx, |view, _| view.paths.clear());
        host.update(cx, |host, cx| {
            for page in 1..=view.read(cx).pages {
                host.set_highlight_layer(format!("{MATCHES}-{page}"), HighlightPriority::SEARCH, vec![], cx);
            }
            host.clear_blocks(MESSAGES, cx);
        });
        views
    }

    fn on_multibuffer_event(&mut self, view: &Entity<MultiBufferView>, event: &MultiBufferEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            MultiBufferEvent::Open { source, offset } => {
                let Some(path) = view.read(cx).paths.get(source).cloned() else { return };
                self.open_file(path, Some(Target::Offset(*offset)), true, window, cx);
            }
            MultiBufferEvent::ExcerptsChanged => {
                self.refresh_issue_messages(view, cx);
                self.refresh_view_diagnostics(view, cx);
            }
            MultiBufferEvent::Edited => cx.notify(),
        }
    }

    // ---------------------------------------------------------------------
    // What's drawn over the excerpts

    /// Diagnostics of every multibuffer showing `path`.
    pub(super) fn refresh_multibuffer_diagnostics_for(&mut self, path: &Path, cx: &mut Context<Self>) {
        for view in self.multibuffers() {
            if view.read(cx).source_of(path).is_some() {
                self.refresh_view_diagnostics(&view, cx);
                if view.read(cx).kind == MultiBufferKind::Issues {
                    view.update(cx, |view, cx| {
                        view.summary = format!("{} · issues changed: Refresh to see them all", view.summary.split(" · ").next().unwrap_or_default());
                        cx.notify();
                    });
                }
            }
        }
    }

    /// Each file's diagnostics, where its excerpts show them.
    fn refresh_view_diagnostics(&mut self, view: &Entity<MultiBufferView>, cx: &mut Context<Self>) {
        let (editor, host) = (view.read(cx).editor.clone(), view.read(cx).host.clone());
        let mut entries = vec![];
        let host_text = host.read(cx).text().clone();
        for (path, source_view) in view.read(cx).source_views(cx) {
            let text = source_view.read(cx).text().clone();
            let source = SourceId(source_view.entity_id().as_u64());
            for d in self.file_diagnostics(&path, cx) {
                let range = lsp::lsp_range_to_offsets(&text, d.range);
                let editor = editor.read(cx);
                let (Some(start), Some(end)) = (editor.host_position(source, range.start), editor.host_position(source, range.end)) else {
                    continue;
                };
                let position = |offset| host_text.offset_to_position(offset);
                entries.push(EditorDiagnostic::new(position(start)..position(end), d.message).with_severity(d.severity).with_source(d.source));
            }
        }
        host.update(cx, |host, cx| {
            if let Some(set) = host.diagnostics_mut() {
                set.clear();
                set.extend(entries);
            }
            cx.notify();
        });
    }

    /// An issue tab's messages, under their lines.
    fn refresh_issue_messages(&mut self, view: &Entity<MultiBufferView>, cx: &mut Context<Self>) {
        let theme = cx.theme();
        let (error, warning) = (theme.danger, theme.warning);
        let (editor, host) = (view.read(cx).editor.clone(), view.read(cx).host.clone());
        let host_text = host.read(cx).text().clone();
        let blocks: Vec<Block> = view
            .read(cx)
            .messages
            .iter()
            .filter_map(|(path, line, severity, message)| {
                let view = view.read(cx);
                let source = view.source_of(path)?;
                let source_view = editor.read(cx).source_editor(source)?;
                let text = source_view.read(cx).text();
                let start = text.line_start_offset(*line as usize);
                let row = host_text.offset_to_point(editor.read(cx).host_position(source, start)?).row as u32;
                let (glyph, color) = if *severity == Severity::Error { ("⛔", error) } else { ("⚠", warning) };
                // Under the line's text, at its indentation.
                let line_text = text.slice_line(*line as usize).to_string();
                let indent = &line_text[..line_text.len() - line_text.trim_start().len()];
                let first = message.lines().next().unwrap_or_default();
                Some(Block::new(row, [format!("{indent}{glyph} {first}")]).color(color))
            })
            .collect();
        host.update(cx, |host, cx| host.set_blocks(MESSAGES, blocks, cx));
    }

    /// Breakpoints and change bars on the excerpts' lines, as in a file's tab.
    fn install_multibuffer_lanes(&self, view: &Entity<MultiBufferView>, cx: &mut Context<Self>) {
        let host = view.read(cx).host.clone();
        let workspace = cx.weak_entity();
        let weak_view = view.downgrade();
        // The file and 1-based line of each visible row.
        let lines = Rc::new(move |rows: Range<u32>, cx: &App| -> Vec<(u32, PathBuf, u32)> {
            let Some(view) = weak_view.upgrade() else { return vec![] };
            let view = view.read(cx);
            let editor = view.editor.read(cx);
            rows.filter_map(|row| {
                let (source, line) = editor.source_row(row, cx)?;
                Some((row, view.paths.get(&source)?.clone(), line + 1))
            })
            .collect()
        });
        let breakpoints = {
            let (workspace, lines) = (workspace.clone(), lines.clone());
            GutterLane::new("multibuffer-breakpoints", move |rows, cx| {
                let Some(this) = workspace.upgrade() else { return vec![] };
                let debugger = this.read(cx).debugger.read(cx);
                let active = debugger.breakpoints_active;
                lines(rows.rows.clone(), cx)
                    .into_iter()
                    .filter_map(|(row, path, line)| {
                        let enabled = debugger.breakpoint_at(&path, line)?.enabled;
                        let workspace = workspace.clone();
                        Some(
                            GutterItem::new(row, move |paint, window, cx| {
                                super::breakpoints::paint_marker(window, paint.bounds, super::breakpoints::marker_color(enabled, active));
                                paint.paint_line_number(white(), window, cx);
                            })
                            .on_click(move |event, window, cx| {
                                let (path, position, clicks) = (path.clone(), event.position, event.click_count);
                                workspace.update(cx, |this, cx| this.gutter_click(path, line, clicks, position, window, cx)).is_ok()
                            }),
                        )
                    })
                    .collect()
            })
            .order(0)
        };
        let changes = {
            let lines = lines.clone();
            GutterLane::new("multibuffer-changes", move |rows, cx| {
                let Some(this) = workspace.upgrade() else { return vec![] };
                let this = this.read(cx);
                let colors = Rc::new(super::change_bars::BarColors::new(cx));
                let mut items = vec![];
                for (row, path, line) in lines(rows.rows.clone(), cx) {
                    let Some(buffer) = this.buffer(&path) else { continue };
                    let row0 = line - 1;
                    let Some((ix, hunk)) = buffer.git.hunks.iter().enumerate().find(|(_, h)| match h.hunk.kind() {
                        HunkKind::Deleted => h.hunk.new.start == row0,
                        _ => h.hunk.new.contains(&row0),
                    }) else {
                        continue;
                    };
                    let (kind, staging, colors, workspace, path) = (hunk.hunk.kind(), hunk.staging, colors.clone(), workspace.clone(), path.clone());
                    items.push(
                        GutterItem::new(row, move |paint, window, _| {
                            super::change_bars::paint_bar(window, paint.cell, kind, staging, false, paint.hovered, &colors);
                        })
                        .cursor(CursorStyle::PointingHand)
                        .on_click(move |event, window, cx| {
                            let position = point(event.bounds.right(), event.bounds.top());
                            let (workspace, path) = (workspace.clone(), path.clone());
                            window.defer(cx, move |window, cx| {
                                workspace.update(cx, |this, cx| this.show_hunk_popover(path, ix, position, window, cx)).ok();
                            });
                            true
                        }),
                    );
                }
                items
            })
            .placement(GutterPlacement::AfterNumbers(px(super::change_bars::LANE_WIDTH)))
        };
        host.update(cx, |host, cx| {
            host.set_gutter_lane(breakpoints, cx);
            host.set_gutter_lane(changes, cx);
        });
    }

    // ---------------------------------------------------------------------
    // Replace and rename through a multibuffer

    /// Find ▸ Find and Replace in Workspace's Replace All: every result
    /// replaced in the results' tab, as one edit ⌘Z undoes across files;
    /// the files are left unsaved for review.
    pub fn replace_all_in_workspace(&mut self, replacement: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.title.is_some() {
            return;
        }
        let query = self.find.input.read(cx).value().to_string();
        let Ok(regex) = crate::search::build_regex(&query, &self.find.options) else { return };
        let Some(view) = self.show_find_results_in_editor(window, cx) else { return };
        while view.read(cx).has_pending() {
            self.load_multibuffer_page(&view, window, cx);
        }
        let host = view.read(cx).host.clone();
        let text = host.read(cx).text().clone();
        let use_captures = self.find.options.regex;
        let mut edits = vec![];
        for row in 0..text.lines_len() {
            let start = text.line_start_offset(row);
            let line = text.slice_line(row).to_string();
            for captures in regex.captures_iter(&line) {
                let Some(m) = captures.get(0) else { continue };
                let mut new_text = String::new();
                if use_captures {
                    captures.expand(&replacement, &mut new_text);
                } else {
                    new_text.push_str(&replacement);
                }
                edits.push((start + m.start()..start + m.end(), new_text));
            }
        }
        let (count, files) = (edits.len(), view.read(cx).paths.len());
        host.update(cx, |host, cx| {
            host.transact(window, cx, |host, window, cx| host.edit(edits, window, cx));
        });
        view.update(cx, |view, cx| {
            view.summary = format!("Replaced {} in {} · unsaved: ⌘S saves them, ⌘Z undoes it", plural(count, "result"), plural(files, "file"));
            cx.notify();
        });
    }

    /// Rename: the language server's edits made in a tab of the places
    /// they change, as one edit ⌘Z undoes across files; nothing is saved.
    pub(super) fn apply_rename(&mut self, edit: lsp_types::WorkspaceEdit, from: String, to: String, window: &mut Window, cx: &mut Context<Self>) {
        let mut shown = vec![];
        let mut elsewhere = vec![];
        for (path, edits) in super::buffers::workspace_edit_files(edit) {
            let path = path.canonicalize().unwrap_or(path);
            if crate::packages::location::is_package_checkout(&path) {
                continue;
            }
            if path.is_file() {
                shown.push((path, edits));
            } else {
                elsewhere.push((path, edits));
            }
        }
        if !elsewhere.is_empty() {
            self.apply_file_edits(elsewhere, window, cx);
        }
        if shown.is_empty() {
            return;
        }
        shown.sort_by(|a, b| a.0.cmp(&b.0));
        let files: Vec<FilePlaces> = shown
            .iter()
            .map(|(path, edits)| FilePlaces {
                path: path.clone(),
                places: edits.iter().map(|e| Place { line: e.range.start.line, column: e.range.start.character, len: 0, message: None }).collect(),
            })
            .collect();
        let count: usize = shown.iter().map(|(_, edits)| edits.len()).sum();
        let summary = format!("{} in {} · unsaved: ⌘S saves them, ⌘Z undoes the rename", plural(count, "change"), plural(shown.len(), "file"));
        let Some(view) = self.open_multibuffer(MultiBufferKind::Rename { from, to }, files, summary, window, cx) else { return };
        while view.read(cx).has_pending() {
            self.load_multibuffer_page(&view, window, cx);
        }
        let editor = view.read(cx).editor.clone();
        let mut host_edits: Vec<(Range<usize>, String)> = vec![];
        let mut direct = vec![];
        for (path, edits) in shown {
            let Some(source) = view.read(cx).source_of(&path) else { continue };
            let Some(text) = editor.read(cx).source_editor(source).map(|e| e.read(cx).text().clone()) else { continue };
            for edit in edits {
                let range = lsp::lsp_range_to_offsets(&text, edit.range);
                let mapped = editor.read(cx).host_position(source, range.start).zip(editor.read(cx).host_position(source, range.end));
                match mapped {
                    Some((start, end)) => host_edits.push((start..end, edit.new_text)),
                    None => direct.push((path.clone(), vec![edit])),
                }
            }
        }
        // The renamed text, where it ends up.
        host_edits.sort_by_key(|(range, _)| range.start);
        let mut delta = 0isize;
        let renamed: Vec<Range<usize>> = host_edits
            .iter()
            .map(|(range, text)| {
                let start = (range.start as isize + delta) as usize;
                delta += text.len() as isize - range.len() as isize;
                start..start + text.len()
            })
            .collect();
        let host = view.read(cx).host.clone();
        let style = HighlightStyle { background_color: Some(cx.theme().info.opacity(0.25)), ..Default::default() };
        host.update(cx, |host, cx| {
            host.transact(window, cx, |host, window, cx| host.edit(host_edits, window, cx));
            for page in 1..=view.read(cx).pages {
                host.set_highlight_layer(format!("{MATCHES}-{page}"), HighlightPriority::SEARCH, vec![], cx);
            }
            host.set_highlight_layer(format!("{MATCHES}-renamed"), HighlightPriority::SEARCH, renamed.into_iter().map(|r| (r, style)).collect(), cx);
        });
        if !direct.is_empty() {
            self.apply_file_edits(direct, window, cx);
        }
        window.push_notification(Notification::info(format!("Renamed in {}: review, then Save", plural(view.read(cx).paths.len(), "file"))), cx);
    }

    // ---------------------------------------------------------------------
    // The tab's commands

    /// ⌥↩ in a multibuffer: the file at the cursor.
    pub(super) fn open_excerpts(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = self.active_multibuffer().cloned() {
            let editor = view.read(cx).editor.clone();
            editor.update(cx, |editor, cx| editor.open_at_cursor(cx));
        }
    }

    /// ⇧↩ in a multibuffer: more lines around the excerpts at the cursors.
    pub(super) fn expand_excerpts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.active_multibuffer().cloned() {
            let editor = view.read(cx).editor.clone();
            editor.update(cx, |editor, cx| editor.expand_at_cursors(zcode_editor::EXPAND_LINES, window, cx));
        }
    }

    /// ⌘S in a multibuffer: every file it changed.
    pub(super) fn save_multibuffer(&mut self, view: &Entity<MultiBufferView>, window: &mut Window, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = view.read(cx).paths.values().cloned().collect();
        for path in paths {
            if self.buffer(&path).is_some_and(|b| b.dirty) {
                self.save_path(&path, window, cx);
            }
        }
        cx.notify();
    }

    /// A debug-channel command for multibuffers: `find` (the Find
    /// navigator's results in a tab), `issues`, `replace <text>` (Replace
    /// All), `open` (⌥↩), `expand` (⇧↩), `all` (every pending file),
    /// `save` (⌘S).
    pub fn multibuffer_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
        match verb {
            "find" => self.show_find_results_in_editor(window, cx).map(|_| ()).ok_or_else(|| "no find results".to_string()),
            "issues" => self.show_issues_in_editor(window, cx).map(|_| ()).ok_or_else(|| "couldn’t open".to_string()),
            "replace" => {
                self.replace_all_in_workspace(arg.to_string(), window, cx);
                Ok(())
            }
            "open" => {
                self.open_excerpts(cx);
                Ok(())
            }
            "expand" => {
                self.expand_excerpts(window, cx);
                Ok(())
            }
            "all" => {
                let view = self.active_multibuffer().cloned().ok_or("no multibuffer")?;
                while view.read(cx).has_pending() {
                    self.load_multibuffer_page(&view, window, cx);
                }
                Ok(())
            }
            "save" => {
                let view = self.active_multibuffer().cloned().ok_or("no multibuffer")?;
                self.save_multibuffer(&view, window, cx);
                Ok(())
            }
            other => Err(format!("unknown multibuffer command {other}")),
        }
    }

    pub(super) fn multibuffer_summary(&self, cx: &App) -> serde_json::Value {
        let open: Vec<String> = self.multibuffers().iter().map(|view| view.read(cx).kind.title()).collect();
        serde_json::json!({
            "open": open,
            "active": self.active_multibuffer().map(|view| {
                let mut summary = view.read(cx).summary_json(cx);
                let dirty: Vec<String> = view
                    .read(cx)
                    .paths
                    .values()
                    .filter(|path| self.buffer(path).is_some_and(|b| b.dirty))
                    .map(|path| super::pane::file_title(path))
                    .collect();
                summary["dirty"] = dirty.into();
                summary
            }),
        })
    }
}

impl MultiBufferItem {
    pub fn icon(&self, cx: &App) -> IconName {
        self.view.read(cx).kind.icon()
    }
}

/// The language most of `files` are in, for the tab's highlighting.
fn majority_language(files: &[FilePlaces]) -> &'static str {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for file in files.iter().take(200) {
        *counts.entry(Workspace::language_for(&file.path)).or_default() += 1;
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(language, _)| language).unwrap_or("text")
}

fn utf16_to_byte(line: &str, column: usize) -> usize {
    let mut units = 0;
    for (ix, ch) in line.char_indices() {
        if units >= column {
            return ix;
        }
        units += ch.len_utf16();
    }
    line.len()
}

pub(super) fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

fn white() -> Hsla {
    hsla(0., 0., 1., 1.)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{FilePlaces, majority_language, plural, utf16_to_byte};

    #[test]
    fn columns_and_languages() {
        assert_eq!(utf16_to_byte("let é = 1", 6), 7);
        assert_eq!(utf16_to_byte("a😀b", 3), 5);
        assert_eq!(utf16_to_byte("ab", 9), 2);
        let file = |name: &str| FilePlaces { path: PathBuf::from(name), places: vec![] };
        assert_eq!(majority_language(&[file("a.swift"), file("b.md"), file("c.swift")]), "swift");
        assert_eq!(majority_language(&[]), "text");
        assert_eq!(plural(1, "result"), "1 result");
        assert_eq!(plural(3, "file"), "3 files");
    }
}

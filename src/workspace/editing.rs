//! Editor commands that go beyond the text engine's built-ins, and
//! LSP-backed commands (references, rename, formatting, outline).

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use zcode_editor::{Copy, Cut, EditorState, GoToDefinition, IndentInline, Paste, Rope, RopeExt as _, ToggleCodeActions};
use gpui_kit::*;
use serde_json::{Value, json};

use super::pickers::{self, OutlineEntry};
use super::{NavigatorTab, Target, Workspace};
use crate::actions::*;
use crate::lsp::{self, offset_to_lsp, path_to_uri};
use crate::search::{FileMatches, LineMatch};

/// Byte range of the full lines touched by `selection` (excluding a final
/// line the selection merely ends at column 0 of).
fn line_block(text: &Rope, selection: &Range<usize>) -> Range<usize> {
    let start_row = text.offset_to_point(selection.start).row;
    let mut end_row = text.offset_to_point(selection.end).row;
    if end_row > start_row && text.line_start_offset(end_row) == selection.end {
        end_row -= 1;
    }
    let start = text.line_start_offset(start_row);
    let end = text.line_end_offset(end_row);
    start..end
}

/// The line containing `offset`, including its trailing newline when it has
/// one.
fn whole_line(text: &Rope, offset: usize) -> Range<usize> {
    let block = line_block(text, &(offset..offset));
    let end = if block.end < text.len() { block.end + 1 } else { block.end };
    block.start..end
}

/// Clipboard metadata key marking text that was cut or copied as a whole
/// line, so pasting it with nothing selected inserts above the current line
/// (as in Zed).
const WHOLE_LINE_KEY: &str = "zcode_whole_line";

fn whole_line_clipboard(text: &Rope, offset: usize) -> (Range<usize>, ClipboardItem) {
    let line = whole_line(text, offset);
    let mut content = text.slice(line.clone()).to_string();
    if !content.ends_with('\n') {
        content.push('\n');
    }
    let item = ClipboardItem::new_string_with_json_metadata(content, json!({ WHOLE_LINE_KEY: true }));
    (line, item)
}

fn is_whole_line(item: &ClipboardItem) -> bool {
    item.metadata()
        .and_then(|m| serde_json::from_str::<Value>(m).ok())
        .is_some_and(|m| m[WHOLE_LINE_KEY] == json!(true))
}

fn comment_prefix(language: &str) -> Option<&'static str> {
    match language {
        "swift" | "c" | "cpp" | "rust" | "javascript" | "typescript" | "go" | "java" | "kotlin" => {
            Some("//")
        }
        "yaml" | "toml" | "bash" | "python" | "ruby" | "make" => Some("#"),
        _ => None,
    }
}

/// Comments or uncomments every line of `block` (Xcode's ⌘/).
pub fn toggle_comment_text(block: &str, prefix: &str) -> String {
    let lines: Vec<&str> = block.split('\n').collect();
    let non_blank: Vec<&&str> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let all_commented = !non_blank.is_empty()
        && non_blank.iter().all(|l| l.trim_start().starts_with(prefix));
    if all_commented {
        lines
            .iter()
            .map(|line| {
                let indent = line.len() - line.trim_start().len();
                let rest = &line[indent..];
                match rest.strip_prefix(prefix) {
                    Some(after) => {
                        let after = after.strip_prefix(' ').unwrap_or(after);
                        format!("{}{}", &line[..indent], after)
                    }
                    None => line.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        let min_indent = non_blank
            .iter()
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        lines
            .iter()
            .map(|line| {
                if line.trim().is_empty() {
                    line.to_string()
                } else {
                    format!("{}{} {}", &line[..min_indent], prefix, &line[min_indent..])
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Workspace {
    fn with_active_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut EditorState, SharedString, &mut Window, &mut Context<EditorState>),
    ) {
        // A property list's outline handles its own editing commands.
        let Some(editor) = self.active_editor() else { return };
        let language = self.active_buffer().map(|b| b.language.clone()).unwrap_or_default();
        editor.update(cx, |state, cx| f(state, language, window, cx));
    }

    /// Like `with_active_editor`, for commands that change the text: the
    /// command is one undo step, and a read-only editor (package sources)
    /// refuses it.
    fn transact_in_active_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut EditorState, SharedString, &mut Window, &mut Context<EditorState>),
    ) {
        self.with_active_editor(window, cx, |state, language, window, cx| {
            state.transact(window, cx, |state, window, cx| f(state, language, window, cx));
        });
    }

    /// The active editor when it has focus and nothing selected — the case
    /// where cut, copy and paste act on whole lines.
    fn line_edit_target(&self, window: &Window, cx: &App) -> Option<Entity<EditorState>> {
        self.active_editor().filter(|editor| {
            let state = editor.read(cx);
            state.focus_handle(cx).is_focused(window) && state.selected_range().is_empty()
        })
    }

    /// ⌘X: cuts the selection, or the current line when nothing is selected.
    pub(super) fn cut_or_cut_line(&mut self, _: &CutOrCutLine, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self
            .line_edit_target(window, cx)
            .filter(|editor| editor.read(cx).is_editable())
        else {
            window.dispatch_action(Box::new(Cut), cx);
            return;
        };
        editor.update(cx, |state, cx| {
            let text = state.text().clone();
            let cursor = state.cursor();
            let (line, item) = whole_line_clipboard(&text, cursor);
            cx.write_to_clipboard(item);
            // The last line has no newline of its own; take the one before
            // it so no empty line is left behind.
            let removed = if line.end == text.len() && line.start > 0 {
                line.start - 1..line.end
            } else {
                line.clone()
            };
            state.set_selected_range(removed.clone(), cx);
            state.replace("", window, cx);
            // Keep the cursor's column on the line that took its place.
            let text = state.text().clone();
            let row = text.offset_to_point(removed.start.min(text.len())).row;
            let start = text.line_start_offset(row);
            let target = (start + cursor - line.start).min(text.line_end_offset(row));
            state.set_selected_range(target..target, cx);
        });
    }

    /// ⌘C: copies the selection, or the current line when nothing is
    /// selected.
    pub(super) fn copy_or_copy_line(&mut self, _: &CopyOrCopyLine, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.line_edit_target(window, cx) else {
            window.dispatch_action(Box::new(Copy), cx);
            return;
        };
        let state = editor.read(cx);
        let (_, item) = whole_line_clipboard(state.text(), state.cursor());
        cx.write_to_clipboard(item);
    }

    /// ⌘V: text cut or copied as a whole line goes above the current line
    /// when nothing is selected; anything else pastes at the cursor.
    pub(super) fn paste_or_paste_line(&mut self, _: &PasteOrPasteLine, window: &mut Window, cx: &mut Context<Self>) {
        let line = cx
            .read_from_clipboard()
            .filter(is_whole_line)
            .and_then(|item| item.text());
        let editor = self
            .line_edit_target(window, cx)
            .filter(|editor| editor.read(cx).is_editable());
        let (Some(editor), Some(line)) = (editor, line) else {
            window.dispatch_action(Box::new(Paste), cx);
            return;
        };
        editor.update(cx, |state, cx| {
            let text = state.text().clone();
            let cursor = state.cursor();
            let start = text.line_start_offset(text.offset_to_point(cursor).row);
            state.set_selected_range(start..start, cx);
            state.insert(line.clone(), window, cx);
            let target = cursor + line.len();
            state.set_selected_range(target..target, cx);
        });
    }

    pub(super) fn toggle_comment(&mut self, _: &ToggleComment, window: &mut Window, cx: &mut Context<Self>) {
        self.transact_in_active_editor(window, cx, |state, language, window, cx| {
            let Some(prefix) = comment_prefix(&language) else { return };
            let text = state.text().clone();
            let block = line_block(&text, &state.selected_range());
            let original = text.slice(block.clone()).to_string();
            let replacement = toggle_comment_text(&original, prefix);
            state.set_selected_range(block.clone(), cx);
            state.replace(replacement.clone(), window, cx);
            state.set_selected_range(block.start..block.start + replacement.len(), cx);
        });
    }

    pub(super) fn duplicate_line(&mut self, _: &DuplicateLine, window: &mut Window, cx: &mut Context<Self>) {
        self.transact_in_active_editor(window, cx, |state, _, window, cx| {
            let text = state.text().clone();
            let selection = state.selected_range();
            if selection.is_empty() {
                let block = line_block(&text, &selection);
                let line = text.slice(block.clone()).to_string();
                let column = selection.start - block.start;
                state.set_selected_range(block.clone(), cx);
                state.replace(format!("{line}\n{line}"), window, cx);
                let new_cursor = block.end + 1 + column;
                state.set_selected_range(new_cursor..new_cursor, cx);
            } else {
                let selected = text.slice(selection.clone()).to_string();
                state.set_selected_range(selection.end..selection.end, cx);
                state.insert(selected.clone(), window, cx);
                state.set_selected_range(selection.end..selection.end + selected.len(), cx);
            }
        });
    }

    fn move_lines(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.transact_in_active_editor(window, cx, |state, _, window, cx| {
            let text = state.text().clone();
            let selection = state.selected_range();
            let block = line_block(&text, &selection);
            let first_row = text.offset_to_point(block.start).row;
            let last_row = text.offset_to_point(block.end).row;
            let lines = text.lines_len();
            if (up && first_row == 0) || (!up && last_row + 1 >= lines) {
                return;
            }
            let block_text = text.slice(block.clone()).to_string();
            if up {
                let prev_start = text.line_start_offset(first_row - 1);
                let prev = text.slice(prev_start..block.start - 1).to_string();
                state.set_selected_range(prev_start..block.end, cx);
                state.replace(format!("{block_text}\n{prev}"), window, cx);
                let shift = prev.len() + 1;
                state.set_selected_range(selection.start - shift..selection.end - shift, cx);
            } else {
                let next_end = text.line_end_offset(last_row + 1);
                let next = text.slice(block.end + 1..next_end).to_string();
                state.set_selected_range(block.start..next_end, cx);
                state.replace(format!("{next}\n{block_text}"), window, cx);
                let shift = next.len() + 1;
                state.set_selected_range(selection.start + shift..selection.end + shift, cx);
            }
        });
    }

    pub(super) fn move_line_up(&mut self, _: &MoveLineUp, window: &mut Window, cx: &mut Context<Self>) {
        self.move_lines(true, window, cx);
    }

    pub(super) fn move_line_down(&mut self, _: &MoveLineDown, window: &mut Window, cx: &mut Context<Self>) {
        self.move_lines(false, window, cx);
    }

    pub(super) fn select_next_occurrence(
        &mut self,
        _: &SelectNextOccurrence,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_active_editor(window, cx, |state, _, _, cx| {
            let text = state.text().clone();
            let selection = state.selected_range();
            if selection.is_empty() {
                if let Some(word) = text.word_range(selection.start) {
                    state.set_selected_range(word, cx);
                }
                return;
            }
            let needle = text.slice(selection.clone()).to_string();
            let haystack = text.to_string();
            let found = haystack[selection.end..]
                .find(&needle)
                .map(|i| i + selection.end)
                .or_else(|| haystack[..selection.start].find(&needle));
            if let Some(start) = found {
                state.add_selection(start..start + needle.len(), cx);
            }
        });
    }

    fn select_placeholder(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut found = false;
        self.with_active_editor(window, cx, |state, _, _, cx| {
            let text = state.text().to_string();
            let selection = state.selected_range();
            let range = if forward {
                lsp::next_placeholder(&text, selection.end)
            } else {
                lsp::previous_placeholder(&text, selection.start)
            };
            if let Some(range) = range {
                state.set_selected_range(range, cx);
                found = true;
            }
        });
        found
    }

    pub(super) fn next_placeholder(&mut self, _: &NextPlaceholder, window: &mut Window, cx: &mut Context<Self>) {
        self.select_placeholder(true, window, cx);
    }

    pub(super) fn previous_placeholder(
        &mut self,
        _: &PreviousPlaceholder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_placeholder(false, window, cx);
    }

    /// Tab jumps to the next `<#placeholder#>` on the current line (as in
    /// Xcode), otherwise indents.
    pub(super) fn tab_or_placeholder(
        &mut self,
        _: &TabOrPlaceholder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.active_editor() else {
            window.dispatch_action(Box::new(IndentInline), cx);
            return;
        };
        let (menu_open, line_has_placeholder) = {
            let state = editor.read(cx);
            let text = state.text();
            let row = text.offset_to_point(state.cursor()).row;
            let line = text.slice_line(row).to_string();
            (state.completion_menu_state().open, line.contains("<#") && line.contains("#>"))
        };
        if !menu_open && line_has_placeholder && self.select_placeholder(true, window, cx) {
            return;
        }
        window.dispatch_action(Box::new(IndentInline), cx);
    }

    pub(super) fn change_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.font_size = if delta == 0. {
            super::DEFAULT_FONT_SIZE
        } else {
            (self.font_size + delta).clamp(8., 36.)
        };
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn toggle_soft_wrap(&mut self, _: &ToggleSoftWrap, window: &mut Window, cx: &mut Context<Self>) {
        self.soft_wrap = !self.soft_wrap;
        let wrap = self.soft_wrap;
        for pane in &self.panes {
            for item in pane.items.iter().filter_map(super::Item::editor) {
                item.editor.update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx));
            }
        }
        cx.notify();
    }

    pub(super) fn jump_to_definition(&mut self, _: &JumpToDefinition, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else { return };
        editor.update(cx, |state, cx| state.focus(window, cx));
        window.dispatch_action(Box::new(GoToDefinition), cx);
    }

    pub(super) fn fix_issue(&mut self, _: &FixIssue, window: &mut Window, cx: &mut Context<Self>) {
        window.dispatch_action(Box::new(ToggleCodeActions), cx);
    }

    pub(super) fn jump_to_counterpart(&mut self, _: &JumpToCounterpart, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active_item().map(|b| b.path.clone()) else { return };
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let candidates: &[&str] = match ext {
            "h" | "hpp" | "hh" => &["m", "mm", "c", "cpp", "cc"],
            "m" | "mm" | "c" | "cpp" | "cc" => &["h", "hpp", "hh"],
            _ => &[],
        };
        for candidate in candidates {
            let other = path.with_extension(candidate);
            if other.exists() {
                self.open_file(other, None, true, window, cx);
                return;
            }
        }
    }

    // ------------------------------------------------------------------
    // LSP-backed commands

    /// The server plus the active document's URI, text and cursor.
    fn lsp_context(&mut self, cx: &mut Context<Self>) -> Option<(Arc<lsp::LanguageServer>, PathBuf, Rope, Range<usize>)> {
        let buffer = self.active_item()?;
        let path = buffer.path.clone();
        let (text, selection) = {
            let state = buffer.editor.read(cx);
            (state.text().clone(), state.selected_range())
        };
        let server = self
            .lsp
            .update(cx, |lsp, cx| lsp.synced_server(&path, cx))?;
        Some((server, path, text, selection))
    }

    pub(super) fn lsp_unavailable(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.notify_error(
            "The language server isn’t ready for this file yet.".into(),
            window,
            cx,
        );
    }

    pub(super) fn find_selected_symbol(
        &mut self,
        _: &FindSelectedSymbol,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((server, path, text, selection)) = self.lsp_context(cx) else {
            self.lsp_unavailable(window, cx);
            return;
        };
        let word = text.word_at(selection.start);
        let request = server.request_raw(
            "textDocument/references",
            json!({
                "textDocument": { "uri": path_to_uri(&path).as_str() },
                "position": offset_to_lsp(&text, selection.start),
                "context": { "includeDeclaration": true },
            }),
        );
        self.navigator = NavigatorTab::Find;
        self.show_navigator = true;
        self.find.searching = true;
        self.find.title = Some(format!("References to “{word}”"));
        cx.notify();
        self.find._task = Some(cx.spawn(async move |this, cx| {
            let locations: Vec<lsp_types::Location> = request
                .await
                .ok()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let results = cx
                .background_spawn(async move { locations_to_matches(locations) })
                .await;
            this.update(cx, |this, cx| {
                this.find.results = Arc::new(results);
                this.find.searching = false;
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn edit_all_in_scope(&mut self, _: &EditAllInScope, window: &mut Window, cx: &mut Context<Self>) {
        let Some((server, path, text, selection)) = self.lsp_context(cx) else {
            self.lsp_unavailable(window, cx);
            return;
        };
        let request = server.request_raw(
            "textDocument/documentHighlight",
            json!({
                "textDocument": { "uri": path_to_uri(&path).as_str() },
                "position": offset_to_lsp(&text, selection.start),
            }),
        );
        let Some(editor) = self.active_editor() else { return };
        cx.spawn(async move |_, cx| {
            let highlights: Vec<lsp_types::DocumentHighlight> = request
                .await
                .ok()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            editor
                .update(cx, |state, cx| {
                    let text = state.text().clone();
                    let mut ranges: Vec<_> = highlights
                        .iter()
                        .map(|h| lsp::lsp_range_to_offsets(&text, h.range))
                        .collect();
                    ranges.sort_by_key(|r| r.start);
                    let cursor = state.cursor();
                    // Keep the range under the cursor primary.
                    if let Some(pos) = ranges.iter().position(|r| r.contains(&cursor) || r.end == cursor) {
                        let primary = ranges.remove(pos);
                        state.set_selected_range(primary, cx);
                        for range in ranges {
                            state.add_selection(range, cx);
                        }
                    }
                });
        })
        .detach();
    }

    pub(super) fn format_file(&mut self, _: &FormatFile, window: &mut Window, cx: &mut Context<Self>) {
        self.format(false, window, cx);
    }

    pub(super) fn re_indent(&mut self, _: &ReIndent, window: &mut Window, cx: &mut Context<Self>) {
        self.format(true, window, cx);
    }

    fn format(&mut self, selection_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((server, path, text, selection)) = self.lsp_context(cx) else {
            self.lsp_unavailable(window, cx);
            return;
        };
        let options = json!({ "tabSize": 4, "insertSpaces": true });
        let uri = path_to_uri(&path);
        let request = if selection_only && server.supports("/documentRangeFormattingProvider") {
            let block = line_block(&text, &selection);
            server.request_raw(
                "textDocument/rangeFormatting",
                json!({
                    "textDocument": { "uri": uri.as_str() },
                    "range": { "start": offset_to_lsp(&text, block.start), "end": offset_to_lsp(&text, block.end) },
                    "options": options,
                }),
            )
        } else {
            server.request_raw(
                "textDocument/formatting",
                json!({ "textDocument": { "uri": uri.as_str() }, "options": options }),
            )
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(value) => {
                    let edits: Vec<lsp_types::TextEdit> =
                        serde_json::from_value(value).unwrap_or_default();
                    if !edits.is_empty() {
                        // `WorkspaceEdit::changes` is keyed by lsp_types' Uri.
                        #[allow(clippy::mutable_key_type)]
                        let mut changes = std::collections::HashMap::new();
                        changes.insert(uri, edits);
                        this.apply_workspace_edit(
                            lsp_types::WorkspaceEdit {
                                changes: Some(changes),
                                ..Default::default()
                            },
                            window,
                            cx,
                        );
                    }
                }
                Err(err) => this.notify_error(format!("Formatting failed: {err:#}"), window, cx),
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn rename(&mut self, _: &Rename, window: &mut Window, cx: &mut Context<Self>) {
        let Some((server, path, text, selection)) = self.lsp_context(cx) else {
            self.lsp_unavailable(window, cx);
            return;
        };
        let word = text.word_at(selection.start);
        if word.is_empty() {
            return;
        }
        let delegate = pickers::RenameDelegate::new(
            cx.weak_entity(),
            server,
            path,
            offset_to_lsp(&text, selection.start),
            word.clone(),
        );
        let picker = cx.new(|cx| crate::picker::Picker::with_query(delegate, &word, window, cx));
        self.show_modal(picker, window, cx);
    }

    pub(super) fn rename_symbol(
        &mut self,
        server: Arc<lsp::LanguageServer>,
        path: PathBuf,
        position: lsp_types::Position,
        new_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = server.request_raw(
            "textDocument/rename",
            json!({
                "textDocument": { "uri": path_to_uri(&path).as_str() },
                "position": position,
                "newName": new_name,
            }),
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(Value::Null) => this.notify_error("Nothing to rename here.".into(), window, cx),
                Ok(value) => match serde_json::from_value::<lsp_types::WorkspaceEdit>(value) {
                    Ok(edit) => this.apply_workspace_edit(edit, window, cx),
                    Err(err) => this.notify_error(format!("Rename failed: {err}"), window, cx),
                },
                Err(err) => this.notify_error(format!("Rename failed: {err:#}"), window, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Re-requests the document outline for the Symbol navigator and ⌃6.
    pub(super) fn refresh_outline(&mut self, cx: &mut Context<Self>) {
        let Some((server, path, _, _)) = self.lsp_context(cx) else {
            self.outline.clear();
            self.outline_path = None;
            return;
        };
        let request = server.request_raw(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": path_to_uri(&path).as_str() } }),
        );
        self.outline_task = Some(cx.spawn(async move |this, cx| {
            let value = request.await.unwrap_or(Value::Null);
            let entries = pickers::flatten_symbols(value);
            this.update(cx, |this, cx| {
                this.outline = entries;
                this.outline_path = Some(path);
                this.outline_generation += 1;
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn reveal_outline_entry(&mut self, entry: &OutlineEntry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.outline_path.clone() else { return };
        self.open_file(path, Some(Target::LspRange(entry.selection_range)), true, window, cx);
    }
}

fn locations_to_matches(locations: Vec<lsp_types::Location>) -> Vec<FileMatches> {
    let mut by_file: std::collections::BTreeMap<PathBuf, Vec<lsp_types::Range>> = Default::default();
    for location in locations {
        if let Some(path) = lsp::uri_to_path(&location.uri) {
            by_file.entry(path).or_default().push(location.range);
        }
    }
    by_file
        .into_iter()
        .map(|(path, mut ranges)| {
            ranges.sort_by_key(|r| (r.start.line, r.start.character));
            let contents = std::fs::read_to_string(&path).unwrap_or_default();
            let lines: Vec<&str> = contents.lines().collect();
            let matches = ranges
                .into_iter()
                .map(|range| {
                    let line = lines.get(range.start.line as usize).copied().unwrap_or("");
                    let rope = Rope::from(line);
                    let start = lsp::lsp_to_offset(&rope, lsp_types::Position::new(0, range.start.character));
                    let end = if range.end.line == range.start.line {
                        lsp::lsp_to_offset(&rope, lsp_types::Position::new(0, range.end.character))
                    } else {
                        line.len()
                    };
                    let trim = line.len() - line.trim_start().len();
                    let trim = trim.min(start);
                    LineMatch {
                        line: range.start.line,
                        column: range.start.character,
                        range: start - trim..end - trim,
                        preview: line[trim..].to_string(),
                    }
                })
                .collect();
            FileMatches { path, matches }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{line_block, toggle_comment_text, whole_line};
    use zcode_editor::Rope;

    #[test]
    fn comments_and_uncomments() {
        let src = "    let a = 1\n\n    let b = 2";
        let commented = toggle_comment_text(src, "//");
        assert_eq!(commented, "    // let a = 1\n\n    // let b = 2");
        assert_eq!(toggle_comment_text(&commented, "//"), src);
    }

    #[test]
    fn line_blocks() {
        let text = Rope::from("one\ntwo\nthree\n");
        assert_eq!(line_block(&text, &(5..5)), 4..7);
        // Selection ending at column 0 of the next line excludes that line.
        assert_eq!(line_block(&text, &(0..8)), 0..7);
    }

    #[test]
    fn whole_lines() {
        let text = Rope::from("one\ntwo\nthree");
        assert_eq!(whole_line(&text, 5), 4..8);
        // The last line has no newline to include.
        assert_eq!(whole_line(&text, 9), 8..13);
    }
}

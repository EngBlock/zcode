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

/// What ⌥↑ (`up`) or ⌥↓ moves: the lines `rows`, and the line above or
/// below that they swap with, in a text of `lines` lines. Both widen to
/// whole folded blocks (a fold's first line through its last), so a fold
/// moves as one line and is never split. `None` at the top or the bottom.
fn line_move(
    folds: &[zcode_editor::FoldRange],
    rows: std::ops::RangeInclusive<usize>,
    lines: usize,
    up: bool,
) -> Option<(std::ops::RangeInclusive<usize>, std::ops::RangeInclusive<usize>)> {
    let widen = |rows: std::ops::RangeInclusive<usize>| {
        let (mut first, mut last) = rows.into_inner();
        while let Some(fold) = folds
            .iter()
            .find(|f| f.start_line <= last && f.end_line >= first && (f.start_line < first || f.end_line > last))
        {
            first = first.min(fold.start_line);
            last = last.max(fold.end_line);
        }
        first..=last
    };
    let moved = widen(rows);
    let past = if up {
        widen(moved.start().checked_sub(1)?..=moved.start() - 1)
    } else {
        let next = moved.end() + 1;
        if next >= lines {
            return None;
        }
        widen(next..=next)
    };
    Some((moved, past))
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

use crate::languages::emmet::MarkupBlock;

/// How a language comments a line out: a prefix, or a pair around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentStyle {
    Line(&'static str),
    Block(&'static str, &'static str),
}

/// `language`'s comments at `offset` of `text`: in markup, a `<script>`
/// block's are JavaScript's and a `<style>` block's CSS's.
fn comment_style(language: &str, text: &str, offset: usize) -> Option<CommentStyle> {
    Some(match language {
        "swift" | "c" | "cpp" | "rust" | "javascript" | "jsx" | "typescript" | "tsx" | "go" | "java" | "kotlin" | "json" => {
            CommentStyle::Line("//")
        }
        "yaml" | "toml" | "bash" | "python" | "ruby" | "make" => CommentStyle::Line("#"),
        "css" => CommentStyle::Block("/*", "*/"),
        // SCSS and Less have line comments.
        "less" => CommentStyle::Line("//"),
        "html" | "vue" | "svelte" | "astro" | "markdown" => match crate::languages::emmet::markup_block_at(text, offset) {
            Some(MarkupBlock::Script) => CommentStyle::Line("//"),
            Some(MarkupBlock::Style) => CommentStyle::Block("/*", "*/"),
            None => CommentStyle::Block("<!--", "-->"),
        },
        _ => return None,
    })
}

/// Comments or uncomments every line of `block` (Xcode's ⌘/): a prefix after
/// the block's indentation, or each line wrapped in a block comment. Blank
/// lines are left alone, unless every line is blank: then each gets an
/// empty comment to type in.
pub fn toggle_comment_text(block: &str, style: CommentStyle) -> String {
    let lines: Vec<&str> = block.split('\n').collect();
    let non_blank: Vec<&&str> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    if non_blank.is_empty() {
        return lines
            .iter()
            .map(|line| match style {
                CommentStyle::Line(prefix) => format!("{line}{prefix} "),
                CommentStyle::Block(open, close) => format!("{line}{open}  {close}"),
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    let commented = |line: &str| match style {
        CommentStyle::Line(prefix) => line.trim_start().starts_with(prefix),
        CommentStyle::Block(open, close) => line.trim_start().starts_with(open) && line.trim_end().ends_with(close),
    };
    let all_commented = !non_blank.is_empty() && non_blank.iter().all(|l| commented(l));
    let indent_of = |line: &str| line.len() - line.trim_start().len();
    if all_commented {
        lines
            .iter()
            .map(|line| {
                let indent = indent_of(line);
                let rest = &line[indent..];
                let uncommented = match style {
                    CommentStyle::Line(prefix) => rest.strip_prefix(prefix).map(|after| after.strip_prefix(' ').unwrap_or(after).to_string()),
                    CommentStyle::Block(open, close) => rest.trim_end().strip_prefix(open).and_then(|r| r.strip_suffix(close)).map(|inner| {
                        let inner = inner.strip_prefix(' ').unwrap_or(inner);
                        inner.strip_suffix(' ').unwrap_or(inner).to_string()
                    }),
                };
                match uncommented {
                    Some(after) => format!("{}{}", &line[..indent], after),
                    None => line.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        let min_indent = non_blank.iter().map(|l| indent_of(l)).min().unwrap_or(0);
        lines
            .iter()
            .map(|line| {
                if line.trim().is_empty() {
                    return line.to_string();
                }
                match style {
                    CommentStyle::Line(prefix) => format!("{}{} {}", &line[..min_indent], prefix, &line[min_indent..]),
                    CommentStyle::Block(open, close) => format!("{}{open} {} {close}", &line[..min_indent], line[min_indent..].trim_end()),
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
            let text = state.text().clone();
            let block = line_block(&text, &state.selected_range());
            let Some(style) = comment_style(&language, &text.to_string(), block.start) else { return };
            let selection = state.selected_range();
            let original = text.slice(block.clone()).to_string();
            let replacement = toggle_comment_text(&original, style);
            state.set_selected_range(block.clone(), cx);
            state.replace(replacement.clone(), window, cx);
            if selection.is_empty() && original.trim().is_empty() {
                // An empty comment on a blank line: the cursor goes inside it.
                let inside = match style {
                    CommentStyle::Line(_) => replacement.len(),
                    CommentStyle::Block(_, close) => replacement.len() - close.len() - 1,
                };
                state.set_selected_range(block.start + inside..block.start + inside, cx);
            } else {
                // Whole lines selected (to the start of the line after them)
                // stay selected with their newline, so ⌘D or ⌥↓ next still
                // takes whole lines.
                let whole_lines = !selection.is_empty() && selection.end == block.end + 1;
                let end = block.start + replacement.len() + usize::from(whole_lines);
                state.set_selected_range(block.start..end, cx);
            }
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

    /// ⌥↑ / ⌥↓: the selected lines swap with the visible line above or
    /// below. A folded block (its first line through its last) moves as one
    /// line and stays folded, and so does one it moves past.
    fn move_lines(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.transact_in_active_editor(window, cx, |state, _, window, cx| {
            let text = state.text().clone();
            let selection = state.selected_range();
            let block = line_block(&text, &selection);
            let folds = state.folded_lines();
            let rows = text.offset_to_point(block.start).row..=text.offset_to_point(block.end).row;
            let Some((moved, past)) = line_move(&folds, rows, text.lines_len(), up) else { return };
            let moved_range = text.line_start_offset(*moved.start())..text.line_end_offset(*moved.end());
            let past_range = text.line_start_offset(*past.start())..text.line_end_offset(*past.end());
            let moved_text = text.slice(moved_range.clone()).to_string();
            let past_text = text.slice(past_range.clone()).to_string();
            // The folds inside `rows`, `delta` lines on.
            let refold = |rows: &std::ops::RangeInclusive<usize>, delta: isize| -> Vec<(usize, usize)> {
                folds
                    .iter()
                    .filter(|f| rows.contains(&f.start_line) && rows.contains(&f.end_line))
                    .map(|f| (f.start_line.saturating_add_signed(delta), f.end_line.saturating_add_signed(delta)))
                    .collect()
            };
            let (moved_lines, past_lines) = (moved.clone().count() as isize, past.clone().count() as isize);
            let (shift, folds_after): (isize, Vec<(usize, usize)>) = if up {
                state.set_selected_range(past_range.start..moved_range.end, cx);
                state.replace(format!("{moved_text}\n{past_text}"), window, cx);
                let folds = [refold(&moved, -past_lines), refold(&past, moved_lines)].concat();
                (-(past_text.len() as isize + 1), folds)
            } else {
                state.set_selected_range(moved_range.start..past_range.end, cx);
                state.replace(format!("{past_text}\n{moved_text}"), window, cx);
                let folds = [refold(&moved, past_lines), refold(&past, -moved_lines)].concat();
                (past_text.len() as isize + 1, folds)
            };
            for (start, end) in folds_after {
                state.fold_lines(zcode_editor::FoldRange::new(start, end), cx);
            }
            let start = selection.start.saturating_add_signed(shift);
            let end = selection.end.saturating_add_signed(shift);
            state.set_selected_range(start..end, cx);
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

    /// Tab moves to a snippet's next tab stop (after a completion or an
    /// Emmet abbreviation), else to the next `<#placeholder#>` on the
    /// current line (as in Xcode), otherwise indents.
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
        if !menu_open && editor.update(cx, |state, cx| state.next_snippet_stop(cx)) {
            return;
        }
        if !menu_open && line_has_placeholder && self.select_placeholder(true, window, cx) {
            return;
        }
        window.dispatch_action(Box::new(IndentInline), cx);
    }

    /// Editor ▸ Font Size: `buffer_font_size` in the settings.
    pub(super) fn change_font_size(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        let size = if delta == 0. {
            crate::settings::DEFAULT_FONT_SIZE
        } else {
            (self.latest_settings(cx).buffer_font_size + delta).clamp(8., 36.)
        };
        let size = if size.fract() == 0. { serde_json::json!(size as i64) } else { serde_json::json!(size) };
        self.set_setting(&["buffer_font_size"], size, window, cx);
    }

    /// Editor ▸ Wrap Lines: `soft_wrap` in the settings.
    pub(super) fn toggle_soft_wrap(&mut self, _: &ToggleSoftWrap, window: &mut Window, cx: &mut Context<Self>) {
        let wrap = if self.latest_settings(cx).soft_wrap { "none" } else { "editor_width" };
        self.set_setting(&["soft_wrap"], serde_json::json!(wrap), window, cx);
    }

    /// Editor ▸ Invisibles: `show_whitespaces` in the settings.
    pub(super) fn toggle_invisibles(&mut self, _: &ToggleInvisibles, window: &mut Window, cx: &mut Context<Self>) {
        let show = if self.latest_settings(cx).show_whitespaces { "none" } else { "all" };
        self.set_setting(&["show_whitespaces"], serde_json::json!(show), window, cx);
    }

    pub(super) fn apply_soft_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// The active document's first server offering `capability` (a JSON
    /// pointer into its capabilities), plus the document's path, text and
    /// selection.
    fn lsp_context(&mut self, capability: &str, cx: &mut Context<Self>) -> Option<(Arc<lsp::LanguageServer>, PathBuf, Rope, Range<usize>)> {
        let buffer = self.active_item()?;
        let path = buffer.path.clone();
        let (text, selection) = {
            let state = buffer.editor.read(cx);
            (state.text().clone(), state.selected_range())
        };
        let server = self.lsp.update(cx, |lsp, cx| lsp.synced_server_supporting(&path, Some(&text), Some(capability), cx))?;
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
        let Some((server, path, text, selection)) = self.lsp_context("/referencesProvider", cx) else {
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
                this.find.set_results(results);
                this.find.searching = false;
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn edit_all_in_scope(&mut self, _: &EditAllInScope, window: &mut Window, cx: &mut Context<Self>) {
        let Some((server, path, text, selection)) = self.lsp_context("/documentHighlightProvider", cx) else {
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

    /// Editor ▸ Emmet ▸ Expand Abbreviation (⌃⌥E): the abbreviation before
    /// the cursor, expanded; Tab visits its tab stops.
    pub(super) fn expand_abbreviation(&mut self, _: &ExpandAbbreviation, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else { return };
        let language = self.active_buffer().map(|b| b.language.clone()).unwrap_or_default();
        let tab = self.active_tab(cx);
        let options = crate::languages::emmet::options(tab.tab_size as u32, tab.hard_tabs);
        let (text, cursor) = {
            let state = editor.read(cx);
            (state.text().clone(), state.cursor())
        };
        let Some((range, snippet, _)) = crate::languages::emmet::expansion_at(&language, &text, cursor, &options, false) else {
            self.notify_error("No Emmet abbreviation before the cursor.".into(), window, cx);
            return;
        };
        self.transact_in_active_editor(window, cx, |state, _, window, cx| state.insert_snippet(range, &snippet, window, cx));
    }

    /// Editor ▸ Emmet ▸ Wrap with Abbreviation…: the selection (or the
    /// cursor's line) inside an abbreviation typed in a sheet, which
    /// previews it.
    pub(super) fn wrap_with_abbreviation(&mut self, _: &WrapWithAbbreviation, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else { return };
        let language = self.active_buffer().map(|b| b.language.clone()).unwrap_or_default();
        let (text, selection) = {
            let state = editor.read(cx);
            (state.text().clone(), state.selected_range())
        };
        let Some(syntax) = crate::languages::emmet::syntax_at(&language, &text, selection.start).filter(|s| *s != zcode_emmet::Syntax::Css) else {
            self.notify_error("Wrap with Abbreviation works in HTML and JSX.".into(), window, cx);
            return;
        };
        // Nothing selected: the cursor's line, without its indentation.
        let range = if selection.is_empty() {
            let line = whole_line(&text, selection.start);
            let content = text.slice(line.clone()).to_string();
            let start = line.start + (content.len() - content.trim_start().len());
            start..line.start + content.trim_end().len()
        } else {
            selection
        };
        let line_start = text.line_start_offset(text.offset_to_point(range.start).row);
        let indent: String = text.slice(line_start..range.start).to_string().chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let tab = self.active_tab(cx);
        let options = crate::languages::emmet::options(tab.tab_size as u32, tab.hard_tabs);
        let delegate = pickers::WrapDelegate::new(cx.weak_entity(), editor, range, text, syntax, options, indent);
        let picker = cx.new(|cx| crate::picker::Picker::new(delegate, window, cx));
        self.show_modal(picker, window, cx);
    }

    pub(super) fn format_file(&mut self, _: &FormatFile, window: &mut Window, cx: &mut Context<Self>) {
        self.format(false, window, cx);
    }

    pub(super) fn re_indent(&mut self, _: &ReIndent, window: &mut Window, cx: &mut Context<Self>) {
        self.format(true, window, cx);
    }

    /// Format File (the formatter the settings choose) or Re-Indent (the
    /// selected lines, by the language server).
    fn format(&mut self, selection_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ranged = if selection_only { self.lsp_context("/documentRangeFormattingProvider", cx) } else { None };
        let request = match ranged {
            Some((server, path, text, selection)) => {
                let block = line_block(&text, &selection);
                let request = server.request_raw(
                    "textDocument/rangeFormatting",
                    json!({
                        "textDocument": { "uri": path_to_uri(&path).as_str() },
                        "range": { "start": offset_to_lsp(&text, block.start), "end": offset_to_lsp(&text, block.end) },
                        "options": self.formatting_options(&path, cx),
                    }),
                );
                let task = cx.background_spawn(async move { Ok(serde_json::from_value(request.await?).unwrap_or_default()) });
                Some((path, task))
            }
            None => self.active_item().map(|item| item.path.clone()).and_then(|path| Some((path.clone(), self.document_formatting(&path, cx)?))),
        };
        let Some((path, request)) = request else {
            self.lsp_unavailable(window, cx);
            return;
        };
        let uri = path_to_uri(&path);
        cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(edits) => {
                    if !edits.is_empty() {
                        // `WorkspaceEdit::changes` is keyed by lsp_types' Uri.
                        #[allow(clippy::mutable_key_type)]
                        let changes = std::collections::HashMap::from([(uri, edits)]);
                        this.apply_workspace_edit(lsp_types::WorkspaceEdit { changes: Some(changes), ..Default::default() }, window, cx);
                    }
                }
                Err(err) => this.notify_error(format!("Formatting failed: {err:#}"), window, cx),
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn rename(&mut self, _: &Rename, window: &mut Window, cx: &mut Context<Self>) {
        let Some((server, path, text, selection)) = self.lsp_context("/renameProvider", cx) else {
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
        original: String,
        new_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = server.request_raw(
            "textDocument/rename",
            json!({
                "textDocument": { "uri": path_to_uri(&path).as_str() },
                "position": position,
                "newName": new_name.clone(),
            }),
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(Value::Null) => this.notify_error("Nothing to rename here.".into(), window, cx),
                // Reviewed in a multibuffer of the places it changes.
                Ok(value) => match serde_json::from_value::<lsp_types::WorkspaceEdit>(value) {
                    Ok(edit) => this.apply_rename(edit, original, new_name, window, cx),
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
        let Some((server, path, _, _)) = self.lsp_context("/documentSymbolProvider", cx) else {
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
    use super::{CommentStyle, comment_style, line_block, toggle_comment_text, whole_line};

    #[test]
    fn markup_and_css_comment_in_blocks() {
        let html = CommentStyle::Block("<!--", "-->");
        let commented = toggle_comment_text("  <div>\n    <p>hi</p>", html);
        assert_eq!(commented, "  <!-- <div> -->\n  <!--   <p>hi</p> -->");
        assert_eq!(toggle_comment_text(&commented, html), "  <div>\n    <p>hi</p>");
        assert_eq!(toggle_comment_text("a { color: red; }", CommentStyle::Block("/*", "*/")), "/* a { color: red; } */");
    }

    #[test]
    fn a_components_blocks_comment_in_their_own_language() {
        let vue = "<template>\n  <p>{{ n }}</p>\n</template>\n<script setup lang=\"ts\">\nconst n = 1\n</script>\n<style>\np { color: red }\n</style>\n";
        let at = |needle: &str| vue.find(needle).unwrap();
        assert_eq!(comment_style("vue", vue, at("<p>")), Some(CommentStyle::Block("<!--", "-->")));
        assert_eq!(comment_style("vue", vue, at("const")), Some(CommentStyle::Line("//")));
        assert_eq!(comment_style("vue", vue, at("p {")), Some(CommentStyle::Block("/*", "*/")));
        assert_eq!(comment_style("tsx", "", 0), Some(CommentStyle::Line("//")));
        assert_eq!(comment_style("text", "", 0), None);
    }
    use zcode_editor::Rope;

    #[test]
    fn blank_lines_get_an_empty_comment() {
        let line = CommentStyle::Line("//");
        assert_eq!(toggle_comment_text("", line), "// ");
        assert_eq!(toggle_comment_text("  \n", line), "  // \n// ");
        assert_eq!(toggle_comment_text("// ", line), "");
        assert_eq!(toggle_comment_text("", CommentStyle::Block("/*", "*/")), "/*  */");
        // Among other lines, blank ones stay blank.
        assert_eq!(toggle_comment_text("a\n\nb", line), "// a\n\n// b");
    }

    #[test]
    fn a_folded_block_moves_as_one_line() {
        use zcode_editor::FoldRange;
        // Lines 1–3 folded (line 2 hidden).
        let folds = [FoldRange::new(1, 3)];
        // Down from the header: the whole block passes line 4.
        assert_eq!(super::line_move(&folds, 1..=1, 6, false), Some((1..=3, 4..=4)));
        // Up from the header: past line 0.
        assert_eq!(super::line_move(&folds, 1..=1, 6, true), Some((1..=3, 0..=0)));
        // Lines around it pass the block as a whole, from above and below.
        assert_eq!(super::line_move(&folds, 0..=0, 6, false), Some((0..=0, 1..=3)));
        assert_eq!(super::line_move(&folds, 4..=4, 6, true), Some((4..=4, 1..=3)));
        // The closing line takes its block with it.
        assert_eq!(super::line_move(&folds, 3..=3, 6, false), Some((1..=3, 4..=4)));
        // Nothing to move past at either end.
        assert_eq!(super::line_move(&folds, 4..=5, 6, false), None);
        assert_eq!(super::line_move(&[FoldRange::new(0, 2)], 1..=1, 6, true), None);
        // Without folds, one line at a time.
        assert_eq!(super::line_move(&[], 2..=2, 6, false), Some((2..=2, 3..=3)));
    }

    #[test]
    fn comments_and_uncomments() {
        let src = "    let a = 1\n\n    let b = 2";
        let commented = toggle_comment_text(src, CommentStyle::Line("//"));
        assert_eq!(commented, "    // let a = 1\n\n    // let b = 2");
        assert_eq!(toggle_comment_text(&commented, CommentStyle::Line("//")), src);
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

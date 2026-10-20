//! Find in Workspace (⇧⌘F): the Find navigator's query and results, and
//! Find and Replace in Workspace (⌥⇧⌘F), whose Replace All edits the
//! results in a multibuffer (`multibuffer.rs`).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui_kit::component::input::InputState;
use gpui_kit::*;

use super::Workspace;
use crate::search::{FileMatches, SearchOptions};

pub struct FindState {
    pub input: Entity<InputState>,
    /// Find and Replace: the replacement, shown with Replace All (made
    /// the first time, not at launch).
    pub replace_input: Option<Entity<InputState>>,
    pub replacing: bool,
    pub options: SearchOptions,
    pub results: Arc<Vec<FileMatches>>,
    /// The navigator's rows for `results`, a file's header then its
    /// matches, laid out only where on screen: a search over a big project
    /// has thousands.
    pub rows: Vec<FindRow>,
    pub scroll: UniformListScrollHandle,
    pub searching: bool,
    pub error: Option<String>,
    pub title: Option<String>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) _task: Option<Task<()>>,
}

/// A row of the Find navigator: a file's header, or one of its matches
/// (at most `MATCHES_SHOWN_PER_FILE` of them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindRow {
    File(usize),
    Match { file: usize, index: usize },
}

/// How many of a file's matches the navigator lists (all of them open in
/// the multibuffer).
pub const MATCHES_SHOWN_PER_FILE: usize = 100;

impl FindState {
    pub fn set_results(&mut self, results: Vec<FileMatches>) {
        self.rows = find_rows(&results);
        self.results = Arc::new(results);
    }
}

/// The rows for `results`, in order.
pub fn find_rows(results: &[FileMatches]) -> Vec<FindRow> {
    let mut rows = Vec::with_capacity(results.iter().map(|f| 1 + f.matches.len().min(MATCHES_SHOWN_PER_FILE)).sum());
    for (file, found) in results.iter().enumerate() {
        rows.push(FindRow::File(file));
        rows.extend((0..found.matches.len().min(MATCHES_SHOWN_PER_FILE)).map(|index| FindRow::Match { file, index }));
    }
    rows
}

/// The characters a result's row shows before its match, at most.
const PREVIEW_LEAD: usize = 10;

/// A result's line as its row shows it, and the match in it: the start
/// elided, so the match is in a row a few dozen characters wide.
pub fn shown_preview(m: &crate::search::LineMatch) -> (String, std::ops::Range<usize>) {
    crate::search::elide_start(&m.preview, m.range.clone(), PREVIEW_LEAD)
}

impl Workspace {
    /// Find ▸ Find and Replace in Workspace (⌥⇧⌘F): the Find navigator
    /// with a Replace field.
    pub(super) fn show_find_and_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.replace_input.is_none() {
            self.find.replace_input = Some(cx.new(|cx| InputState::new(window, cx).placeholder("Replace With")));
        }
        self.find.replacing = true;
        self.show_navigator_tab(super::NavigatorTab::Find, window, cx);
    }

    pub fn run_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.find.input.read(cx).value().to_string();
        let _ = window;
        self.find.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        if query.is_empty() {
            self.find.set_results(vec![]);
            self.find.searching = false;
            cx.notify();
            return;
        }
        let regex = match crate::search::build_regex(&query, &self.find.options) {
            Ok(regex) => regex,
            Err(err) => {
                // The last query's results aren't this one's.
                self.find.set_results(vec![]);
                self.find.searching = false;
                self.find.title = None;
                self.find.error = Some(err.to_string());
                cx.notify();
                return;
            }
        };
        self.find.error = None;
        self.find.title = None;
        self.find.searching = true;
        let cancel = Arc::new(AtomicBool::new(false));
        self.find.cancel = cancel.clone();
        let root = self.project.root.clone();
        let files = self.files.clone();
        let unsaved = self.unsaved_texts(cx);
        let started = std::time::Instant::now();
        self.find._task = Some(cx.spawn(async move |this, cx| {
            let results = cx
                .background_spawn(async move { crate::search::search(&root, &files, &unsaved, &regex, &cancel) })
                .await;
            this.update(cx, |this, cx| {
                let matches = results.iter().map(|f| f.matches.len()).sum();
                crate::perf::Perf::record_timing(cx, "find", started.elapsed(), matches);
                this.find.set_results(results);
                this.find.searching = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The control channel's Find navigator: `query <text>` searches for
    /// it; `regex|case|word on|off` sets an option (the `.*`, `Aa` and `W`
    /// buttons) and searches again.
    pub fn find_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
        let options = &mut self.find.options;
        let option = match verb {
            "query" => {
                self.show_navigator_tab(super::NavigatorTab::Find, window, cx);
                let query = arg.to_string();
                self.find.input.update(cx, |input, cx| input.set_value(query, window, cx));
                self.run_find(window, cx);
                return Ok(());
            }
            "regex" => &mut options.regex,
            "case" => &mut options.case_sensitive,
            "word" => &mut options.whole_word,
            other => return Err(format!("unknown find command {other}")),
        };
        *option = arg == "on";
        self.run_find(window, cx);
        Ok(())
    }

    /// The line under the Find navigator's query: searching, the pattern's
    /// error, or how many results.
    pub(super) fn find_summary_line(&self, cx: &App) -> String {
        let results = &self.find.results;
        let total: usize = results.iter().map(|f| f.matches.len()).sum();
        if self.find.searching {
            "Searching…".to_string()
        } else if let Some(err) = &self.find.error {
            format!("Invalid pattern: {err}")
        } else if results.is_empty() && self.find.input.read(cx).value().is_empty() && self.find.title.is_none() {
            String::new()
        } else {
            let capped = if total >= crate::search::MAX_MATCHES { "+" } else { "" };
            format!(
                "{}{total}{capped} {} in {}",
                self.find.title.as_ref().map(|t| format!("{t}: ")).unwrap_or_default(),
                if total == 1 && capped.is_empty() { "result" } else { "results" },
                super::multibuffer::plural(results.len(), "file")
            )
        }
    }

    /// The control channel's view of the Find navigator: its summary line
    /// and the first results' rows as shown (1-based lines).
    pub(super) fn find_summary(&self, cx: &App) -> serde_json::Value {
        let rows: Vec<serde_json::Value> = self
            .find
            .results
            .iter()
            .flat_map(|file| file.matches.iter().map(move |m| (file, m)))
            .take(50)
            .map(|(file, m)| {
                let (shown, range) = shown_preview(m);
                serde_json::json!({
                    "file": file.path.file_name().map(|n| n.to_string_lossy().into_owned()),
                    "line": m.line + 1,
                    // Where the match starts in the row, in characters.
                    "match_at": shown[..range.start].chars().count(),
                    "match": &shown[range],
                    "shown": shown,
                })
            })
            .collect();
        serde_json::json!({ "summary": self.find_summary_line(cx), "searching": self.find.searching, "error": self.find.error, "rows": rows })
    }

    /// The text of every buffer with unsaved edits, by path: what Find in
    /// Workspace searches instead of their files, so it finds what the
    /// editor shows, on the lines it shows it.
    pub(super) fn unsaved_texts(&self, cx: &App) -> std::collections::HashMap<std::path::PathBuf, String> {
        self.buffers.iter().filter(|b| b.dirty).map(|b| (b.path.clone(), b.editor.read(cx).text().to_string())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{FindRow, MATCHES_SHOWN_PER_FILE, find_rows};
    use crate::search::{FileMatches, LineMatch};

    fn file(matches: usize) -> FileMatches {
        FileMatches {
            path: "/p/a.swift".into(),
            matches: (0..matches).map(|i| LineMatch { line: i as u32, column: 0, range: 0..1, preview: "x".into() }).collect(),
        }
    }

    #[test]
    fn rows_are_a_header_then_the_matches_capped() {
        let rows = find_rows(&[file(2), file(MATCHES_SHOWN_PER_FILE + 5)]);
        assert_eq!(rows.len(), 1 + 2 + 1 + MATCHES_SHOWN_PER_FILE);
        assert_eq!(rows[0], FindRow::File(0));
        assert_eq!(rows[2], FindRow::Match { file: 0, index: 1 });
        assert_eq!(rows[3], FindRow::File(1));
        assert_eq!(rows[rows.len() - 1], FindRow::Match { file: 1, index: MATCHES_SHOWN_PER_FILE - 1 });
        assert!(find_rows(&[]).is_empty());
    }
}

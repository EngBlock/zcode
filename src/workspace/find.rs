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
        let started = std::time::Instant::now();
        self.find._task = Some(cx.spawn(async move |this, cx| {
            let results = cx
                .background_spawn(async move { crate::search::search(&root, &files, &regex, &cancel) })
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

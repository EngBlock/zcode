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
    pub searching: bool,
    pub error: Option<String>,
    pub title: Option<String>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) _task: Option<Task<()>>,
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
            self.find.results = Arc::new(vec![]);
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
                this.find.results = Arc::new(results);
                this.find.searching = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

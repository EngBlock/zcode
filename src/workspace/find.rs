//! Find in Workspace (⇧⌘F): the Find navigator's query and results.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use gpui_kit::component::input::InputState;
use gpui_kit::*;

use super::Workspace;
use crate::search::{FileMatches, SearchOptions};

pub struct FindState {
    pub input: Entity<InputState>,
    pub options: SearchOptions,
    pub results: Arc<Vec<FileMatches>>,
    pub searching: bool,
    pub error: Option<String>,
    pub title: Option<String>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) _task: Option<Task<()>>,
}

impl Workspace {
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
        self.find._task = Some(cx.spawn(async move |this, cx| {
            let results = cx
                .background_spawn(async move { crate::search::search(&root, &files, &regex, &cancel) })
                .await;
            this.update(cx, |this, cx| {
                this.find.results = Arc::new(results);
                this.find.searching = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

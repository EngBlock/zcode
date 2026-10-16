//! Navigate ▸ Go Back / Go Forward (⌃⌘← / ⌃⌘→): each pane keeps where it
//! has been, as each of Xcode's editors does.

use gpui_kit::*;

use super::pane::{HistoryEntry, Pane, PaneId};
use super::{Target, Workspace};
use crate::actions::{GoBack, GoForward};

impl Workspace {
    fn current_location(&self, pane: PaneId, cx: &App) -> Option<HistoryEntry> {
        let item = self.pane(pane).and_then(Pane::active_editor_item)?;
        Some(HistoryEntry {
            path: item.path.clone(),
            offset: item.editor.read(cx).cursor(),
        })
    }

    /// Remembers where `pane` is before it navigates elsewhere.
    pub(super) fn push_history(&mut self, pane: PaneId, cx: &App) {
        let Some(entry) = self.current_location(pane, cx) else { return };
        let Some(pane) = self.pane_mut(pane) else { return };
        if pane.back.last() != Some(&entry) {
            pane.back.push(entry);
            if pane.back.len() > 100 {
                pane.back.remove(0);
            }
            pane.forward.clear();
        }
    }

    pub(super) fn go_back(&mut self, _: &GoBack, window: &mut Window, cx: &mut Context<Self>) {
        self.step_history(true, window, cx);
    }

    pub(super) fn go_forward(&mut self, _: &GoForward, window: &mut Window, cx: &mut Context<Self>) {
        self.step_history(false, window, cx);
    }

    fn step_history(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.active_pane;
        let current = self.current_location(id, cx);
        let Some(pane) = self.pane_mut(id) else { return };
        let (from, to) = if back { (&mut pane.back, &mut pane.forward) } else { (&mut pane.forward, &mut pane.back) };
        let Some(entry) = from.pop() else { return };
        to.extend(current);
        self.open_file_in(id, entry.path, Some(Target::Offset(entry.offset)), false, window, cx);
    }
}

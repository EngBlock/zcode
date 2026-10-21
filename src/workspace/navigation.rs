//! Navigate ▸ Go Back / Go Forward (⌃⌘← / ⌃⌘→): each pane keeps where it
//! has been, as each of Xcode's editors does.

use gpui_kit::*;

use super::pane::{HistoryEntry, Pane, PaneId};
use super::{Target, Workspace};
use crate::actions::{GoBack, GoForward};

impl Workspace {
    pub(super) fn current_location(&self, pane: PaneId, cx: &App) -> Option<HistoryEntry> {
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

    /// `pane`'s shown tab, at `closed`, was closed: Go Forward reopens it.
    pub(super) fn shown_tab_closed(&mut self, pane: PaneId, closed: HistoryEntry, cx: &App) {
        let now = self.current_location(pane, cx);
        if let Some(pane) = self.pane_mut(pane) {
            pane.tab_closed(closed, now.as_ref());
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
        let Some(entry) = self.pane_mut(id).and_then(|pane| pane.step(back, current)) else { return };
        self.open_file_in(id, entry.path, Some(Target::Offset(entry.offset)), false, window, cx);
    }
}

impl Pane {
    /// Where Go Back (or Forward) goes from `current`, which the other
    /// direction gets. Places that are where the pane already is are passed
    /// over.
    fn step(&mut self, back: bool, current: Option<HistoryEntry>) -> Option<HistoryEntry> {
        let (from, to) = if back { (&mut self.back, &mut self.forward) } else { (&mut self.forward, &mut self.back) };
        while from.last().is_some_and(|entry| Some(entry) == current.as_ref()) {
            from.pop();
        }
        let entry = from.pop()?;
        to.extend(current);
        Some(entry)
    }

    /// The shown tab, at `closed`, was closed and `now` is shown: as if Go
    /// Back had left `closed`, so Go Back goes on to where the pane was
    /// before and Go Forward reopens it.
    fn tab_closed(&mut self, closed: HistoryEntry, now: Option<&HistoryEntry>) {
        if now.is_some() && self.back.last() == now {
            self.back.pop();
        }
        if self.forward.last() != Some(&closed) {
            self.forward.push(closed);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::pane::{HistoryEntry, Pane, PaneId};

    fn at(file: &str, offset: usize) -> HistoryEntry {
        HistoryEntry { path: PathBuf::from(file), offset }
    }

    #[test]
    fn closing_the_shown_tab_goes_back_past_the_tab_now_shown_and_forward_to_the_closed_one() {
        // A opened, then B (A is remembered), then B's tab closed: A shows.
        let mut pane = Pane::new(PaneId(0));
        pane.back = vec![at("Z", 0), at("A", 4)];
        pane.tab_closed(at("B", 9), Some(&at("A", 4)));
        // Go Back: Z, not A again.
        assert_eq!(pane.step(true, Some(at("A", 4))), Some(at("Z", 0)));
        // Forward from Z: A, then the closed B.
        assert_eq!(pane.step(false, Some(at("Z", 0))), Some(at("A", 4)));
        assert_eq!(pane.step(false, Some(at("A", 4))), Some(at("B", 9)));
        assert_eq!(pane.step(false, Some(at("B", 9))), None);
    }

    #[test]
    fn stepping_passes_over_the_place_already_shown() {
        let mut pane = Pane::new(PaneId(0));
        pane.back = vec![at("A", 1), at("B", 2), at("B", 2)];
        assert_eq!(pane.step(true, Some(at("B", 2))), Some(at("A", 1)));
        assert_eq!(pane.forward, [at("B", 2)]);
        assert!(pane.back.is_empty());
        assert_eq!(pane.step(true, Some(at("A", 1))), None);
    }
}

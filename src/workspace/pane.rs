//! Panes: tabs of items in the editor area. An item is a file's editor (or
//! its property list editor) or the project settings; the workspace keeps
//! its panes in a split tree (`pane_group.rs`) and one of them focused.
//!
//! A file shown in two panes is one buffer (`Buffer`, in `buffers.rs`) with
//! an editor per pane, linked so they share text, undo and anchors; each
//! keeps its own cursor and scroll position.

use std::path::{Path, PathBuf};

use gpui_kit::*;
use zcode_editor::{EditorState, RangeDecorationCollection};

use super::{PlistEditor, ProjectSettings};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub u64);

/// A file's editor in one pane.
pub struct EditorItem {
    pub path: PathBuf,
    /// This pane's view of the buffer.
    pub editor: Entity<EditorState>,
    /// Property lists: the structured editor over `editor`.
    pub plist: Option<PlistView>,
    /// The line execution stopped on, in this view.
    pub(super) exec_mark: Option<RangeDecorationCollection>,
    pub(super) _subscriptions: Vec<Subscription>,
}

pub struct PlistView {
    pub editor: Entity<PlistEditor>,
    /// Open As ▸ Source Code.
    pub as_source: bool,
}

impl EditorItem {
    pub fn title(&self) -> String {
        file_title(&self.path)
    }

    /// The property list editor, when the tab shows it instead of the text.
    pub fn shown_plist(&self) -> Option<&Entity<PlistEditor>> {
        self.plist.as_ref().filter(|p| !p.as_source).map(|p| &p.editor)
    }
}

pub fn file_title(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

pub enum Item {
    Editor(EditorItem),
    ProjectSettings(Entity<ProjectSettings>),
}

impl Item {
    pub fn editor(&self) -> Option<&EditorItem> {
        match self {
            Item::Editor(item) => Some(item),
            Item::ProjectSettings(_) => None,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.editor().map(|item| item.path.as_path())
    }

    /// What gets focus when the item is shown.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Item::Editor(item) => match item.shown_plist() {
                Some(plist) => plist.read(cx).focus_handle(cx),
                None => item.editor.read(cx).focus_handle(cx),
            },
            Item::ProjectSettings(settings) => settings.read(cx).focus_handle(cx),
        }
    }
}

/// Xcode's assistant modes that zcode has: Counterparts shows the
/// counterpart (.h/.m) of the file in the pane it assists and follows it;
/// Manual shows what was opened in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssistantMode {
    Counterparts,
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assistant {
    /// The pane it assists.
    pub of: PaneId,
    pub mode: AssistantMode,
}

/// A place in a file, for Go Back / Go Forward.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct HistoryEntry {
    pub path: PathBuf,
    pub offset: usize,
}

pub struct Pane {
    pub id: PaneId,
    pub items: Vec<Item>,
    /// The shown item, when there are any.
    pub active: usize,
    pub assistant: Option<Assistant>,
    /// Navigation history, per pane as in Xcode.
    pub(super) back: Vec<HistoryEntry>,
    pub(super) forward: Vec<HistoryEntry>,
}

impl Pane {
    pub fn new(id: PaneId) -> Self {
        Self { id, items: vec![], active: 0, assistant: None, back: vec![], forward: vec![] }
    }

    pub fn active_item(&self) -> Option<&Item> {
        self.items.get(self.active)
    }

    pub fn active_editor_item(&self) -> Option<&EditorItem> {
        self.active_item().and_then(Item::editor)
    }

    pub fn position_of(&self, path: &Path) -> Option<usize> {
        self.items.iter().position(|item| item.path() == Some(path))
    }

    pub fn settings_position(&self) -> Option<usize> {
        self.items.iter().position(|item| matches!(item, Item::ProjectSettings(_)))
    }

    /// Removes the item at `ix`; the tab after it is shown, or the one
    /// before it when it was the last.
    pub fn remove(&mut self, ix: usize) -> Item {
        let item = self.items.remove(ix);
        self.active = next_active(self.active, ix, self.items.len());
        item
    }
}

/// The shown tab after removing tab `removed` from tabs where `active` was
/// shown and `len` are left.
fn next_active(active: usize, removed: usize, len: usize) -> usize {
    match active {
        _ if len == 0 => 0,
        active if active > removed => active - 1,
        active => active.min(len - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::next_active;

    #[test]
    fn closing_a_tab_shows_the_next_one_or_the_last() {
        // Tabs a b c, b shown: closing b shows c (now at 1).
        assert_eq!(next_active(1, 1, 2), 1);
        // c shown and closed: b.
        assert_eq!(next_active(2, 2, 2), 1);
        // a closed while c is shown: c moves to 1.
        assert_eq!(next_active(2, 0, 2), 1);
        // c closed while a is shown: a stays.
        assert_eq!(next_active(0, 2, 2), 0);
        assert_eq!(next_active(0, 0, 0), 0);
    }
}

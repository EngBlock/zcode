//! Panes: tabs of items in the editor area. An item is a file's editor (or
//! its property list editor), a file's comparison with `HEAD` (Code
//! Review), a multibuffer (excerpts of many files: find results, issues,
//! references, a rename), a terminal, the project settings, the
//! Organizer, an asset catalog or an image; the
//! workspace keeps its panes in a split tree (`pane_group.rs`) and one of
//! them focused.
//!
//! A file shown in two panes is one buffer (`Buffer`, in `buffers.rs`) with
//! an editor per pane, linked so they share text, undo and anchors; each
//! keeps its own cursor and scroll position.

use std::path::{Path, PathBuf};

use gpui_kit::*;
use zcode_editor::{EditorState, RangeDecorationCollection};

use super::diff_view::{DiffBase, DiffView};
use crate::terminal::TerminalView;
use super::multibuffer::MultiBufferView;
use super::asset_catalog::AssetCatalogView;
use super::image_view::ImageView;
use super::organizer::Organizer;
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

/// A file compared with `HEAD` (or with what it was before an agent
/// changed it), its local side a view of the file's buffer.
pub struct DiffItem {
    pub path: PathBuf,
    pub kind: DiffBase,
    pub view: Entity<DiffView>,
    pub(super) _subscriptions: Vec<Subscription>,
}

impl DiffItem {
    pub fn title(&self) -> String {
        match self.kind {
            DiffBase::Head => format!("{} (Changes)", file_title(&self.path)),
            DiffBase::BeforeAgent => format!("{} (Agent Changes)", file_title(&self.path)),
        }
    }
}

/// Excerpts of many files in one editor, edited in place.
pub struct MultiBufferItem {
    pub view: Entity<MultiBufferView>,
    /// The editor the excerpts are shown in.
    pub host: Entity<EditorState>,
    pub title: String,
    pub(super) _subscriptions: Vec<Subscription>,
}

/// A terminal in a tab.
pub struct TerminalItem {
    pub view: Entity<TerminalView>,
    pub(super) _subscriptions: Vec<Subscription>,
}

pub enum Item {
    Editor(EditorItem),
    Diff(DiffItem),
    MultiBuffer(MultiBufferItem),
    Terminal(TerminalItem),
    ProjectSettings(Entity<ProjectSettings>),
    /// Window ▸ Organizer.
    Organizer(Entity<Organizer>),
    /// An `.xcassets` folder.
    AssetCatalog(AssetCatalogItem),
    /// An image file (`image_view.rs`).
    Image(ImageItem),
}

/// An asset catalog in a tab.
pub struct AssetCatalogItem {
    /// The `.xcassets` folder.
    pub path: PathBuf,
    pub view: Entity<AssetCatalogView>,
    pub(super) _subscriptions: Vec<Subscription>,
}

/// An image file in a tab.
pub struct ImageItem {
    pub path: PathBuf,
    pub view: Entity<ImageView>,
    pub(super) _subscriptions: Vec<Subscription>,
}

impl Item {
    pub fn editor(&self) -> Option<&EditorItem> {
        match self {
            Item::Editor(item) => Some(item),
            Item::Diff(_) | Item::MultiBuffer(_) | Item::Terminal(_) | Item::ProjectSettings(_) | Item::Organizer(_) | Item::AssetCatalog(_) | Item::Image(_) => None,
        }
    }

    /// The file a file tab shows (not a comparison's: that tab isn't the
    /// file's), the asset catalog's folder, or the image's file.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Item::AssetCatalog(item) => Some(&item.path),
            Item::Image(item) => Some(&item.path),
            item => item.editor().map(|item| item.path.as_path()),
        }
    }

    /// The file whose buffer the tab edits: a file tab's or a comparison's.
    pub fn buffer_path(&self) -> Option<&Path> {
        match self {
            Item::Editor(item) => Some(&item.path),
            Item::Diff(diff) => Some(&diff.path),
            Item::MultiBuffer(_) | Item::Terminal(_) | Item::ProjectSettings(_) | Item::Organizer(_) | Item::AssetCatalog(_) | Item::Image(_) => None,
        }
    }

    /// The entity the tab is found by (`Workspace::find_view`).
    pub fn view_id(&self) -> EntityId {
        match self {
            Item::Editor(item) => item.editor.entity_id(),
            Item::Diff(diff) => diff.view.entity_id(),
            Item::MultiBuffer(item) => item.view.entity_id(),
            Item::Terminal(item) => item.view.entity_id(),
            Item::ProjectSettings(settings) => settings.entity_id(),
            Item::Organizer(organizer) => organizer.entity_id(),
            Item::AssetCatalog(item) => item.view.entity_id(),
            Item::Image(item) => item.view.entity_id(),
        }
    }

    pub fn title(&self, cx: &App) -> String {
        match self {
            Item::Editor(item) => item.title(),
            Item::Diff(diff) => diff.title(),
            Item::MultiBuffer(item) => item.title.clone(),
            Item::Terminal(item) => item.view.read(cx).title(),
            Item::ProjectSettings(_) => "Project Settings".into(),
            Item::Organizer(_) => "Organizer".into(),
            Item::AssetCatalog(item) => file_title(&item.path),
            Item::Image(item) => file_title(&item.path),
        }
    }

    /// What gets focus when the item is shown.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Item::Editor(item) => match item.shown_plist() {
                Some(plist) => plist.read(cx).focus_handle(cx),
                None => item.editor.read(cx).focus_handle(cx),
            },
            Item::Diff(diff) => diff.view.read(cx).focus_handle(cx),
            Item::MultiBuffer(item) => item.view.read(cx).focus_handle(cx),
            Item::Terminal(item) => item.view.read(cx).focus_handle(cx),
            Item::ProjectSettings(settings) => settings.read(cx).focus_handle(cx),
            Item::Organizer(organizer) => organizer.read(cx).focus_handle(cx),
            Item::AssetCatalog(item) => item.view.read(cx).focus_handle(cx),
            Item::Image(item) => item.view.read(cx).focus_handle(cx),
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

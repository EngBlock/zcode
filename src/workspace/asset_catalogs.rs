//! Asset catalogs in the window: an `.xcassets` folder (or a set in one)
//! opened from the navigator, Open Quickly or the command line shows the
//! catalog's editor (`asset_catalog.rs`) in a tab, one per catalog per pane;
//! its edits rescan the project. The control channel's `assets` and
//! `dump`'s `asset_catalog`.

use std::path::{Path, PathBuf};

use gpui_kit::*;
use serde_json::Value;

use super::Workspace;
use super::asset_catalog::{AssetCatalogEvent, AssetCatalogView};
use super::pane::{AssetCatalogItem, Item, PaneId};

impl Workspace {
    /// Shows `catalog` in `pane` (its tab added if needed), on `selected`.
    pub(super) fn open_asset_catalog_in(
        &mut self,
        pane: PaneId,
        catalog: PathBuf,
        selected: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self.pane(pane).and_then(|p| p.position_of(&catalog));
        let ix = match existing {
            Some(ix) => ix,
            None => match self.add_asset_catalog_item(pane, &catalog, selected.clone(), window, cx) {
                Some(ix) => ix,
                None => return,
            },
        };
        self.activate_item(pane, ix, window, cx);
        if existing.is_some()
            && let Some(view) = self.asset_catalog_view(pane, ix)
        {
            let path = selected.unwrap_or(catalog);
            view.update(cx, |view, cx| view.select_path(path, window, cx));
        }
    }

    /// A catalog's tab at the end of `pane`.
    pub(super) fn add_asset_catalog_item(
        &mut self,
        pane: PaneId,
        catalog: &Path,
        selected: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.pane(pane)?;
        // Removed files wait for ⌘Z outside the project, where builds don't
        // see them.
        let stash = self.build.read(cx).cache_dir().join("AssetUndo");
        let lock = self.project_edit_lock.clone();
        let root = catalog.to_path_buf();
        let view = cx.new(|cx| AssetCatalogView::new(root, selected, stash, lock, window, cx));
        let id = view.entity_id();
        let focus = view.read(cx).focus_handle(cx);
        let subscriptions = vec![
            cx.on_focus_in(&focus, window, move |this, window, cx| this.view_focused(id, window, cx)),
            cx.subscribe_in(&view, window, |this, _, event: &AssetCatalogEvent, window, cx| match event {
                AssetCatalogEvent::FilesChanged => {
                    this.rescan(cx);
                    this.refresh_scm(cx);
                }
                AssetCatalogEvent::Error(message) => this.notify_error(message.clone(), window, cx),
            }),
        ];
        let p = self.pane_mut(pane)?;
        p.items.push(Item::AssetCatalog(AssetCatalogItem { path: catalog.to_path_buf(), view, _subscriptions: subscriptions }));
        Some(p.items.len() - 1)
    }

    fn asset_catalog_view(&self, pane: PaneId, ix: usize) -> Option<Entity<AssetCatalogView>> {
        match self.pane(pane)?.items.get(ix)? {
            Item::AssetCatalog(item) => Some(item.view.clone()),
            _ => None,
        }
    }

    /// The focused pane's catalog, else the first one open.
    fn active_asset_catalog(&self) -> Option<Entity<AssetCatalogView>> {
        let shown = self.pane(self.active_pane).and_then(|p| self.asset_catalog_view(p.id, p.active));
        shown.or_else(|| {
            self.panes.iter().flat_map(|p| &p.items).find_map(|item| match item {
                Item::AssetCatalog(item) => Some(item.view.clone()),
                _ => None,
            })
        })
    }

    /// The control channel's `assets <command>` for the shown catalog (see
    /// `AssetCatalogView::command`).
    pub fn assets_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        let view = self.active_asset_catalog().ok_or("no asset catalog is open")?;
        view.update(cx, |view, cx| view.command(command, window, cx))?;
        Ok(String::new())
    }

    /// `dump`'s `asset_catalog`.
    pub(super) fn asset_catalog_summary(&self, cx: &App) -> Value {
        self.active_asset_catalog().map(|view| view.read(cx).summary(cx)).unwrap_or(Value::Null)
    }
}

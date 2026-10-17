//! Product ▸ Archive and the Organizer in the window: archiving like a
//! build (the log shown), the Organizer tab revealing a new archive,
//! Distribute App's sheet and its run, and their state for the control
//! channel and MCP. Archiving and exporting are `crate::build::archive`'s.

use std::path::PathBuf;

use gpui_kit::*;
use serde_json::{Value, json};

use super::distribute_sheet::DistributeSheet;
use super::organizer::{Organizer, archive_json};
use super::pane::Item;
use super::{DebugTab, Workspace};
use crate::actions::{Archive as ArchiveAction, ShowOrganizer};
use crate::build::BuildAction;
use crate::build::archive::{Archive, DistributeRequest};

impl Workspace {
    pub(super) fn archive(&mut self, _: &ArchiveAction, window: &mut Window, cx: &mut Context<Self>) {
        self.start_build(BuildAction::Archive, window, cx);
    }

    pub(super) fn show_organizer(&mut self, _: &ShowOrganizer, window: &mut Window, cx: &mut Context<Self>) {
        self.open_organizer(None, window, cx);
    }

    /// Window ▸ Organizer: its tab (opened at the end of the focused pane's
    /// when it isn't there), on `reveal` when given.
    pub fn open_organizer(&mut self, reveal: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let organizer = match self.organizer.clone() {
            Some(organizer) => {
                match reveal {
                    Some(path) => organizer.update(cx, |o, cx| o.reveal(path, cx)),
                    None => organizer.update(cx, |o, cx| o.refresh(cx)),
                }
                organizer
            }
            None => {
                let scheme = self.build.read(cx).scheme.clone();
                let weak = cx.weak_entity();
                let organizer = cx.new(|cx| {
                    let mut organizer = Organizer::new(weak, scheme, cx);
                    if let Some(path) = reveal {
                        organizer.reveal(path, cx);
                    }
                    organizer
                });
                let view = organizer.entity_id();
                let focus = organizer.read(cx).focus_handle(cx);
                self.organizer_subscriptions = vec![cx.on_focus_in(&focus, window, move |this, window, cx| this.view_focused(view, window, cx))];
                self.organizer = Some(organizer.clone());
                organizer
            }
        };
        let (pane, ix) = match self.find_view(organizer.entity_id()) {
            Some(found) => found,
            None => {
                let pane = self.active_pane;
                let Some(p) = self.pane_mut(pane) else { return };
                p.items.push(Item::Organizer(organizer));
                (pane, p.items.len() - 1)
            }
        };
        self.activate_item(pane, ix, window, cx);
    }

    /// Product ▸ Archive made an archive: the Organizer shows it, unless
    /// the scheme's Archive action says not to.
    pub(super) fn archived(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let reveal = self.build.read(cx).scheme_options.as_ref().is_none_or(|l| l.options.archive.reveal_in_organizer);
        if reveal {
            self.open_organizer(Some(path), window, cx);
        } else if let Some(organizer) = &self.organizer {
            organizer.update(cx, |o, cx| o.refresh(cx));
        }
    }

    /// A distribution ended: the Organizer shows how.
    pub(super) fn distributed(&mut self, cx: &mut Context<Self>) {
        if let Some(organizer) = &self.organizer {
            organizer.update(cx, |_, cx| cx.notify());
        }
    }

    /// Distribute App's sheet for `archive`.
    pub fn show_distribute_sheet(&mut self, archive: Archive, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.latest_settings(cx).notary_keychain_profile;
        let cache_dir = self.build.read(cx).cache_dir().to_path_buf();
        let weak = cx.weak_entity();
        let sheet = cx.new(|cx| DistributeSheet::new(weak, archive, profile, cache_dir, window, cx));
        self.distribute_sheet = Some(sheet.downgrade());
        self.show_modal(sheet, window, cx);
    }

    /// Distribute App confirmed: the export runs like a build, its output
    /// in the build log. A keychain profile typed in the sheet is kept in
    /// the user's settings for next time.
    pub fn distribute(&mut self, request: DistributeRequest, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(profile) = &request.notarize
            && *profile != self.latest_settings(cx).notary_keychain_profile
            && let Err(err) = crate::settings::set(None, &["distribution", "notary_keychain_profile"], json!(profile), cx)
        {
            ::log::warn!("couldn’t keep the keychain profile in settings.json: {err}");
        }
        self.show_debug_area = true;
        self.debug_tab = DebugTab::BuildLog;
        self.build.update(cx, |b, cx| b.distribute(request, cx));
        if let Some(organizer) = &self.organizer {
            organizer.update(cx, |_, cx| cx.notify());
        }
        let _ = window;
        cx.notify();
    }

    /// The control channel's `organizer <command>`: `open`, `refresh`,
    /// `select <archive>` (a path, file name or name), `group <app>`,
    /// `distribute` (the sheet), `delete`, `finder`.
    pub fn organizer_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
        if verb == "open" {
            self.open_organizer(None, window, cx);
            return Ok(String::new());
        }
        let organizer = self.organizer.clone().ok_or("the Organizer isn't open")?;
        match verb {
            "refresh" => organizer.update(cx, |o, cx| o.refresh(cx)),
            "select" => organizer.update(cx, |o, cx| o.select_named(arg, cx))?,
            "group" => organizer.update(cx, |o, cx| o.select_group(arg.to_string(), cx)),
            "distribute" => organizer.update(cx, |o, cx| o.distribute(window, cx))?,
            "delete" => organizer.update(cx, |o, cx| o.delete_selected(cx))?,
            "finder" => organizer.update(cx, |o, cx| o.show_in_finder(cx)),
            other => return Err(format!("usage: organizer open|refresh|select <archive>|group <app>|distribute|delete|finder (not “{other}”)")),
        }
        Ok(String::new())
    }

    /// The control channel's `distribute <command>` for the open sheet
    /// (see `DistributeSheet::command`).
    pub fn distribute_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        let sheet = self.distribute_sheet.as_ref().and_then(|s| s.upgrade()).ok_or("Distribute App isn't open")?;
        sheet.update(cx, |sheet, cx| sheet.command(command, window, cx))?;
        Ok(String::new())
    }

    /// `dump`'s `archives`: the Organizer, the sheet, the last archive and
    /// distribution.
    pub(super) fn archives_summary(&self, cx: &App) -> Value {
        let build = self.build.read(cx);
        let sheet = self.distribute_sheet.as_ref().and_then(|s| s.upgrade());
        json!({
            "dir": crate::paths::archives_dir(),
            "organizer": self.organizer.as_ref().map(|o| o.read(cx).summary()),
            "sheet": sheet.map(|s| s.read(cx).summary(cx)),
            "distributing": build.distributing.as_ref().map(|d| d.choices.method.id()),
            "last_archive": build.last_archive.as_ref().map(|o| json!({
                "archive": o.archive.as_ref().map(archive_json),
                "error": o.error,
            })),
            "last_distribution": build.last_distribution.as_ref().map(|d| json!({
                "archive": d.archive,
                "method": d.method.id(),
                "uploaded": d.uploaded,
                "export_path": d.export_path,
                "products": d.products,
                "notarization": d.notarization.as_ref().map(|n| json!({ "id": n.id, "status": n.status, "message": n.message })),
                "stapled": d.stapled,
                "error": d.error,
            })),
        })
    }

    /// Product ▸ Archive for MCP. The run's generation, to wait for.
    pub fn mcp_archive(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<u64, String> {
        if !self.project.kind.is_xcode() {
            return Err("Archive needs an Xcode project or workspace; Swift packages and folders can't be archived.".into());
        }
        self.start_build(BuildAction::Archive, window, cx);
        let build = self.build.read(cx);
        if !build.phase.is_busy() {
            return Err("The archive didn’t start.".into());
        }
        Ok(build.generation)
    }
}

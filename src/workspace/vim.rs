//! Editor ▸ Vim Mode in every editor tab: files in every pane, both sides
//! of Code Review and multibuffers; not property list outlines, terminals
//! or the text fields of sheets and navigators, which aren't editors. The
//! modal editing is the editor crate's (`crates/zcode-editor/src/vim`);
//! here are the setting, the status bar's indicator and what acts on tabs:
//! `:w` saves what the tab shows (a multibuffer saves the files it
//! changed), `:q` closes the tab (Zed's choice; Vim would close the window)
//! and refuses unsaved changes without `!`, `gt` / `gT` switch tabs, `⌃O` /
//! Tab go back and forward, and `=` re-indents through Editor ▸ Re-Indent.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use zcode_editor::{EditorState, VimCommand, VimStatus, VimStatusChanged};

use super::{Item, PaneId, Workspace};
use crate::actions::{GoBack, GoForward, NextTab, PreviousTab, ReIndent};
use crate::settings::Settings;

impl Workspace {
    /// Editor ▸ Vim Mode: `vim_mode` in the user's settings.
    pub(super) fn toggle_vim_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on = Settings::get(cx).vim_mode;
        if let Err(err) = crate::settings::set(None, &["vim_mode"], serde_json::json!(!on), cx) {
            self.notify_error(err, window, cx);
        }
    }

    /// For an editor a tab shows: Vim mode as the setting says, and its
    /// commands handled for the tab. Nothing runs per keystroke while Vim
    /// mode is off.
    pub(super) fn install_vim(&mut self, editor: &Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Vec<Subscription> {
        Self::set_vim(editor, Settings::get(cx).vim_mode, cx);
        vec![
            cx.subscribe_in(editor, window, |this, editor, command: &VimCommand, window, cx| {
                this.vim_command(editor.clone(), command, window, cx)
            }),
            cx.subscribe(editor, |_, _, _: &VimStatusChanged, cx| cx.notify()),
        ]
    }

    fn set_vim(editor: &Entity<EditorState>, on: bool, cx: &mut App) {
        let style = HighlightStyle { background_color: Some(cx.theme().warning.opacity(0.35)), ..Default::default() };
        editor.update(cx, |state, cx| {
            state.set_vim_enabled(on, cx);
            if on {
                state.set_vim_search_style(style, cx);
            }
        });
    }

    /// After the setting changed, in any window: every editor tab follows.
    pub(super) fn apply_vim_mode(&mut self, cx: &mut Context<Self>) {
        let on = Settings::get(cx).vim_mode;
        for editor in self.vim_editors(cx) {
            Self::set_vim(&editor, on, cx);
        }
        cx.notify();
    }

    /// The editors of every tab Vim mode applies to.
    fn vim_editors(&self, cx: &App) -> Vec<Entity<EditorState>> {
        self.panes
            .iter()
            .flat_map(|pane| pane.items.iter())
            .flat_map(|item| item.editors(cx))
            .collect()
    }

    /// The tab an editor belongs to.
    fn tab_of_editor(&self, editor: EntityId, cx: &App) -> Option<(PaneId, usize)> {
        self.panes.iter().find_map(|pane| {
            let ix = pane.items.iter().position(|item| item.editors(cx).iter().any(|e| e.entity_id() == editor))?;
            Some((pane.id, ix))
        })
    }

    /// What the status bar shows for the focused tab's editor.
    pub(super) fn vim_status(&self, window: &Window, cx: &App) -> Option<VimStatus> {
        let editor = match self.pane(self.active_pane)?.active_item()? {
            Item::Editor(item) => item.shown_plist().is_none().then(|| item.editor.clone())?,
            Item::MultiBuffer(item) => item.host.clone(),
            Item::Diff(diff) => {
                let diff = diff.view.read(cx);
                let base_focused = diff.base.read(cx).focus_handle(cx).is_focused(window);
                if base_focused { diff.base.clone() } else { diff.current.clone() }
            }
            Item::Terminal(_) | Item::ProjectSettings(_) | Item::Organizer(_) | Item::AssetCatalog(_) | Item::Image(_) | Item::Rendered(_) => return None,
        };
        editor.read(cx).vim_status()
    }

    /// For the control channel's `dump`.
    pub(super) fn vim_summary(&self, window: &Window, cx: &App) -> serde_json::Value {
        let editors = self.vim_editors(cx);
        let status = self.vim_status(window, cx);
        serde_json::json!({
            "enabled": Settings::get(cx).vim_mode,
            "editors": editors.len(),
            "editors_on": editors.iter().filter(|e| e.read(cx).is_vim_enabled()).count(),
            "mode": status.as_ref().map(|s| s.mode.label()),
            "pending": status.as_ref().map(|s| s.pending.to_string()),
            "command_line": status.as_ref().and_then(|s| s.command_line.as_ref().map(|c| c.to_string())),
            "message": status.as_ref().and_then(|s| s.message.as_ref().map(|m| m.to_string())),
        })
    }

    fn vim_command(&mut self, editor: Entity<EditorState>, command: &VimCommand, window: &mut Window, cx: &mut Context<Self>) {
        let Some((pane, ix)) = self.tab_of_editor(editor.entity_id(), cx) else { return };
        match command {
            VimCommand::Write { all: false, quit } => {
                self.save_tab(pane, ix, window, cx);
                if *quit {
                    self.close_item(pane, ix, window, cx);
                }
            }
            VimCommand::Write { all: true, quit } => {
                self.save_all_from_user(window, cx);
                if *quit {
                    self.close_all_tabs(window, cx);
                }
            }
            VimCommand::Quit { all: false, force } => {
                let Some(item) = self.pane(pane).and_then(|p| p.items.get(ix)) else { return };
                let unsaved = self.unsaved_if_closed([item], cx);
                if !force && !unsaved.is_empty() {
                    editor.update(cx, |state, cx| {
                        state.set_vim_message("E37: No write since last change (add ! to override)", true, cx)
                    });
                    return;
                }
                self.force_close_item(pane, ix, window, cx);
                // As Don't Save: a buffer a preview keeps goes back to the file.
                for path in &unsaved {
                    self.discard_changes(path, window, cx);
                }
            }
            VimCommand::Quit { all: true, force } => {
                let items: Vec<&Item> = self.panes.iter().flat_map(|p| p.items.iter()).collect();
                let unsaved = self.unsaved_if_closed(items, cx);
                if !force && !unsaved.is_empty() {
                    let message = format!("E37: No write since last change in {} (add ! to override)", super::pane::file_title(&unsaved[0]));
                    editor.update(cx, |state, cx| state.set_vim_message(message, true, cx));
                    return;
                }
                self.close_all_tabs(window, cx);
                for path in &unsaved {
                    self.discard_changes(path, window, cx);
                }
            }
            VimCommand::NextTab => window.dispatch_action(Box::new(NextTab), cx),
            VimCommand::PreviousTab => window.dispatch_action(Box::new(PreviousTab), cx),
            VimCommand::GoBack => window.dispatch_action(Box::new(GoBack), cx),
            VimCommand::GoForward => window.dispatch_action(Box::new(GoForward), cx),
            VimCommand::Reindent(range) => {
                // Re-Indent works on the focused editor's selection.
                editor.update(cx, |state, cx| state.set_selected_range(range.clone(), cx));
                self.re_indent(&ReIndent, window, cx);
                editor.update(cx, |state, cx| state.set_selected_range(range.start..range.start, cx));
            }
        }
    }

    /// ⌘S for one tab.
    fn save_tab(&mut self, pane: PaneId, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.pane(pane).and_then(|p| p.items.get(ix)) else { return };
        match item {
            Item::MultiBuffer(item) => {
                let view = item.view.clone();
                self.save_multibuffer(&view, window, cx);
            }
            item => {
                if let Some(path) = item.buffer_path().map(|p| p.to_path_buf()) {
                    self.save_path_from_user(&path, window, cx);
                }
            }
        }
    }

    /// `:qa`, `:wqa`: every tab closes, as `:q!` on each; the window stays.
    fn close_all_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        loop {
            let Some((pane, ix)) = self.panes.iter().find(|p| !p.items.is_empty()).map(|p| (p.id, p.items.len() - 1)) else { break };
            let before: usize = self.panes.iter().map(|p| p.items.len()).sum();
            self.force_close_item(pane, ix, window, cx);
            let after: usize = self.panes.iter().map(|p| p.items.len()).sum();
            if after >= before {
                break;
            }
        }
    }
}

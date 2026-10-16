//! The popover a change bar opens: the change's lines as they were in
//! `HEAD` and as they are now, Discard Change (Xcode's name), Stage Change /
//! Unstage Change (Zed's), and the next and previous change.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::Workspace;
use crate::actions::PickerDismiss;
use crate::git::diff::{self, HunkKind, Staging};

/// Lines shown of each side before "… more lines".
const MAX_LINES: usize = 24;

/// What the popover shows of a hunk.
struct HunkData {
    path: PathBuf,
    ix: usize,
    total: usize,
    kind: HunkKind,
    staging: Staging,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
}

impl HunkData {
    /// Hunk `ix` of `path`, or `None` when it's gone.
    fn read(workspace: &Workspace, path: &Path, ix: usize, cx: &App) -> Option<Self> {
        let buffer = workspace.buffer(path)?;
        let hunk = buffer.git.hunks.get(ix)?.clone();
        let head = buffer.git.bases.as_ref().and_then(|b| b.head.clone()).unwrap_or_default();
        let current = buffer.editor.read(cx).text().to_string();
        let lines = |text: &str, rows| diff::slice(&diff::lines(text), rows).iter().map(|l| l.trim_end_matches('\n').to_string()).collect();
        Some(Self {
            path: path.to_path_buf(),
            ix,
            total: buffer.git.hunks.len(),
            kind: hunk.hunk.kind(),
            staging: hunk.staging,
            old_lines: lines(&head, &hunk.hunk.old),
            new_lines: lines(&current, &hunk.hunk.new),
        })
    }
}

pub struct HunkPopover {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    data: HunkData,
}

impl EventEmitter<DismissEvent> for HunkPopover {}

impl Focusable for HunkPopover {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl HunkPopover {
    fn dismiss(&mut self, _: &PickerDismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    /// Discard, stage or unstage the change, or show the next or previous
    /// one (`next`, `previous`): its buttons, and the control channel's
    /// `git popover <command>`. The workspace acts once the current update
    /// is over (the control channel calls this from inside it).
    pub fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (path, ix, total) = (self.data.path.clone(), self.data.ix, self.data.total);
        let workspace = self.workspace.clone();
        match command {
            "discard" | "stage" | "unstage" => {
                cx.emit(DismissEvent);
                let command = command.to_string();
                window.defer(cx, move |window, cx| {
                    workspace
                        .update(cx, |ws, cx| match command.as_str() {
                            "discard" => ws.discard_hunk(&path, ix, window, cx),
                            stage => ws.stage_hunk(&path, ix, stage == "stage", window, cx),
                        })
                        .ok();
                });
                Ok(())
            }
            "next" | "previous" => {
                if total == 0 {
                    return Ok(());
                }
                let ix = if command == "next" { (ix + 1) % total } else { (ix + total - 1) % total };
                let popover = cx.entity().downgrade();
                window.defer(cx, move |_, cx| {
                    let Some(ws) = workspace.upgrade() else { return };
                    let Some(data) = HunkData::read(ws.read(cx), &path, ix, cx) else { return };
                    let row = ws.read(cx).buffer(&path).and_then(|b| b.git.hunks.get(ix)).map(|h| h.hunk.new.start).unwrap_or(0);
                    popover
                        .update(cx, |popover, cx| {
                            popover.data = data;
                            cx.notify();
                        })
                        .ok();
                    // Show it in the editor too, keeping the popover's focus.
                    ws.update(cx, |ws, cx| ws.reveal_line_quietly(&path, row, cx));
                });
                Ok(())
            }
            other => Err(format!("unknown popover command {other}")),
        }
    }

    pub fn summary(&self) -> serde_json::Value {
        let data = &self.data;
        serde_json::json!({
            "path": data.path,
            "index": data.ix,
            "total": data.total,
            "kind": format!("{:?}", data.kind),
            "staging": format!("{:?}", data.staging),
            "old": data.old_lines,
            "new": data.new_lines,
        })
    }
}

impl Render for HunkPopover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let data = &self.data;
        let mono = theme.mono_font_family.clone();
        let (title, staged_label) = (
            match data.kind {
                HunkKind::Added => "Added",
                HunkKind::Modified => "Modified",
                HunkKind::Deleted => "Deleted",
            },
            match data.staging {
                Staging::Staged => " · Staged",
                Staging::Partial => " · Partly Staged",
                Staging::Unstaged => "",
            },
        );
        let staged = data.staging == Staging::Staged;
        let button = |id: &'static str, label: &'static str| Button::new(id).label(label).xsmall();
        v_flex()
            .id("hunk-popover")
            .key_context("HunkPopover")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::dismiss))
            .min_w(px(320.))
            .max_w(px(720.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .text_sm()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(format!("{title}{staged_label}")))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(format!("Change {} of {}", data.ix + 1, data.total)))
                    .child(Button::new("hunk-previous").icon(IconName::ArrowUp).ghost().xsmall().tooltip("Previous Change (⌃⇧\\)").on_click(
                        cx.listener(|this, _, window, cx| this.command("previous", window, cx).unwrap_or_default()),
                    ))
                    .child(Button::new("hunk-next").icon(IconName::ArrowDown).ghost().xsmall().tooltip("Next Change (⌃\\)").on_click(
                        cx.listener(|this, _, window, cx| this.command("next", window, cx).unwrap_or_default()),
                    )),
            )
            .child(
                div()
                    .id("hunk-lines")
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .py_1()
                    .font_family(mono)
                    .text_xs()
                    .child(hunk_lines(&data.old_lines, &data.new_lines, MAX_LINES, cx)),
            )
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .justify_end()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(button("hunk-stage", if staged { "Unstage Change" } else { "Stage Change" }).on_click(cx.listener(
                        move |this, _, window, cx| this.command(if staged { "unstage" } else { "stage" }, window, cx).unwrap_or_default(),
                    )))
                    .child(button("hunk-discard", "Discard Change").danger().on_click(
                        cx.listener(|this, _, window, cx| this.command("discard", window, cx).unwrap_or_default()),
                    )),
            )
    }
}

/// A change's lines, `HEAD`'s (−, red) above the new ones (+, green), at
/// most `max` of each.
pub(super) fn hunk_lines(old: &[String], new: &[String], max: usize, cx: &App) -> Div {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let side = |lines: &[String], sign: &'static str, color: Hsla| {
        let more = lines.len().saturating_sub(max);
        v_flex()
            .children(lines.iter().take(max).map(|line| {
                h_flex()
                    .px_2()
                    .bg(color.opacity(0.16))
                    .child(div().w(px(14.)).flex_none().text_color(color).child(sign))
                    .child(div().whitespace_nowrap().child(if line.is_empty() { " ".to_string() } else { line.clone() }))
            }))
            .when(more > 0, |el| el.child(div().px_2().text_color(muted).child(format!("… {more} more lines"))))
    };
    v_flex().child(side(old, "−", theme.danger)).child(side(new, "+", theme.success))
}

impl Workspace {
    /// Opens the popover of hunk `ix` of `path` at `position` (its bar).
    pub(super) fn show_hunk_popover(&mut self, path: PathBuf, ix: usize, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = HunkData::read(self, &path, ix, cx) else { return };
        let workspace = cx.weak_entity();
        let popover = cx.new(|cx| HunkPopover { focus_handle: cx.focus_handle(), workspace, data });
        self.git.popover = Some(popover.downgrade());
        self.show_modal_at(popover, Some(position), window, cx);
    }

    /// Moves the cursor of the focused pane's view of `path` to `row`
    /// without taking focus (the popover keeps it).
    pub(super) fn reveal_line_quietly(&mut self, path: &Path, row: u32, cx: &mut Context<Self>) {
        let Some(editor) = self.active_item().filter(|item| item.path == path).map(|item| item.editor.clone()) else { return };
        editor.update(cx, |state, cx| {
            let offset = zcode_editor::RopeExt::line_start_offset(state.text(), row as usize);
            state.set_selected_range(offset..offset, cx);
        });
    }
}

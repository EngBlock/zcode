//! Drawing the agent panel: its header (agent, mode, Follow, New Thread),
//! the thread (a virtual list that follows its end while the agent
//! writes), the plan, the files the agent changed, and the message field.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::message_scroller::MessageScroller;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{AgentPanel, Status};
use crate::acp::schema::{PermissionOptionKind, PlanEntryStatus, ToolCallStatus, ToolKind};
use crate::acp::{Entry, ToolCallEntry, ToolContent};

/// Diff lines a tool call shows before "Show All".
const DIFF_PREVIEW_LINES: usize = 24;
/// Lines of a tool call's text output shown before "Show All".
const OUTPUT_PREVIEW_LINES: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub enum DiffLineKind {
    Added,
    Removed,
    /// Between hunks: where the next one starts.
    Gap,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// 1-based, in the old text for a removed line, the new one otherwise.
    pub number: u32,
    pub text: String,
}

/// A tool call's diff as lines: each change's removed lines, then its added ones.
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    let hunks = crate::git::diff::line_hunks(old, new);
    let (old_lines, new_lines) = (crate::git::diff::lines(old), crate::git::diff::lines(new));
    let mut out = vec![];
    for (ix, hunk) in hunks.iter().enumerate() {
        if ix > 0 {
            out.push(DiffLine { kind: DiffLineKind::Gap, number: hunk.new.start + 1, text: String::new() });
        }
        for row in hunk.old.clone() {
            let text = old_lines.get(row as usize).map(|l| l.trim_end_matches('\n').to_string()).unwrap_or_default();
            out.push(DiffLine { kind: DiffLineKind::Removed, number: row + 1, text });
        }
        for row in hunk.new.clone() {
            let text = new_lines.get(row as usize).map(|l| l.trim_end_matches('\n').to_string()).unwrap_or_default();
            out.push(DiffLine { kind: DiffLineKind::Added, number: row + 1, text });
        }
    }
    out
}

fn kind_icon(kind: ToolKind) -> IconName {
    match kind {
        ToolKind::Read => IconName::FileText,
        ToolKind::Edit => IconName::Pencil,
        ToolKind::Delete => IconName::Trash,
        ToolKind::Move => IconName::ArrowRight,
        ToolKind::Search => IconName::Search,
        ToolKind::Execute => IconName::SquareTerminal,
        ToolKind::Think => IconName::Brain,
        ToolKind::Fetch => IconName::Globe,
        ToolKind::SwitchMode => IconName::Layers,
        _ => IconName::Wrench,
    }
}

fn markdown(id: impl Into<ElementId>, text: String) -> TextView {
    TextView::markdown(id, text)
        .style(TextViewStyle::default().paragraph_gap(rems(0.5)).heading_font_size(|level, rem| match level {
            1 => rem * 1.15,
            2 => rem * 1.05,
            _ => rem,
        }))
        .selectable(true)
}

impl AgentPanel {
    pub(super) fn render_entry(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(entry) = self.thread.entries.get(ix).cloned() else { return div().into_any_element() };
        let row = div().w_full().px_3().py_1p5();
        match entry {
            Entry::User(message) => row
                .child(
                    div()
                        .id(("agent-user", ix))
                        .px_3()
                        .py_2()
                        .rounded(theme.radius_lg)
                        .bg(theme.secondary)
                        .border_1()
                        .border_color(theme.border)
                        .text_sm()
                        .child(message.text),
                )
                .into_any_element(),
            Entry::Assistant(text) => row.text_sm().child(markdown(("agent-message", ix), text)).into_any_element(),
            Entry::Thought(text) => {
                let expanded = self.expanded.contains(&ix);
                let first = text.trim().lines().next().unwrap_or("").to_string();
                row.child(
                    v_flex()
                        .gap_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(
                            h_flex()
                                .id(("agent-thought", ix))
                                .gap_1()
                                .cursor_pointer()
                                .child(Icon::new(IconName::Brain).xsmall())
                                .child(div().font_weight(FontWeight::MEDIUM).child("Thinking"))
                                .child(Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall())
                                .when(!expanded, |el| el.child(div().flex_1().min_w_0().truncate().child(first)))
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(ix, cx))),
                        )
                        .when(expanded, |el| el.child(div().pl_4().child(markdown(("agent-thought-text", ix), text)))),
                )
                .into_any_element()
            }
            Entry::Tool(call) => row.child(self.render_tool_call(ix, &call, cx)).into_any_element(),
        }
    }

    fn render_tool_call(&mut self, ix: usize, call: &ToolCallEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let expanded = self.expanded.contains(&ix);
        let status: AnyElement = match call.status {
            ToolCallStatus::InProgress => Spinner::new().xsmall().into_any_element(),
            ToolCallStatus::Completed => Icon::new(IconName::CircleCheck).xsmall().text_color(theme.success).into_any_element(),
            ToolCallStatus::Failed => Icon::new(IconName::CircleX).xsmall().text_color(theme.danger).into_any_element(),
            _ => Icon::new(IconName::CircleDashed).xsmall().text_color(theme.muted_foreground).into_any_element(),
        };
        let mut card = v_flex()
            .id(("agent-tool", ix))
            .w_full()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .text_xs()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1p5()
                    .child(Icon::new(kind_icon(call.kind)).xsmall().text_color(theme.muted_foreground))
                    .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(call.title.clone()))
                    .child(status),
            );
        // Where it reads or writes: click to open.
        if !call.locations.is_empty() {
            let mut places = h_flex().px_2().pb_1().gap_2().flex_wrap();
            for (n, (path, line)) in call.locations.iter().take(6).enumerate() {
                let label = match line {
                    Some(line) if *line > 0 => format!("{}:{line}", file_name(path)),
                    _ => file_name(path),
                };
                let (path, line) = (path.clone(), *line);
                places = places.child(
                    div()
                        .id(("agent-location", ix * 16 + n))
                        .text_color(theme.link)
                        .cursor_pointer()
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| this.follow_to(&path, line.map(|l| l.saturating_sub(1)), window, cx))),
                );
            }
            card = card.child(places);
        }
        for (n, content) in call.content.iter().enumerate() {
            match content {
                ToolContent::Text(text) if text.trim().is_empty() => {}
                ToolContent::Text(text) => {
                    let lines: Vec<&str> = text.trim_end().lines().collect();
                    let shown = if expanded { lines.len() } else { lines.len().min(OUTPUT_PREVIEW_LINES) };
                    card = card.child(
                        div()
                            .mx_2()
                            .mb_1()
                            .p_1p5()
                            .rounded(theme.radius)
                            .bg(theme.muted)
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .whitespace_normal()
                            .children(lines[..shown].iter().map(|l| div().child(l.to_string()))),
                    );
                    if shown < lines.len() {
                        card = card.child(self.show_all(ix, lines.len() - shown, "line", cx));
                    }
                }
                ToolContent::Diff { path, old, new } => {
                    let lines = self
                        .diff_cache
                        .entry((ix, n))
                        .or_insert_with(|| Rc::new(diff_lines(old.as_deref().unwrap_or(""), new)))
                        .clone();
                    card = card.child(self.render_diff(ix, n, path, old.is_none(), &lines, expanded, cx));
                }
                ToolContent::Terminal(_) => {}
            }
        }
        if let Some(permission) = &call.permission {
            if permission.pending() {
                let mut buttons = h_flex().px_2().pb_2().pt_1().gap_1().flex_wrap();
                for (n, option) in permission.options.iter().enumerate() {
                    let (call_id, option_id) = (call.id.clone(), option.option_id.0.to_string());
                    let button = Button::new(("agent-permission", ix * 16 + n)).label(option.name.clone()).xsmall();
                    let button = match option.kind {
                        PermissionOptionKind::AllowOnce => button.primary(),
                        PermissionOptionKind::RejectOnce | PermissionOptionKind::RejectAlways => button.ghost(),
                        _ => button,
                    };
                    buttons = buttons.child(button.on_click(cx.listener(move |this, _, _, cx| this.answer_permission(&call_id, Some(option_id.clone()), cx))));
                }
                card = card.border_color(theme.warning).child(buttons);
            } else if let Some(outcome) = &permission.outcome {
                let said = match outcome {
                    Some(id) => permission.options.iter().find(|o| o.option_id.0.as_ref() == id).map(|o| o.name.clone()).unwrap_or_else(|| id.clone()),
                    None => "Cancelled".into(),
                };
                card = card.child(div().px_2().pb_1().text_color(theme.muted_foreground).child(said));
            }
        }
        card
    }

    #[allow(clippy::too_many_arguments)]
    fn render_diff(&self, ix: usize, n: usize, path: &std::path::Path, created: bool, lines: &[DiffLine], expanded: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let added = lines.iter().filter(|l| l.kind == DiffLineKind::Added).count();
        let removed = lines.iter().filter(|l| l.kind == DiffLineKind::Removed).count();
        let shown = if expanded { lines.len() } else { lines.len().min(DIFF_PREVIEW_LINES) };
        let reviewable = self.changes.iter().any(|c| c.path == path || path.canonicalize().is_ok_and(|p| p == c.path));
        let review_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut body = v_flex().font_family(theme.mono_font_family.clone());
        for line in &lines[..shown] {
            let (sign, bg, fg) = match line.kind {
                DiffLineKind::Added => ("+", theme.success.opacity(0.14), theme.foreground),
                DiffLineKind::Removed => ("−", theme.danger.opacity(0.14), theme.foreground),
                DiffLineKind::Gap => ("", theme.transparent, theme.muted_foreground),
            };
            body = body.child(
                h_flex()
                    .bg(bg)
                    .text_color(fg)
                    .child(div().w(px(36.)).flex_none().pr_1().text_right().text_color(theme.muted_foreground).child(if line.kind == DiffLineKind::Gap { "⋯".to_string() } else { line.number.to_string() }))
                    .child(div().w(px(12.)).flex_none().child(sign))
                    .child(div().flex_1().min_w_0().truncate().child(line.text.clone())),
            );
        }
        v_flex()
            .mx_2()
            .mb_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .child(
                h_flex()
                    .px_1p5()
                    .py_0p5()
                    .gap_1p5()
                    .bg(theme.muted)
                    .child(div().flex_1().min_w_0().truncate().child(file_name(path)))
                    .when(created, |el| el.child(div().text_color(theme.muted_foreground).child("new")))
                    .child(div().text_color(theme.success).child(format!("+{added}")))
                    .child(div().text_color(theme.danger).child(format!("−{removed}")))
                    .when(reviewable, |el| {
                        el.child(Button::new(("agent-diff-review", ix * 16 + n)).label("Review").xsmall().ghost().on_click(cx.listener(move |this, _, window, cx| {
                            this.review(&review_path, window, cx)
                        })))
                    }),
            )
            .child(body)
            .when(shown < lines.len(), |el| el.child(self.show_all(ix, lines.len() - shown, "line", cx)))
    }

    fn show_all(&self, ix: usize, hidden: usize, what: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .id(("agent-show-all", ix))
            .px_2()
            .pb_1()
            .text_color(theme.link)
            .cursor_pointer()
            .child(format!("Show {hidden} more {what}{}", if hidden == 1 { "" } else { "s" }))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(ix, cx)))
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let panel = cx.entity().downgrade();
        let agents = self.agents.clone();
        let current = self.agent.id.clone();
        let agent_menu = Button::new("agent-select")
            .label(self.agent.name.clone())
            .icon(IconName::Bot)
            .ghost()
            .small()
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.label("Agents");
                for agent in &agents {
                    let (panel, id) = (panel.clone(), agent.id.clone());
                    menu = menu.item(PopupMenuItem::new(agent.name.clone()).checked(agent.id == current).on_click(move |_, window, cx| {
                        panel.update(cx, |this, cx| this.select_agent(&id, window, cx)).ok();
                    }));
                }
                menu
            });
        let mode_menu = self.thread.modes.clone().map(|modes| {
            let panel = cx.entity().downgrade();
            let current = modes.current_mode_id.0.to_string();
            let label = modes.available_modes.iter().find(|m| m.id.0.as_ref() == current).map(|m| m.name.clone()).unwrap_or(current.clone());
            Button::new("agent-mode").label(label).ghost().xsmall().dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                for mode in &modes.available_modes {
                    let (panel, id) = (panel.clone(), mode.id.0.to_string());
                    menu = menu.item(PopupMenuItem::new(mode.name.clone()).checked(mode.id.0.as_ref() == current).on_click(move |_, _, cx| {
                        panel.update(cx, |this, cx| this.set_mode(id.clone(), cx)).ok();
                    }));
                }
                menu
            })
        });
        h_flex()
            .h(px(32.))
            .flex_none()
            .px_1()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(agent_menu)
            .children(mode_menu)
            .child(div().flex_1())
            .child(
                Button::new("agent-follow")
                    .icon(IconName::Crosshair)
                    .ghost()
                    .xsmall()
                    .selected(self.follow)
                    .tooltip("Follow Agent: show the files it reads and changes")
                    .on_click(cx.listener(|this, _, _, cx| this.set_follow(!this.follow, cx))),
            )
            .child(
                Button::new("agent-new-thread")
                    .icon(IconName::Plus)
                    .ghost()
                    .xsmall()
                    .tooltip("New Thread")
                    .on_click(cx.listener(|this, _, window, cx| this.new_thread(window, cx))),
            )
    }

    fn render_plan(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.thread.plan.is_empty() {
            return None;
        }
        let theme = cx.theme().clone();
        let done = self.thread.plan.iter().filter(|e| e.status == PlanEntryStatus::Completed).count();
        let mut list = v_flex()
            .px_3()
            .py_1()
            .gap_0p5()
            .text_xs()
            .border_t_1()
            .border_color(theme.border)
            .child(h_flex().gap_1().text_color(theme.muted_foreground).child(Icon::new(IconName::ListChecks).xsmall()).child(format!("Plan · {done} of {}", self.thread.plan.len())));
        for entry in &self.thread.plan {
            let (icon, color) = match entry.status {
                PlanEntryStatus::Completed => (IconName::CircleCheck, theme.success),
                PlanEntryStatus::InProgress => (IconName::Loader, theme.foreground),
                _ => (IconName::Circle, theme.muted_foreground),
            };
            list = list.child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(icon).xsmall().text_color(color))
                    .child(div().flex_1().min_w_0().truncate().when(entry.status == PlanEntryStatus::Completed, |el| el.line_through().text_color(theme.muted_foreground)).child(entry.content.clone())),
            );
        }
        Some(list)
    }

    fn render_changes(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.changes.is_empty() {
            return None;
        }
        let theme = cx.theme().clone();
        let count = self.changes.len();
        let mut list = v_flex().px_2().py_1().gap_0p5().text_xs().border_t_1().border_color(theme.border).child(
            h_flex()
                .gap_1()
                .child(div().flex_1().font_weight(FontWeight::MEDIUM).child(if count == 1 { "1 file changed".to_string() } else { format!("{count} files changed") }))
                .child(Button::new("agent-keep-all").label("Keep All").xsmall().ghost().on_click(cx.listener(|this, _, _, cx| this.keep(None, cx))))
                .child(Button::new("agent-reject-all").label("Reject All").xsmall().ghost().on_click(cx.listener(|this, _, window, cx| {
                    for path in this.changes.iter().map(|c| c.path.clone()).collect::<Vec<PathBuf>>() {
                        this.reject(&path, window, cx);
                    }
                }))),
        );
        for (n, change) in self.changes.iter().enumerate() {
            let (review, reject) = (change.path.clone(), change.path.clone());
            list = list.child(
                h_flex()
                    .gap_1()
                    .child(Icon::new(crate::workspace::pickers::file_icon(&change.path)).xsmall().text_color(theme.muted_foreground))
                    .child(
                        div()
                            .id(("agent-change", n))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .cursor_pointer()
                            .child(file_name(&change.path))
                            .on_click(cx.listener(move |this, _, window, cx| this.review(&review, window, cx))),
                    )
                    .when(change.before.is_none(), |el| el.child(div().text_color(theme.muted_foreground).child("new")))
                    .child(div().text_color(theme.success).child(format!("+{}", change.added)))
                    .child(div().text_color(theme.danger).child(format!("−{}", change.removed)))
                    .child(Button::new(("agent-change-reject", n)).label("Reject").xsmall().ghost().on_click(cx.listener(move |this, _, window, cx| this.reject(&reject, window, cx)))),
            );
        }
        Some(list)
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let line = |content: AnyElement| h_flex().px_3().py_1().gap_1p5().text_xs().border_t_1().border_color(theme.border).child(content);
        match &self.status {
            Status::Idle => None,
            Status::Starting => Some(line(h_flex().gap_1p5().child(Spinner::new().xsmall()).child(format!("Starting {}…", self.agent.name)).into_any_element()).into_any_element()),
            Status::Generating => {
                let waiting = self.thread.entries.iter().any(|e| matches!(e, Entry::Tool(call) if call.permission.as_ref().is_some_and(|p| p.pending())));
                let text = if waiting { format!("{} is waiting for you", self.agent.name) } else { format!("{} is working…", self.agent.name) };
                Some(line(h_flex().gap_1p5().text_color(theme.muted_foreground).child(Spinner::new().xsmall()).child(text).into_any_element()).into_any_element())
            }
            Status::Error(message) => Some(
                line(h_flex().gap_1p5().text_color(theme.danger).child(Icon::new(IconName::TriangleAlert).xsmall()).child(div().flex_1().whitespace_normal().child(message.clone())).into_any_element())
                    .into_any_element(),
            ),
            Status::AuthRequired(methods) => {
                let mut buttons = h_flex().gap_1().flex_wrap();
                for (n, (id, name, description)) in methods.iter().enumerate() {
                    let id = id.clone();
                    let mut button = Button::new(("agent-auth", n)).label(name.clone()).xsmall();
                    if let Some(description) = description {
                        button = button.tooltip(description.clone());
                    }
                    buttons = buttons.child(button.on_click(cx.listener(move |this, _, window, cx| this.authenticate(id.clone(), window, cx))));
                }
                let workspace = self.workspace.clone();
                buttons = buttons.child(Button::new("agent-auth-terminal").label("Open Terminal").xsmall().ghost().on_click(move |_, window, cx| {
                    workspace
                        .update(cx, |ws, cx| {
                            let dir = ws.project.root.clone();
                            ws.new_terminal_tab(crate::terminal::Spawn::shell(dir), window, cx)
                        })
                        .ok();
                }));
                Some(
                    v_flex()
                        .px_3()
                        .py_1()
                        .gap_1()
                        .text_xs()
                        .border_t_1()
                        .border_color(theme.border)
                        .child(format!("{} needs you to sign in. Sign in below, or in a terminal (for Claude Code: claude /login), then send again.", self.agent.name))
                        .child(buttons)
                        .into_any_element(),
                )
            }
        }
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string())
}

impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let panel = cx.entity().downgrade();
        let body: AnyElement = if self.thread.entries.is_empty() {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .p_6()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(Icon::new(IconName::Bot).large())
                .child(div().text_center().child(format!("Ask {} about this project, or to change it.", self.agent.name)))
                .child(div().text_xs().text_center().child("@ adds a file or symbol. Its changes land in the editor, where ⌘Z undoes them."))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .child(
                    MessageScroller::new("agent-thread", self.scroller.clone(), move |ix, window, cx| {
                        panel.upgrade().map(|panel| panel.update(cx, |this, cx| this.render_entry(ix, window, cx))).unwrap_or_else(|| div().into_any_element())
                    })
                    .size_full()
                    .scrollbar(true),
                )
                .into_any_element()
        };
        let busy = self.busy();
        let can_send = !busy && !matches!(self.status, Status::AuthRequired(_));
        let _ = window;
        v_flex()
            .id("agent-panel")
            .key_context("AgentPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .border_l_1()
            .border_color(theme.border)
            .child(self.render_header(cx))
            .child(body)
            .children(self.render_plan(cx))
            .children(self.render_changes(cx))
            .children(self.render_status(cx))
            .child(
                v_flex()
                    .flex_none()
                    .p_2()
                    .gap_1()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(Textarea::new(&self.composer).text_sm())
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("agent-mention")
                                    .icon(IconName::AtSign)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Mention a File or Symbol (@)")
                                    .on_click(cx.listener(|this, _, window, cx| this.show_mentions(None, window, cx))),
                            )
                            .child(div().flex_1())
                            .map(|el| {
                                if busy {
                                    el.child(Button::new("agent-stop").icon(IconName::Square).label("Stop").xsmall().on_click(cx.listener(|this, _, _, cx| this.cancel(cx))))
                                } else {
                                    el.child(
                                        Button::new("agent-send")
                                            .icon(IconName::SendHorizontal)
                                            .xsmall()
                                            .primary()
                                            .disabled(!can_send)
                                            .tooltip("Send (Return; ⇧Return for a new line)")
                                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                                    )
                                }
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{DiffLine, DiffLineKind, diff_lines};

    #[test]
    fn diff_lines_show_each_change_with_its_line_numbers() {
        let lines = diff_lines("a\nb\nc\nd\ne\n", "a\nB\nc\nd\ne\nf\n");
        let shown: Vec<(DiffLineKind, u32, &str)> = lines.iter().map(|l| (l.kind.clone(), l.number, l.text.as_str())).collect();
        assert_eq!(
            shown,
            [
                (DiffLineKind::Removed, 2, "b"),
                (DiffLineKind::Added, 2, "B"),
                (DiffLineKind::Gap, 6, ""),
                (DiffLineKind::Added, 6, "f"),
            ]
        );
        assert_eq!(diff_lines("", "new\n"), [DiffLine { kind: DiffLineKind::Added, number: 1, text: "new".into() }]);
    }
}

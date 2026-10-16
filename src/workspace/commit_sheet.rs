//! Integrate ▸ Commit… (⌥⌘C): the changed files with a checkbox each for
//! whether it's staged, the selected file's changes with Stage / Unstage on
//! each one (staging a hunk writes the index's text, no patch), the
//! message, and Amend, which replaces the last commit and starts from its
//! message. The draft message is kept when the sheet closes.

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::Workspace;
use super::hunk_popover::hunk_lines;
use crate::actions::PickerDismiss;
use crate::git::diff::{self, Hunk, Staging};
use crate::git::{self, FileStatus};

const MAX_HUNK_LINES: usize = 40;

#[derive(Clone)]
struct CommitFile {
    path: PathBuf,
    rel: String,
    letter: &'static str,
    staging: Staging,
}

#[derive(Clone)]
struct HunkView {
    hunk: Hunk,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
}

/// The selected file's changes.
#[derive(Default)]
struct Detail {
    rel: String,
    /// The index against the work tree: what Stage would add.
    unstaged: Vec<HunkView>,
    /// `HEAD` against the index: what is staged.
    staged: Vec<HunkView>,
    head: Option<String>,
    index: Option<String>,
    worktree: Option<String>,
    mode: Option<String>,
}

pub struct CommitSheet {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    root: PathBuf,
    branch: Option<String>,
    files: Vec<CommitFile>,
    selected: Option<String>,
    detail: Option<Detail>,
    pub message: Entity<TextareaState>,
    amend: bool,
    /// The message before Amend filled in the last one.
    draft: Option<String>,
    _task: Option<Task<()>>,
    /// Committed: the draft is used up.
    committed: bool,
    _amend_task: Option<Task<()>>,
}

impl EventEmitter<DismissEvent> for CommitSheet {}

impl Focusable for CommitSheet {
    /// The message, ready for typing.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.message.read(cx).focus_handle(cx)
    }
}

impl CommitSheet {
    fn new(workspace: &Workspace, weak: WeakEntity<Workspace>, draft: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| TextareaState::new(window, cx).placeholder("Enter commit message here").rows(5).default_value(draft));
        let mut sheet = Self {
            focus_handle: cx.focus_handle(),
            workspace: weak,
            root: workspace.git.root.clone().unwrap_or_default(),
            branch: workspace.git.status.branch.clone(),
            files: vec![],
            selected: None,
            detail: None,
            message,
            amend: false,
            draft: None,
            _task: None,
            committed: false,
            _amend_task: None,
        };
        sheet.reload(workspace, cx);
        sheet
    }

    /// The files from the workspace's status, and the selected one's changes.
    pub fn reload(&mut self, workspace: &Workspace, cx: &mut Context<Self>) {
        self.branch = workspace.git.status.branch.clone();
        let root = self.root.clone();
        self.files = workspace
            .git
            .status
            .files
            .iter()
            .map(|f: &FileStatus| CommitFile {
                rel: git::relative(&root, &f.path).unwrap_or_default(),
                path: f.path.clone(),
                letter: f.letter(),
                staging: match (f.is_staged(), f.is_unstaged()) {
                    (true, false) => Staging::Staged,
                    (true, true) => Staging::Partial,
                    _ => Staging::Unstaged,
                },
            })
            .collect();
        if self.selected.as_ref().is_none_or(|rel| !self.files.iter().any(|f| &f.rel == rel)) {
            self.selected = self.files.first().map(|f| f.rel.clone());
        }
        self.load_detail(cx);
        cx.notify();
    }

    fn load_detail(&mut self, cx: &mut Context<Self>) {
        let Some(rel) = self.selected.clone() else {
            self.detail = None;
            return;
        };
        let root = self.root.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            let detail = cx.background_spawn(async move { read_detail(&root, rel) }).await;
            this.update(cx, |this, cx| {
                this.detail = Some(detail);
                cx.notify();
            })
            .ok();
        }));
    }

    fn dismiss(&mut self, _: &PickerDismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    /// The message as typed (without Amend's); nothing once committed.
    pub fn draft(&self, cx: &App) -> String {
        if self.committed {
            String::new()
        } else if self.amend {
            self.draft.clone().unwrap_or_default()
        } else {
            self.message.read(cx).value().to_string()
        }
    }

    fn staged_count(&self) -> usize {
        self.files.iter().filter(|f| f.staging != Staging::Unstaged).count()
    }

    fn can_commit(&self, cx: &App) -> bool {
        !self.message.read(cx).value().trim().is_empty() && (self.amend || self.staged_count() > 0)
    }

    /// The sheet's controls, for its buttons and the control channel's
    /// `git commit <command>`: `select <path>`, `toggle-file <path>`,
    /// `stage-hunk <n>`, `unstage-hunk <n>`, `message <text>` (`\n` for new
    /// lines), `amend on|off`, `commit`, `cancel`.
    pub fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
        let workspace = self.workspace.clone();
        match verb {
            "select" => {
                self.selected = Some(arg.to_string());
                self.detail = None;
                self.load_detail(cx);
            }
            "toggle-file" => {
                let file = self.files.iter().find(|f| f.rel == arg).cloned().ok_or(format!("no changed file {arg}"))?;
                let stage = file.staging != Staging::Staged;
                window.defer(cx, move |window, cx| {
                    workspace.update(cx, |ws, cx| ws.stage_paths(vec![file.path], stage, window, cx)).ok();
                });
            }
            "stage-hunk" | "unstage-hunk" => {
                let n: usize = arg.parse().map_err(|_| format!("usage: {verb} <n>"))?;
                let detail = self.detail.as_ref().ok_or("no file selected")?;
                let (rel, mode) = (detail.rel.clone(), detail.mode.clone());
                let index = detail.index.clone().unwrap_or_default();
                let new_index = if verb == "stage-hunk" {
                    let hunk = detail.unstaged.get(n).ok_or(format!("no unstaged change {n}"))?;
                    diff::apply(&index, detail.worktree.as_deref().unwrap_or_default(), &[&hunk.hunk])
                } else {
                    let hunk = detail.staged.get(n).ok_or(format!("no staged change {n}"))?;
                    diff::apply(&index, detail.head.as_deref().unwrap_or_default(), &[&hunk.hunk.inverted()])
                };
                // A new file with nothing staged leaves the index.
                let remove = detail.head.is_none() && new_index.is_empty() && verb == "unstage-hunk";
                let label = if verb == "stage-hunk" { "Staging" } else { "Unstaging" };
                window.defer(cx, move |window, cx| {
                    workspace
                        .update(cx, |ws, cx| {
                            ws.run_git(
                                label,
                                move |root| git::write_index(root, &rel, (!remove).then_some(new_index.as_str()), mode.as_deref()),
                                |_, _, _, _| {},
                                window,
                                cx,
                            )
                        })
                        .ok();
                });
            }
            "message" => {
                let text = arg.replace("\\n", "\n");
                self.message.update(cx, |input, cx| input.set_value(text, window, cx));
            }
            "amend" => self.set_amend(arg == "on", window, cx),
            "commit" => {
                if !self.can_commit(cx) {
                    return Err("nothing to commit, or no message".into());
                }
                let message = self.message.read(cx).value().to_string();
                let amend = self.amend;
                self.committed = true;
                cx.emit(DismissEvent);
                window.defer(cx, move |window, cx| {
                    workspace
                        .update(cx, |ws, cx| {
                            ws.run_git(
                                if amend { "Amending" } else { "Committing" },
                                move |root| git::commit(root, &message, amend),
                                |ws, id, window, cx| {
                                    ws.git.last_commit = Some(id.clone());
                                    gpui_kit::component::WindowExt::push_notification(
                                        window,
                                        gpui_kit::component::notification::Notification::info(format!("Committed {id}")),
                                        cx,
                                    );
                                },
                                window,
                                cx,
                            )
                        })
                        .ok();
                });
            }
            "cancel" => cx.emit(DismissEvent),
            other => return Err(format!("unknown commit command {other}")),
        }
        cx.notify();
        Ok(())
    }

    fn set_amend(&mut self, amend: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.amend == amend {
            return;
        }
        self.amend = amend;
        if amend {
            self.draft = Some(self.message.read(cx).value().to_string());
            let root = self.root.clone();
            self._amend_task = Some(cx.spawn_in(window, async move |this, cx| {
                let last = cx.background_spawn(async move { git::last_message(&root).unwrap_or_default() }).await;
                this.update_in(cx, |this, window, cx| {
                    if this.amend {
                        this.message.update(cx, |input, cx| input.set_value(last, window, cx));
                    }
                })
                .ok();
            }));
        } else {
            let draft = self.draft.take().unwrap_or_default();
            self.message.update(cx, |input, cx| input.set_value(draft, window, cx));
        }
        cx.notify();
    }

    pub fn summary(&self, cx: &App) -> serde_json::Value {
        let hunks = |list: &[HunkView]| list.iter().map(|h| serde_json::json!({ "old": h.old_lines, "new": h.new_lines })).collect::<Vec<_>>();
        serde_json::json!({
            "files": self.files.iter().map(|f| format!("{:?} {} {}", f.staging, f.letter, f.rel)).collect::<Vec<_>>(),
            "selected": self.selected,
            "unstaged": self.detail.as_ref().map(|d| hunks(&d.unstaged)),
            "staged": self.detail.as_ref().map(|d| hunks(&d.staged)),
            "message": self.message.read(cx).value().to_string(),
            "amend": self.amend,
            "can_commit": self.can_commit(cx),
        })
    }
}

/// A file's versions and its changes in the index and the work tree.
fn read_detail(root: &std::path::Path, rel: String) -> Detail {
    let bases = git::bases(root, std::slice::from_ref(&rel), &Default::default()).pop().unwrap_or_default();
    let worktree = std::fs::read(root.join(&rel)).ok().and_then(|b| String::from_utf8(b).ok());
    let views = |old: &str, new: &str| -> Vec<HunkView> {
        let (old_lines, new_lines) = (diff::lines(old), diff::lines(new));
        let text = |lines: &[&str], rows: &std::ops::Range<u32>| -> Vec<String> {
            diff::slice(lines, rows).iter().map(|l| l.trim_end_matches('\n').to_string()).collect()
        };
        diff::line_hunks(old, new)
            .into_iter()
            .map(|hunk| HunkView { old_lines: text(&old_lines, &hunk.old), new_lines: text(&new_lines, &hunk.new), hunk })
            .collect()
    };
    let (head, index) = (bases.head.clone().unwrap_or_default(), bases.index.clone().unwrap_or_default());
    Detail {
        unstaged: views(&index, worktree.as_deref().unwrap_or_default()),
        staged: if bases.index.is_none() && bases.head.is_some() { views(&head, "") } else { views(&head, &index) },
        rel,
        head: bases.head,
        index: bases.index,
        worktree,
        mode: bases.mode,
    }
}

impl Render for CommitSheet {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = self.staged_count();
        let can_commit = self.can_commit(cx);
        let files = v_flex().id("commit-files").w(px(280.)).flex_none().h_full().overflow_y_scroll().border_r_1().border_color(theme.border).children(
            self.files.iter().enumerate().map(|(ix, file)| {
                let selected = self.selected.as_deref() == Some(file.rel.as_str());
                let (rel, rel2) = (file.rel.clone(), file.rel.clone());
                h_flex()
                    .id(("commit-file", ix))
                    .px_2()
                    .py(px(3.))
                    .gap_1p5()
                    .text_sm()
                    .when(selected, |el| el.bg(theme.accent))
                    .hover(|el| el.bg(theme.secondary_hover))
                    .child(
                        Checkbox::new(("commit-check", ix))
                            .checked(file.staging != Staging::Unstaged)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.command(&format!("toggle-file {rel}"), window, cx).ok();
                            })),
                    )
                    .child(div().flex_1().truncate().child(super::pane::file_title(&file.path)))
                    .when(file.staging == Staging::Partial, |el| el.child(div().text_xs().text_color(theme.muted_foreground).child("partly")))
                    .child(div().w(px(12.)).text_xs().font_weight(FontWeight::BOLD).text_color(theme.muted_foreground).child(file.letter))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.command(&format!("select {rel2}"), window, cx).ok();
                    }))
            }),
        );
        let mut changes = v_flex().id("commit-changes").flex_1().min_w_0().h_full().overflow_y_scroll().font_family(theme.mono_font_family.clone()).text_xs();
        match &self.detail {
            None => {}
            Some(detail) if detail.unstaged.is_empty() && detail.staged.is_empty() => {
                changes = changes.child(div().p_4().text_color(theme.muted_foreground).child("No changes to show (a binary file, or a mode change)."));
            }
            Some(detail) => {
                for (title, list, staged) in [("Staged", &detail.staged, true), ("Not Staged", &detail.unstaged, false)] {
                    if list.is_empty() {
                        continue;
                    }
                    changes = changes.child(div().px_2().pt_2().pb_1().font_weight(FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child(title));
                    for (n, hunk) in list.iter().enumerate() {
                        let command = format!("{} {n}", if staged { "unstage-hunk" } else { "stage-hunk" });
                        changes = changes.child(
                            v_flex()
                                .mx_2()
                                .mb_2()
                                .border_1()
                                .border_color(theme.border)
                                .rounded(theme.radius)
                                .child(
                                    h_flex()
                                        .px_2()
                                        .py_0p5()
                                        .bg(theme.muted)
                                        .child(div().flex_1().text_color(theme.muted_foreground).child(format!(
                                            "@@ −{},{} +{},{} @@",
                                            hunk.hunk.old.start + 1,
                                            hunk.hunk.old.len(),
                                            hunk.hunk.new.start + 1,
                                            hunk.hunk.new.len()
                                        )))
                                        .child(
                                            Button::new(SharedString::from(format!("commit-hunk-{staged}-{n}")))
                                                .label(if staged { "Unstage" } else { "Stage" })
                                                .xsmall()
                                                .ghost()
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    this.command(&command, window, cx).ok();
                                                })),
                                        ),
                                )
                                .child(hunk_lines(&hunk.old_lines, &hunk.new_lines, MAX_HUNK_LINES, cx)),
                        );
                    }
                }
            }
        }
        if self.files.is_empty() {
            changes = changes.child(div().p_4().text_color(theme.muted_foreground).child("No local changes."));
        }
        let amend = self.amend;
        v_flex()
            .id("commit-sheet")
            .key_context("CommitSheet")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::dismiss))
            .w(px(920.))
            .h(px(620.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .text_sm()
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Commit"))
                    .when_some(self.branch.clone(), |el, branch| el.child(div().text_xs().text_color(theme.muted_foreground).child(format!("on {branch}")))),
            )
            .child(h_flex().flex_1().min_h_0().child(files).child(changes))
            .child(
                v_flex()
                    .p_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(Textarea::new(&self.message).h(px(96.)))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Checkbox::new("commit-amend").label("Amend").checked(amend).on_click(cx.listener(|this, checked: &bool, window, cx| {
                                this.set_amend(*checked, window, cx);
                            })))
                            .child(div().flex_1())
                            .child(Button::new("commit-cancel").label("Cancel").small().on_click(cx.listener(|this, _, window, cx| {
                                this.command("cancel", window, cx).ok();
                            })))
                            .child(
                                Button::new("commit-commit")
                                    .label(match (amend, count) {
                                        (true, _) => "Amend Last Commit".to_string(),
                                        (false, 1) => "Commit 1 File".to_string(),
                                        (false, n) => format!("Commit {n} Files"),
                                    })
                                    .small()
                                    .primary()
                                    .disabled(!can_commit)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.command("commit", window, cx).ok();
                                    })),
                            ),
                    ),
            )
    }
}

impl Workspace {
    /// Integrate ▸ Commit…: saves open files first, as Xcode does.
    pub fn show_commit_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git.root.is_none() {
            self.notify_error("The project isn’t in a Git repository.".into(), window, cx);
            return;
        }
        self.save_all(window, cx);
        let draft = std::mem::take(&mut self.git.commit_draft);
        let weak = cx.weak_entity();
        let this = &*self;
        let sheet = cx.new(|cx| CommitSheet::new(this, weak, draft, window, cx));
        self.git.commit_sheet = Some(sheet.downgrade());
        self.show_modal(sheet, window, cx);
        // The saves changed the status.
        self.refresh_scm(cx);
    }

    /// The status was read again: the commit sheet follows it.
    pub(super) fn update_commit_sheet(&mut self, cx: &mut Context<Self>) {
        let Some(sheet) = self.git.commit_sheet.as_ref().and_then(|s| s.upgrade()) else { return };
        let this = &*self;
        sheet.update(cx, |sheet, cx| sheet.reload(this, cx));
    }
}

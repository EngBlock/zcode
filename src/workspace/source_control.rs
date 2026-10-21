//! Source control: the repository the project is in, its status (the
//! Source Control navigator, the status bar's branch) and the versions of
//! open files the gutter compares with, all read in the background, and the
//! Integrate menu: Commit…, Push…, Pull…, Fetch Changes, branches, Stage,
//! Unstage and Discard.
//!
//! Status is refreshed after zcode's own saves and git operations (with a
//! short debounce), when the window becomes active, and when the git
//! directory changes (a commit or checkout in a terminal), which a cheap
//! poll of a few files' modification times notices.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::navigator::{empty, row, section_header};
use super::pickers::file_icon;
use super::Workspace;
use crate::git::{self, Bases, Branch, FileStatus, Status};

/// How long after a save the status is read again.
const SAVE_REFRESH_DELAY: Duration = Duration::from_millis(300);
/// How often the git directory is checked for outside changes while
/// FSEvents doesn't watch it.
const GIT_DIR_POLL: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct GitState {
    /// The top of the work tree the project is in.
    pub root: Option<PathBuf>,
    git_dir: Option<PathBuf>,
    /// Whether the project has been looked at (until then the navigator
    /// doesn't say it isn't a repository).
    pub discovered: bool,
    pub status: Status,
    pub branches: Vec<Branch>,
    pub remotes: Vec<String>,
    pub user_email: Option<String>,
    /// The git operation running ("Pushing…"); others wait for it.
    pub busy: Option<SharedString>,
    pub last_error: Option<String>,
    pub last_commit: Option<String>,
    /// Inline blame on the cursor's line (Zed) and Editor ▸ Authors.
    pub inline_blame: bool,
    pub show_authors: bool,
    pub(super) inline: super::blame::InlineBlame,
    /// Code Review side by side (rather than inline).
    pub side_by_side: bool,
    /// The commit message being written, kept while the sheet is closed.
    pub commit_draft: String,
    pub(super) popover: Option<WeakEntity<super::hunk_popover::HunkPopover>>,
    pub(super) commit_sheet: Option<WeakEntity<super::commit_sheet::CommitSheet>>,
    pub(super) remote_sheet: Option<WeakEntity<super::remote_sheet::RemoteSheet>>,
    refresh: Option<Task<()>>,
    watching: bool,
}

impl GitState {
    pub fn new(settings: &crate::settings::Settings) -> Self {
        Self {
            inline_blame: settings.inline_blame,
            show_authors: settings.show_authors,
            side_by_side: settings.side_by_side_comparison,
            ..Default::default()
        }
    }
}

struct Refreshed {
    root: PathBuf,
    git_dir: Option<PathBuf>,
    status: Status,
    branches: Vec<Branch>,
    remotes: Vec<String>,
    email: Option<String>,
    bases: Vec<(PathBuf, Bases)>,
}

impl Workspace {
    /// The repository's root and `path` relative to it, when `path` is in it
    /// (and isn't a package checkout, which is a repository of its own).
    pub(super) fn git_rel(&self, path: &Path) -> Option<(PathBuf, String)> {
        let root = self.git.root.clone()?;
        if crate::packages::location::is_package_checkout(path) {
            return None;
        }
        let rel = git::relative(&root, path)?;
        Some((root, rel))
    }

    pub fn refresh_scm(&mut self, cx: &mut Context<Self>) {
        self.schedule_scm_refresh(Duration::ZERO, cx);
    }

    /// Reads the status (and the open files' versions) after `delay`; a
    /// newer request replaces a pending one.
    pub(super) fn schedule_scm_refresh(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let project_root = self.project.root.clone();
        let known = self.git.discovered.then(|| self.git.root.clone());
        let first = self.git.user_email.is_none();
        let files: Vec<(PathBuf, Option<Bases>)> = self
            .buffers
            .iter()
            .filter(|b| !b.read_only)
            .map(|b| (b.path.clone(), b.git.bases.clone()))
            .collect();
        self.git.refresh = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let refreshed = cx
                .background_spawn(async move {
                    let root = match known {
                        Some(root) => root?,
                        None => git::discover(&project_root)?,
                    };
                    let status = git::status(&root).ok()?;
                    let bases = read_bases(&root, files);
                    Some(Refreshed {
                        git_dir: git::git_dir(&root),
                        branches: git::branches(&root).unwrap_or_default(),
                        remotes: git::remotes(&root),
                        email: if first { git::user_email(&root) } else { None },
                        status,
                        bases,
                        root,
                    })
                })
                .await;
            this.update(cx, |this, cx| this.scm_refreshed(refreshed, cx)).ok();
        }));
    }

    fn scm_refreshed(&mut self, refreshed: Option<Refreshed>, cx: &mut Context<Self>) {
        self.git.discovered = true;
        let Some(r) = refreshed else {
            if self.git.root.is_some() {
                // The repository went away (or git failed): say so, keep trying.
                self.git.status = Status::default();
            }
            cx.notify();
            return;
        };
        let moved = self.git.status.head != r.status.head || self.git.status.branch != r.status.branch;
        let was_untracked: Vec<PathBuf> = self.untracked_paths();
        self.git.root = Some(r.root);
        self.git.git_dir = r.git_dir;
        self.git.status = r.status;
        self.git.branches = r.branches;
        self.git.remotes = r.remotes;
        if r.email.is_some() {
            self.git.user_email = r.email;
        }
        if !self.git.watching {
            self.git.watching = true;
            self.watch_git(cx);
        }
        let untracked = self.untracked_paths();
        for (path, bases) in r.bases {
            let changed_tracking = was_untracked.contains(&path) != untracked.contains(&path);
            self.set_git_bases(&path, bases, changed_tracking, cx);
        }
        // Buffers opened before the repository was found.
        let missing: Vec<PathBuf> =
            self.buffers.iter().filter(|b| !b.read_only && b.git.bases.is_none()).map(|b| b.path.clone()).collect();
        if !missing.is_empty() {
            self.load_git_bases(missing, cx);
        }
        if moved {
            self.invalidate_blame(cx);
        }
        self.update_commit_sheet(cx);
        cx.notify();
    }

    fn untracked_paths(&self) -> Vec<PathBuf> {
        self.git.status.files.iter().filter(|f| f.index == git::Change::Untracked).map(|f| f.path.clone()).collect()
    }

    /// Whether git doesn't track `path` (every line of it is added).
    pub(super) fn is_untracked(&self, path: &Path) -> bool {
        self.git.status.file(path).is_some_and(|f| f.index == git::Change::Untracked)
    }

    /// Reads the index and `HEAD` versions of `paths` in the background.
    pub(super) fn load_git_bases(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(root) = self.git.root.clone() else { return };
        let files: Vec<(PathBuf, Option<Bases>)> = paths
            .into_iter()
            .filter(|p| self.git_rel(p).is_some())
            .map(|p| {
                let known = self.buffer(&p).and_then(|b| b.git.bases.clone());
                (p, known)
            })
            .collect();
        if files.is_empty() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let bases = cx.background_spawn(async move { read_bases(&root, files) }).await;
            this.update(cx, |this, cx| {
                for (path, bases) in bases {
                    this.set_git_bases(&path, bases, false, cx);
                }
            })
            .ok();
        });
        self._tasks.push(task);
    }

    /// Follows the git directory for changes made outside zcode: looked at
    /// when FSEvents says something in it changed (`fs_watch`), else polled.
    fn watch_git(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            let mut last: Option<Vec<Option<SystemTime>>> = None;
            let changes = crate::fs_watch::Changes::new(vec![]);
            loop {
                changes.wait(GIT_DIR_POLL, cx.background_executor()).await;
                let Ok(dir) = this.update(cx, |this, _| this.git.git_dir.clone()) else { break };
                let Some(dir) = dir else { continue };
                changes.set_paths(vec![dir.clone()]);
                let stamps = cx
                    .background_spawn(async move {
                        ["HEAD", "index", "logs/HEAD", "FETCH_HEAD", "packed-refs"]
                            .iter()
                            .map(|name| std::fs::metadata(dir.join(name)).and_then(|m| m.modified()).ok())
                            .collect::<Vec<_>>()
                    })
                    .await;
                if last.as_ref().is_some_and(|last| *last != stamps)
                    && this
                        .update(cx, |this, cx| {
                            if this.git.busy.is_none() {
                                this.refresh_scm(cx);
                            }
                        })
                        .is_err()
                {
                    break;
                }
                last = Some(stamps);
            }
        });
        self._tasks.push(task);
    }

    /// After a save or a reload: the status, a moment later.
    pub(super) fn file_changed_on_disk(&mut self, cx: &mut Context<Self>) {
        if self.git.root.is_some() {
            self.schedule_scm_refresh(SAVE_REFRESH_DELAY, cx);
        }
    }

    // ---------------------------------------------------------------------
    // Operations

    /// Runs a git operation in the background (one at a time), then reads
    /// the status again and calls `done` with its result.
    pub(super) fn run_git<T: Send + 'static>(
        &mut self,
        label: &str,
        op: impl FnOnce(&Path) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.git.root.clone() else {
            self.notify_error("The project isn’t in a Git repository.".into(), window, cx);
            return;
        };
        if let Some(busy) = &self.git.busy {
            self.notify_error(format!("Wait for “{busy}” to finish."), window, cx);
            return;
        }
        self.git.busy = Some(SharedString::from(label.to_string()));
        self.git.last_error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { op(&root) }).await;
            this.update_in(cx, |this, window, cx| {
                this.git.busy = None;
                match result {
                    Ok(value) => done(this, value, window, cx),
                    Err(err) => {
                        let message = format!("{err:#}");
                        this.git.last_error = Some(message.clone());
                        this.notify_error(message, window, cx);
                    }
                }
                this.reload_changed_buffers(cx);
                this.refresh_scm(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// After a checkout, pull or discard: open files without edits take
    /// what's on disk now, and the navigator what's in the folder.
    fn reload_changed_buffers(&mut self, cx: &mut Context<Self>) {
        let changed: Vec<PathBuf> = self
            .buffers
            .iter()
            .filter(|b| !b.dirty && std::fs::metadata(&b.path).and_then(|m| m.modified()).ok().is_some_and(|m| Some(m) != b.mtime))
            .map(|b| b.path.clone())
            .collect();
        // Outside the window's update this may run in: reloading updates it.
        let workspace = cx.weak_entity();
        cx.defer(move |cx| {
            workspace
                .update(cx, |this, cx| {
                    for path in &changed {
                        this.reload_from_disk(path, None, cx);
                    }
                })
                .ok();
        });
        self.rescan(cx);
    }

    /// Integrate ▸ Fetch Changes.
    pub fn fetch_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_git("Fetching", |root| git::fetch(root), |_, _, _, _| {}, window, cx);
    }

    /// Stage (or unstage) whole files, from the navigator or the commit sheet.
    pub fn stage_paths(&mut self, paths: Vec<PathBuf>, stage: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.git.root.clone() else { return };
        let rels: Vec<String> = paths.iter().filter_map(|p| git::relative(&root, p)).collect();
        if rels.is_empty() {
            return;
        }
        let label = if stage { "Staging" } else { "Unstaging" };
        self.run_git(
            label,
            move |root| if stage { git::stage_files(root, &rels) } else { git::unstage_files(root, &rels) },
            |_, _, _, _| {},
            window,
            cx,
        );
    }

    /// Integrate ▸ Stage All Changes / Unstage All Changes.
    pub fn stage_all(&mut self, stage: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self
            .git
            .status
            .files
            .iter()
            .filter(|f| if stage { f.is_unstaged() } else { f.is_staged() })
            .map(|f| f.path.clone())
            .collect();
        self.stage_paths(paths, stage, window, cx);
    }

    /// Discard Changes: after asking, files go back to `HEAD`; files git
    /// doesn't track (or that are new since `HEAD`) go to the Trash.
    pub fn discard_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.git.root.clone() else { return };
        let files: Vec<FileStatus> = paths.iter().filter_map(|p| self.git.status.file(p).cloned()).collect();
        if files.is_empty() {
            return;
        }
        let message = match &files[..] {
            [file] => format!("Discard changes in “{}”?", super::pane::file_title(&file.path)),
            files => format!("Discard changes in {} files?", files.len()),
        };
        let detail = "Files are returned to their last committed versions; new files are moved to the Trash. Edits not yet saved in these files are lost.";
        let answer = crate::debug::ask(&message, detail, &["Discard Changes", "Cancel"], window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != 0 {
                return;
            }
            this.update_in(cx, |this, window, cx| this.discard_files_now(root, files, window, cx)).ok();
        })
        .detach();
    }

    fn discard_files_now(&mut self, root: PathBuf, files: Vec<FileStatus>, window: &mut Window, cx: &mut Context<Self>) {
        let new: Vec<PathBuf> = files
            .iter()
            .filter(|f| matches!(f.index, git::Change::Untracked | git::Change::Added) && f.worktree != git::Change::Deleted)
            .map(|f| f.path.clone())
            .collect();
        let rels: Vec<String> = files
            .iter()
            .filter(|f| f.index != git::Change::Untracked)
            .flat_map(|f| [Some(&f.path), f.orig.as_ref()].into_iter().flatten())
            .filter_map(|p| git::relative(&root, p))
            .collect();
        // Unsaved edits in these files go too: the files are about to change.
        for file in &files {
            if let Some(ix) = self.buffers.iter().position(|b| b.path == file.path && b.dirty) {
                let saved = self.buffers[ix].saved_text.to_string();
                let editor = self.buffers[ix].editor.clone();
                editor.update(cx, |state, cx| state.set_value(saved, window, cx));
                self.buffers[ix].dirty = false;
            }
        }
        self.run_git(
            "Discarding",
            move |root| git::discard_files(root, &rels),
            move |this, _, window, cx| {
                for path in new {
                    if path.exists() {
                        this.move_to_trash(path, window, cx);
                    }
                }
            },
            window,
            cx,
        );
    }

    /// Integrate ▸ Discard All Changes….
    pub fn discard_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.git.status.files.iter().map(|f| f.path.clone()).collect();
        self.discard_paths(paths, window, cx);
    }

    /// Checks out a branch (the branch picker, the navigator).
    pub fn switch_branch(&mut self, branch: Branch, window: &mut Window, cx: &mut Context<Self>) {
        let label = format!("Switching to {}", branch.name);
        self.run_git(&label, move |root| git::switch_branch(root, &branch), |_, _, _, _| {}, window, cx);
    }

    /// New Branch: from `HEAD`, checked out.
    pub fn create_branch(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let label = format!("Creating {name}");
        self.run_git(&label, move |root| git::create_branch(root, &name), |_, _, _, _| {}, window, cx);
    }

    /// The remote branch Push… and Pull… start from: the upstream, or the
    /// branch's name on the first remote.
    pub(super) fn default_remote_branch(&self) -> Option<String> {
        if let Some(upstream) = &self.git.status.upstream {
            return Some(upstream.clone());
        }
        let remote = self.git.remotes.iter().find(|r| *r == "origin").or(self.git.remotes.first())?;
        Some(format!("{remote}/{}", self.git.status.branch.as_deref().unwrap_or("main")))
    }

    pub fn pull(&mut self, remote_branch: String, rebase: bool, window: &mut Window, cx: &mut Context<Self>) {
        let label = format!("Pulling from {remote_branch}");
        self.run_git(&label, move |root| git::pull(root, &remote_branch, rebase), |_, _, _, _| {}, window, cx);
    }

    pub fn push(&mut self, remote_branch: String, tags: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(local) = self.git.status.branch.clone() else {
            self.notify_error("Check out a branch to push.".into(), window, cx);
            return;
        };
        let label = format!("Pushing to {remote_branch}");
        self.run_git(&label, move |root| git::push(root, &local, &remote_branch, tags), |_, _, _, _| {}, window, cx);
    }

    /// The control channel's `git` command: `refresh`; `popover <discard |
    /// stage | unstage | next | previous>` (the open hunk popover);
    /// `discard-hunk | stage-hunk | unstage-hunk <n>` (the active file's
    /// hunk `n`); `stage | unstage | discard <path>` (whole files, without
    /// Discard's confirmation); `branch <name>`, `new-branch <name>`;
    /// `commit <command>` (the open commit sheet, see `CommitSheet::command`);
    /// `remote <command>` (the open Push… or Pull… sheet); `diff <path>`;
    /// `inline-blame on|off`.
    pub fn git_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let (verb, arg) = command.split_once(' ').unwrap_or((command, ""));
        let active = || self.active_item().map(|item| item.path.clone()).ok_or("no file is shown".to_string());
        match verb {
            "refresh" => self.refresh_scm(cx),
            "popover" => {
                let popover = self.git.popover.as_ref().and_then(|p| p.upgrade()).ok_or("no hunk popover")?;
                popover.update(cx, |popover, cx| popover.command(arg, window, cx))?;
            }
            "discard-hunk" | "stage-hunk" | "unstage-hunk" => {
                let path = active()?;
                let n: usize = arg.parse().map_err(|_| format!("usage: git {verb} <n>"))?;
                match verb {
                    "discard-hunk" => self.discard_hunk(&path, n, window, cx),
                    stage => self.stage_hunk(&path, n, stage == "stage-hunk", window, cx),
                }
            }
            "stage" | "unstage" => self.stage_paths(vec![PathBuf::from(arg)], verb == "stage", window, cx),
            "discard" => {
                let root = self.git.root.clone().ok_or("not a repository")?;
                let file = self.git.status.file(Path::new(arg)).cloned().ok_or(format!("{arg} has no changes"))?;
                self.discard_files_now(root, vec![file], window, cx);
            }
            "branch" => {
                let branch = self.git.branches.iter().find(|b| b.name == arg).cloned().ok_or(format!("no branch {arg}"))?;
                self.switch_branch(branch, window, cx);
            }
            "new-branch" => self.create_branch(arg.to_string(), window, cx),
            "commit" => {
                let sheet = self.git.commit_sheet.as_ref().and_then(|s| s.upgrade()).ok_or("the commit sheet isn’t open")?;
                sheet.update(cx, |sheet, cx| sheet.command(arg, window, cx))?;
            }
            "remote" => {
                let sheet = self.git.remote_sheet.as_ref().and_then(|s| s.upgrade()).ok_or("no Push… or Pull… sheet")?;
                sheet.update(cx, |sheet, cx| sheet.command(arg, window, cx))?;
            }
            "diff" => self.open_diff(PathBuf::from(arg), window, cx),
            "inline-blame" => {
                self.set_setting(&["git", "inline_blame", "enabled"], serde_json::json!(arg == "on"), window, cx);
            }
            other => return Err(format!("unknown git command {other}")),
        }
        Ok(())
    }

    // ---------------------------------------------------------------------
    // The Source Control navigator

    pub(super) fn render_scm_navigator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let status = &self.git.status;
        let branch = match (&status.branch, &status.head) {
            (Some(branch), _) => branch.clone(),
            (None, Some(head)) => format!("detached at {}", &head[..head.len().min(8)]),
            (None, None) => String::new(),
        };
        let tracking = match (status.ahead, status.behind) {
            (0, 0) => String::new(),
            (ahead, behind) => {
                [(ahead, "↑"), (behind, "↓")].iter().filter(|(n, _)| *n > 0).map(|(n, a)| format!("{a}{n}")).collect::<Vec<_>>().join(" ")
            }
        };
        let button = |id: &'static str, icon: IconName, tooltip: &'static str| Button::new(id).icon(icon).ghost().xsmall().tooltip(tooltip);
        let mut list = v_flex().id("scm").size_full().overflow_y_scroll().child(
            h_flex()
                .px_2()
                .pt_2()
                .gap_1()
                .child(Icon::new(IconName::GitBranch).xsmall().text_color(theme.muted_foreground))
                .child(div().flex_1().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).child(branch))
                .when(!tracking.is_empty(), |el| el.child(div().text_xs().text_color(theme.muted_foreground).child(tracking)))
                .child(button("scm-fetch", IconName::Download, "Fetch Changes").on_click(cx.listener(|this, _, window, cx| this.fetch_changes(window, cx))))
                .child(button("scm-commit", IconName::GitCommitHorizontal, "Commit… (⌥⌘C)").on_click(cx.listener(|this, _, window, cx| this.show_commit_sheet(window, cx))))
                .child(button("scm-refresh", IconName::RefreshCw, "Refresh File Status").on_click(cx.listener(|this, _, _, cx| this.refresh_scm(cx)))),
        );
        if let Some(busy) = &self.git.busy {
            list = list.child(div().px_2().text_xs().text_color(theme.muted_foreground).child(format!("{busy}…")));
        }
        if self.git.root.is_none() {
            return list.child(empty(if self.git.discovered { "Not a Git repository." } else { "" }, cx));
        }
        let staged: Vec<FileStatus> = status.files.iter().filter(|f| f.is_staged()).cloned().collect();
        let unstaged: Vec<FileStatus> = status.files.iter().filter(|f| f.is_unstaged()).cloned().collect();
        if staged.is_empty() && unstaged.is_empty() {
            list = list.child(empty("No local changes.", cx));
        }
        for (title, files, is_staged) in [("Staged", staged, true), ("Changes", unstaged, false)] {
            if files.is_empty() {
                continue;
            }
            list = list.child(section_header(format!("{title} ({})", files.len()), cx));
            for (ix, file) in files.into_iter().enumerate() {
                list = list.child(self.render_scm_row(file, is_staged, ix, cx));
            }
        }
        let branches: Vec<Branch> = self.git.branches.iter().filter(|b| !b.remote).cloned().collect();
        list = list.child(
            h_flex()
                .justify_between()
                .child(section_header("Branches", cx))
                .child(button("scm-new-branch", IconName::GitBranchPlus, "New Branch…").on_click(cx.listener(|this, _, window, cx| this.show_branch_picker(true, window, cx)))),
        );
        for (ix, branch) in branches.into_iter().enumerate() {
            let current = branch.current;
            list = list.child(
                row(("scm-branch", ix))
                    .hover(|el| el.bg(theme.sidebar_accent))
                    .child(div().w(px(14.)).when(current, |el| el.child(Icon::new(IconName::Check).xsmall())))
                    .child(div().flex_1().truncate().when(current, |el| el.font_weight(FontWeight::SEMIBOLD)).child(branch.name.clone()))
                    .when_some(branch.upstream.clone(), |el, upstream| {
                        el.child(div().text_xs().text_color(theme.muted_foreground).truncate().max_w(px(110.)).child(upstream))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !branch.current {
                            this.switch_branch(branch.clone(), window, cx);
                        }
                    })),
            );
        }
        list
    }

    fn render_scm_row(&self, file: FileStatus, is_staged: bool, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let change = if is_staged { file.index } else { file.worktree };
        let color = match change {
            git::Change::Modified | git::Change::Renamed | git::Change::TypeChanged => theme.warning,
            git::Change::Added | git::Change::Untracked | git::Change::Copied => theme.success,
            git::Change::Deleted | git::Change::Conflicted => theme.danger,
            git::Change::Unmodified => theme.muted_foreground,
        };
        let path = file.path.clone();
        let exists = change != git::Change::Deleted;
        let workspace = cx.weak_entity();
        let menu_path = path.clone();
        row((if is_staged { "scm-staged" } else { "scm-row" }, ix))
            .hover(|el| el.bg(theme.sidebar_accent))
            .child(Icon::new(file_icon(&path)).small().text_color(theme.muted_foreground))
            .child(div().flex_1().truncate().when(!exists, |el| el.line_through()).child(super::pane::file_title(&path)))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .max_w(px(110.))
                    .child(self.relative(path.parent().unwrap_or(&path))),
            )
            .child(div().w(px(14.)).text_xs().font_weight(FontWeight::BOLD).text_color(color).child(change.letter()))
            // Selecting a change shows it, as in Xcode.
            .on_click(cx.listener(move |this, _, window, cx| {
                if path.is_file() {
                    this.open_diff(path.clone(), window, cx);
                }
            }))
            .context_menu(move |menu, _, _| {
                let with = |action: Rc<dyn Fn(&mut Workspace, PathBuf, &mut Window, &mut Context<Workspace>)>| {
                    let (workspace, path) = (workspace.clone(), menu_path.clone());
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        workspace.update(cx, |ws, cx| action(ws, path.clone(), window, cx)).ok();
                    }
                };
                menu.when(exists, |menu| {
                    menu.item(PopupMenuItem::new("Show Changes").on_click(with(Rc::new(|ws, path, window, cx| ws.open_diff(path, window, cx)))))
                        .item(PopupMenuItem::new("Open File").on_click(with(Rc::new(|ws, path, window, cx| ws.open_file(path, None, true, window, cx)))))
                        .separator()
                })
                .item(PopupMenuItem::new(if is_staged { "Unstage" } else { "Stage" }).on_click(with(Rc::new(move |ws, path, window, cx| {
                    ws.stage_paths(vec![path], !is_staged, window, cx)
                }))))
                .item(PopupMenuItem::new("Discard Changes…").on_click(with(Rc::new(|ws, path, window, cx| ws.discard_paths(vec![path], window, cx)))))
            })
    }

    /// The control channel's view of the repository.
    pub(super) fn git_summary(&self, cx: &App) -> serde_json::Value {
        let status = &self.git.status;
        let rel = |p: &Path| self.git.root.as_deref().and_then(|root| git::relative(root, p)).unwrap_or_else(|| p.display().to_string());
        serde_json::json!({
            "root": self.git.root,
            "branch": status.branch,
            "upstream": status.upstream,
            "ahead": status.ahead,
            "behind": status.behind,
            "staged": status.files.iter().filter(|f| f.is_staged()).map(|f| format!("{} {}", f.index.letter(), rel(&f.path))).collect::<Vec<_>>(),
            "unstaged": status.files.iter().filter(|f| f.is_unstaged()).map(|f| format!("{} {}", f.worktree.letter(), rel(&f.path))).collect::<Vec<_>>(),
            "branches": self.git.branches.iter().map(|b| format!("{}{}", if b.current { "* " } else { "" }, b.name)).collect::<Vec<_>>(),
            "remotes": self.git.remotes,
            "busy": self.git.busy,
            "last_error": self.git.last_error,
            "last_commit": self.git.last_commit,
            "inline_blame": self.git.inline_blame,
            "show_authors": self.git.show_authors,
            "hunks": self.hunks_summary(cx),
            "blame": self.blame_summary(cx),
            "popover": self.git.popover.as_ref().and_then(|p| p.upgrade()).map(|p| p.read(cx).summary()),
            "commit_sheet": self.git.commit_sheet.as_ref().and_then(|s| s.upgrade()).map(|s| s.read(cx).summary(cx)),
            "remote_sheet": self.git.remote_sheet.as_ref().and_then(|s| s.upgrade()).map(|s| s.read(cx).summary()),
            "diff": self.active_diff().map(|d| d.read(cx).summary(cx)),
        })
    }
}

/// The versions of `files` (with what was known of them) in `root`.
fn read_bases(root: &Path, files: Vec<(PathBuf, Option<Bases>)>) -> Vec<(PathBuf, Bases)> {
    let mut known: HashMap<String, String> = HashMap::new();
    let mut rels = vec![];
    let mut paths = vec![];
    for (path, bases) in files {
        let Some(rel) = git::relative(root, &path) else { continue };
        if let Some(bases) = bases {
            for (id, text) in [(bases.head_id, bases.head), (bases.index_id, bases.index)] {
                if let (Some(id), Some(text)) = (id, text) {
                    known.insert(id, text);
                }
            }
        }
        rels.push(rel);
        paths.push(path);
    }
    paths.into_iter().zip(git::bases(root, &rels, &known)).collect()
}

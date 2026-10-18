//! Keeps Xcode projects in step with the project navigator, as Xcode does:
//! New File, Rename (and move), Move to Trash and Add Files… update the
//! file references, groups and build files in `project.pbxproj`, and the
//! File inspector edits target membership. Edits run in the background, one
//! at a time; synchronized folders are left alone (see `xcodeproj::files`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use gpui_kit::*;

use super::Workspace;
use super::add_files::{AddFilesDialog, AddFilesRequest};
use crate::actions::AddFiles;
use crate::xcodeproj::files::{self, AddOptions, ProjectFiles};
use crate::xcodeproj::{self, XcodeProject};

/// Parsed Xcode projects for the File inspector, reloaded when their
/// `project.pbxproj` changes.
#[derive(Default)]
pub struct ProjectIndex {
    pub projects: Vec<(PathBuf, Option<SystemTime>, Arc<ProjectFiles>)>,
    /// Bumped on every reload, so views can tell their data is stale.
    pub generation: u64,
    task: Option<Task<()>>,
}

fn pbxproj_mtime(project: &Path) -> Option<SystemTime> {
    std::fs::metadata(XcodeProject::pbxproj_path(project)).and_then(|m| m.modified()).ok()
}

/// Copies a file or folder, refusing to overwrite.
fn copy_item(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "already exists"));
    }
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_item(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

impl Workspace {
    /// Applies `edit` to each Xcode project (or just `only`) in the
    /// background, writes the ones it changes, then refreshes the views that
    /// show them.
    pub(super) fn edit_projects(
        &mut self,
        only: Option<PathBuf>,
        edit: impl Fn(&Path, &str) -> Result<Option<String>> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let projects: Vec<PathBuf> =
            self.xcode_projects.iter().filter(|p| only.as_ref().is_none_or(|only| only == *p)).cloned().collect();
        if projects.is_empty() {
            return;
        }
        let lock = self.project_edit_lock.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (changed, errors) = cx
                .background_spawn(async move {
                    // One read-modify-write at a time.
                    let _guard = lock.lock();
                    let mut changed = false;
                    let mut errors = vec![];
                    for project in projects {
                        let file = XcodeProject::pbxproj_path(&project);
                        let result = std::fs::read_to_string(&file)
                            .map_err(anyhow::Error::from)
                            .and_then(|text| edit(&project, &text))
                            .and_then(|new| match new {
                                Some(text) => xcodeproj::write_project_file(&file, &text).map(|_| true),
                                None => Ok(false),
                            });
                        match result {
                            Ok(written) => changed |= written,
                            Err(err) => {
                                let name = project.file_name().unwrap_or_default().to_string_lossy().to_string();
                                errors.push(format!("Couldn’t update {name}: {err:#}"));
                            }
                        }
                    }
                    (changed, errors)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                for error in errors {
                    this.notify_error(error, window, cx);
                }
                if changed {
                    this.project_files_changed(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Something rewrote a `project.pbxproj`.
    pub(super) fn project_files_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(settings) = self.project_settings.clone() {
            settings.update(cx, |settings, cx| settings.reload_if_changed(window, cx));
        }
        self.refresh_project_index(cx);
        // New files, membership and build settings reach sourcekit-lsp.
        if let Some(build_server) = &self.build_server {
            build_server.update(cx, |server, cx| server.project_changed(cx));
        }
        // Target membership decides which test target a file's tests run in.
        self.refresh_tests(cx);
        cx.notify();
    }

    /// Re-parses (in the background) the projects whose file changed since
    /// they were last read.
    pub(super) fn refresh_project_index(&mut self, cx: &mut Context<Self>) {
        let projects = self.xcode_projects.clone();
        let known: Vec<(PathBuf, Option<SystemTime>, Arc<ProjectFiles>)> = self.project_index.projects.clone();
        self.project_index.task = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let mut changed = false;
                    let mut out = vec![];
                    for project in projects {
                        let mtime = pbxproj_mtime(&project);
                        if let Some(entry) = known.iter().find(|(p, m, _)| *p == project && *m == mtime) {
                            out.push(entry.clone());
                            continue;
                        }
                        changed = true;
                        let text = std::fs::read_to_string(XcodeProject::pbxproj_path(&project)).unwrap_or_default();
                        match ProjectFiles::parse(&project, &text) {
                            Ok(files) => out.push((project, mtime, Arc::new(files))),
                            Err(err) => log::warn!("couldn’t read {}: {err:#}", project.display()),
                        }
                    }
                    (changed || out.len() != known.len()).then_some(out)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(projects) = loaded {
                    this.project_index.projects = projects;
                    this.project_index.generation += 1;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// After New File: add the file to the targets Xcode would preselect.
    pub(super) fn add_new_file_to_projects(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_projects(
            None,
            move |project, text| {
                let files = ProjectFiles::parse(project, text)?;
                if !files.contains(&path) {
                    return Ok(None);
                }
                let targets = files.suggested_targets(&path);
                files::add_files(project, text, std::slice::from_ref(&path), &targets, &AddOptions::default())
            },
            window,
            cx,
        );
    }

    pub(super) fn move_in_projects(&mut self, from: PathBuf, to: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_projects(None, move |project, text| files::move_path(project, text, &from, &to), window, cx);
    }

    pub(super) fn remove_from_projects(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_projects(None, move |project, text| files::remove_paths(project, text, std::slice::from_ref(&path)), window, cx);
    }

    /// A Target Membership checkbox.
    pub fn set_target_membership(
        &mut self,
        project: PathBuf,
        path: PathBuf,
        target: String,
        member: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_projects(
            Some(project),
            move |project, text| files::set_target_membership(project, text, &path, &target, member),
            window,
            cx,
        );
    }

    /// The folder new files go in: the navigator's selected folder (or the
    /// selected file's), else the active file's, else the project root.
    pub(super) fn selected_folder(&self, cx: &App) -> PathBuf {
        let selected = self.tree.read(cx).selected_entry().map(|e| (PathBuf::from(e.item().id.as_str()), e.is_folder()));
        match selected {
            Some((path, true)) => path,
            Some((path, false)) => path.parent().map(Path::to_path_buf).unwrap_or(path),
            None => self
                .active_buffer()
                .and_then(|b| b.path.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| self.project.root.clone()),
        }
    }

    /// File ▸ Add Files… (⌥⌘A): choose files, then where and to which targets.
    pub(super) fn add_files(&mut self, _: &AddFiles, window: &mut Window, cx: &mut Context<Self>) {
        let destination = self.selected_folder(cx);
        self.prompt_add_files(destination, window, cx);
    }

    pub(super) fn prompt_add_files(&mut self, destination: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else { return };
            this.update_in(cx, |this, window, cx| this.show_add_files(paths, destination, window, cx)).ok();
        })
        .detach();
    }

    /// The Add Files sheet for `paths`, going into folder `destination`.
    pub fn show_add_files(&mut self, paths: Vec<PathBuf>, destination: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let destination = files::canonical(&destination);
        // The project whose folder holds the destination, read now: the
        // sheet needs its targets.
        let project = self.xcode_projects.iter().find_map(|project| {
            let text = std::fs::read_to_string(XcodeProject::pbxproj_path(project)).ok()?;
            let files = ProjectFiles::parse(project, &text).ok()?;
            files.covers(&destination).then(|| (project.clone(), files))
        });
        let workspace = cx.weak_entity();
        let dialog = cx.new(|cx| AddFilesDialog::new(workspace, paths, destination, project, window, cx));
        self.add_files_dialog = Some(dialog.downgrade());
        self.show_modal(dialog, window, cx);
    }

    /// Copies (if asked) and adds the files the sheet confirmed.
    pub fn finish_add_files(&mut self, request: AddFilesRequest, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_modal(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let AddFilesRequest { paths, destination, copy, project, targets, folder_references } = request;
            let folder = destination.clone();
            let (added, errors) = cx
                .background_spawn(async move {
                    let destination = folder;
                    let mut added = vec![];
                    let mut errors = vec![];
                    for path in paths {
                        let name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
                        if !copy || path.starts_with(&destination) {
                            added.push(path);
                            continue;
                        }
                        let target = destination.join(&name);
                        match copy_item(&path, &target) {
                            Ok(()) => added.push(target),
                            Err(err) => errors.push(format!("Couldn’t copy “{}”: {err}", name.to_string_lossy())),
                        }
                    }
                    (added, errors)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                for error in errors {
                    this.notify_error(error, window, cx);
                }
                this.rescan(cx);
                if let Some(project) = project
                    && !added.is_empty()
                {
                    let options = AddOptions { folder_references, destination: Some(destination) };
                    this.edit_projects(
                        Some(project),
                        move |project, text| files::add_files(project, text, &added, &targets, &options),
                        window,
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }
}

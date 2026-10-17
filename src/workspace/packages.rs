//! Swift packages in the project window: the Package Dependencies section
//! at the bottom of the project navigator (each resolved package with its
//! version, expandable to browse its sources read-only), and the package
//! workflow: File ▸ Add Package Dependencies…, File ▸ Packages (Reset
//! Package Caches, Resolve Package Versions, Update to Latest Package
//! Versions), and a package's Update Package and Remove Package.
//!
//! Package changes run one at a time in the background, as jobs: a file
//! edit (`project.pbxproj`, `Package.swift`, `Package.resolved`, under the
//! project edit lock), then the resolver. Their progress shows in the
//! activity view and their errors in the Issue navigator; afterwards the
//! index refreshes so new modules are importable.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::component::tree::TreeItem;
use gpui_kit::*;
use smol::channel;

use super::add_package::{AddPackageDialog, AddPackageRequest};
use super::{NavigatorTab, Workspace};
use crate::actions::{AddPackageDependencies, ResetPackageCaches, ResolvePackageVersions, UpdateToLatestPackageVersions};
use crate::build::log::{Issue, IssueSource, Severity};
use crate::build::process::{self, ProcessHandle};
use crate::packages::{self, Dependencies, Operation, manifest, resolved};
use crate::project::{self, ProjectKind};
use crate::xcodeproj::packages::{ProjectPackages, Source, add_package, remove_package};
use crate::xcodeproj::{self, XcodeProject};

/// Tree item IDs of the section header and of each package (files in a
/// package use their paths, as everywhere in the navigator).
pub const PACKAGES_ID: &str = "zcode:packages";
const PACKAGE_PREFIX: &str = "zcode:package:";

pub fn package_item_id(identity: &str) -> String {
    format!("{PACKAGE_PREFIX}{identity}")
}

pub fn package_of_item(id: &str) -> Option<&str> {
    id.strip_prefix(PACKAGE_PREFIX)
}

/// What the navigator shows on a package's row.
#[derive(Clone, Debug)]
pub struct PackageRow {
    pub name: String,
    pub state: String,
    pub local: bool,
}

/// A package change: a file edit, then commands, stopping at the first
/// that fails.
pub struct Job {
    pub activity: String,
    pub edit: Option<Box<dyn FnOnce() -> anyhow::Result<()> + Send>>,
    /// The edit rewrites `project.pbxproj`, so views showing it reload.
    pub edits_project: bool,
    pub commands: Vec<Command>,
}

#[derive(Default)]
pub struct PackagesState {
    pub dependencies: Arc<Dependencies>,
    /// The navigator's Package Dependencies section.
    pub items: Vec<TreeItem>,
    pub rows: Rc<HashMap<SharedString, PackageRow>>,
    /// What the activity view shows while a job runs.
    pub activity: Option<String>,
    /// Errors of the last job, for the Issue navigator.
    pub issues: Vec<Issue>,
    pub last_log: Option<PathBuf>,
    /// Jobs finished, so tests can wait for one.
    pub completed: u64,
    pub dialog: Option<WeakEntity<AddPackageDialog>>,
    queue: VecDeque<Job>,
    running: bool,
    process: ProcessHandle,
    generation: u64,
    load_task: Option<Task<()>>,
    job_task: Option<Task<()>>,
}

impl PackagesState {
    pub fn is_busy(&self) -> bool {
        self.running || !self.queue.is_empty()
    }
}

/// Expanded item IDs in `items` and their descendants.
fn expanded_ids(items: &[TreeItem], out: &mut HashSet<SharedString>) {
    for item in items {
        if item.is_expanded() {
            out.insert(item.id.clone());
        }
        expanded_ids(&item.children, out);
    }
}

/// `item` with the expansion `expanded` remembers.
fn restore_expansion(mut item: TreeItem, expanded: &HashSet<SharedString>) -> TreeItem {
    item.children = std::mem::take(&mut item.children).into_iter().map(|c| restore_expansion(c, expanded)).collect();
    if expanded.contains(&item.id) { item.expanded(true) } else { item }
}

impl Workspace {
    /// The navigator's items: the project's files, then Package Dependencies.
    pub(super) fn update_tree(&mut self, cx: &mut Context<Self>) {
        let mut items = self.file_items.clone();
        items.extend(self.packages.items.iter().cloned());
        self.tree.update(cx, |tree, cx| tree.set_items(items, cx));
        self.reveal_active_in_tree(cx);
    }

    /// Re-reads the package graph and the files of checked-out packages in
    /// the background, then rebuilds the Package Dependencies section.
    pub(super) fn refresh_packages(&mut self, cx: &mut Context<Self>) {
        if matches!(self.project.kind, ProjectKind::Folder) {
            return;
        }
        self.packages.generation += 1;
        let generation = self.packages.generation;
        let project = self.project.clone();
        let xcode_projects = self.xcode_projects.clone();
        let cache_dir = self.cache_dir.clone();
        self.packages.load_task = Some(cx.spawn(async move |this, cx| {
            let (dependencies, files) = cx
                .background_spawn(async move {
                    let dependencies = packages::load(&project, &xcode_projects, &cache_dir);
                    // Local packages inside the project already show in its
                    // file tree.
                    let files: HashMap<String, Vec<PathBuf>> = dependencies
                        .packages
                        .iter()
                        .filter_map(|p| Some((p, p.checkout.as_ref()?)))
                        .filter(|(p, checkout)| !(p.local && checkout.starts_with(&project.root)))
                        .map(|(p, checkout)| (p.identity.clone(), project::scan_files(checkout)))
                        .collect();
                    (dependencies, files)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.packages.generation != generation {
                    return;
                }
                this.set_package_items(dependencies, files, cx);
            })
            .ok();
        }));
    }

    fn set_package_items(&mut self, dependencies: Dependencies, files: HashMap<String, Vec<PathBuf>>, cx: &mut Context<Self>) {
        // Nothing changed (most refreshes after a build): keep the tree and
        // its selection.
        if *self.packages.dependencies == dependencies && (dependencies.packages.is_empty() || !self.packages.items.is_empty()) {
            return;
        }
        let mut expanded = HashSet::new();
        expanded_ids(&self.packages.items, &mut expanded);
        // The section starts expanded and its packages collapsed, as in Xcode.
        if self.packages.items.is_empty() {
            expanded.insert(PACKAGES_ID.into());
        }
        let mut rows = HashMap::new();
        let packages: Vec<TreeItem> = dependencies
            .packages
            .iter()
            .map(|package| {
                let id = package_item_id(&package.identity);
                rows.insert(
                    SharedString::from(id.clone()),
                    PackageRow {
                        name: package.name.clone(),
                        state: package.state.clone(),
                        local: package.local,
                    },
                );
                let children = match (&package.checkout, files.get(&package.identity)) {
                    (Some(checkout), Some(files)) => project::build_tree(checkout, files),
                    _ => vec![],
                };
                restore_expansion(TreeItem::new(id, package.name.clone()).children(children), &expanded)
            })
            .collect();
        self.packages.items = if packages.is_empty() {
            vec![]
        } else {
            vec![restore_expansion(TreeItem::new(PACKAGES_ID, "Package Dependencies").children(packages), &expanded)]
        };
        self.packages.rows = Rc::new(rows);
        self.packages.dependencies = Arc::new(dependencies);
        self.update_tree(cx);
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // Jobs

    pub(super) fn queue_package_job(&mut self, job: Job, window: &mut Window, cx: &mut Context<Self>) {
        self.packages.queue.push_back(job);
        self.start_next_package_job(window, cx);
    }

    fn start_next_package_job(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.packages.running {
            return;
        }
        let Some(job) = self.packages.queue.pop_front() else { return };
        self.packages.running = true;
        self.packages.activity = Some(job.activity.clone());
        self.packages.issues.clear();
        let handle = ProcessHandle::default();
        self.packages.process = handle.clone();
        let lock = self.project_edit_lock.clone();
        let log_path = self.cache_dir.join("logs").join(format!(
            "{}-packages.log",
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default()
        ));
        cx.notify();
        self.packages.job_task = Some(cx.spawn_in(window, async move |this, cx| {
            let Job { activity, edit, edits_project, commands } = job;
            let mut lines = vec![activity];
            let mut ok = true;
            if let Some(edit) = edit {
                let result = cx
                    .background_spawn(async move {
                        let _guard = lock.lock();
                        edit()
                    })
                    .await;
                if let Err(err) = result {
                    lines.push(format!("error: {err:#}"));
                    ok = false;
                }
                if edits_project && ok {
                    this.update_in(cx, |this, window, cx| this.project_files_changed(window, cx)).ok();
                }
            }
            for cmd in commands {
                if !ok {
                    break;
                }
                lines.push(format!("$ {}", crate::build::display_command(&cmd)));
                let (tx, rx) = channel::unbounded::<String>();
                let process = cx.background_spawn(process::run_streaming(cmd, tx, handle.clone()));
                while let Ok(line) = rx.recv().await {
                    if let Some(activity) = packages::progress(&line) {
                        this.update(cx, |this, cx| {
                            this.packages.activity = Some(activity);
                            cx.notify();
                        })
                        .ok();
                    }
                    lines.push(line);
                }
                match process.await {
                    Ok(Some(0)) => {}
                    Ok(code) => {
                        ok = false;
                        lines.push(format!("(exit {})", code.map_or("signal".to_string(), |c| c.to_string())));
                    }
                    Err(err) => {
                        ok = false;
                        lines.push(format!("error: {err:#}"));
                    }
                }
            }
            let log = lines.join("\n");
            let written = log_path.clone();
            cx.background_spawn(async move {
                if let Some(dir) = written.parent() {
                    std::fs::create_dir_all(dir).ok();
                }
                std::fs::write(&written, log).ok();
            })
            .await;
            this.update_in(cx, |this, window, cx| this.finish_package_job(ok, lines, log_path, window, cx)).ok();
        }));
    }

    fn finish_package_job(&mut self, ok: bool, lines: Vec<String>, log: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.packages.running = false;
        self.packages.activity = None;
        self.packages.completed += 1;
        self.packages.last_log = Some(log);
        if ok {
            self.packages.issues.clear();
            // New modules become importable once the index sees them.
            match self.project.kind {
                ProjectKind::SwiftPackage => {
                    let changed = vec![self.project.root.join("Package.swift"), self.project.root.join("Package.resolved")];
                    self.lsp.update(cx, |lsp, cx| lsp.files_changed(&changed, cx));
                }
                _ => self.build.update(cx, |build, cx| build.refresh_index(cx)),
            }
        } else {
            let mut messages = packages::errors(&lines);
            if messages.is_empty() {
                let last = lines.iter().rev().find(|l| !l.trim().is_empty() && !l.starts_with("(exit")).cloned().unwrap_or_default();
                messages.push(format!("Package resolution failed: {}", last.trim()));
            }
            self.packages.issues = messages
                .into_iter()
                .map(|message| Issue { severity: Severity::Error, message, path: None, line: None, column: None, source: IssueSource::Package })
                .collect();
            self.navigator = NavigatorTab::Issues;
            self.show_navigator = true;
        }
        self.refresh_packages(cx);
        self.start_next_package_job(window, cx);
        cx.notify();
    }

    /// The scheme a workspace resolves with: the chosen one, else the first.
    fn resolve_scheme(&self, cx: &App) -> Option<String> {
        let build = self.build.read(cx);
        build.scheme.clone().or_else(|| build.schemes.schemes.first().cloned())
    }

    /// File ▸ Packages commands and Update Package.
    pub fn package_operation(&mut self, operation: Operation, window: &mut Window, cx: &mut Context<Self>) {
        if !self.project.kind.is_xcode() && self.project.kind != ProjectKind::SwiftPackage {
            return;
        }
        let project = self.project.clone();
        let dependencies = self.packages.dependencies.clone();
        let cache_dir = self.cache_dir.clone();
        let commands = packages::commands(&self.project, &operation, self.resolve_scheme(cx).as_deref());
        let prepare = operation.clone();
        let job = Job {
            activity: operation.activity(),
            edit: Some(Box::new(move || packages::prepare(&project, &prepare, &dependencies, &cache_dir))),
            edits_project: false,
            commands,
        };
        self.queue_package_job(job, window, cx);
    }

    pub(super) fn resolve_package_versions(&mut self, _: &ResolvePackageVersions, window: &mut Window, cx: &mut Context<Self>) {
        self.package_operation(Operation::Resolve, window, cx);
    }

    pub(super) fn update_to_latest_package_versions(
        &mut self,
        _: &UpdateToLatestPackageVersions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.package_operation(Operation::UpdateAll, window, cx);
    }

    pub(super) fn reset_package_caches(&mut self, _: &ResetPackageCaches, window: &mut Window, cx: &mut Context<Self>) {
        self.package_operation(Operation::Reset, window, cx);
    }

    /// A package's Update Package.
    pub fn update_package(&mut self, identity: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(package) = self.packages.dependencies.package(identity).cloned() else { return };
        if package.local {
            return;
        }
        self.package_operation(Operation::Update(package.identity), window, cx);
    }

    /// A package's Remove Package: takes it out of the project (or
    /// `Package.swift`) with the products targets link from it, then
    /// resolves.
    pub fn remove_package(&mut self, identity: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let package = self.packages.dependencies.package(identity).cloned().ok_or(format!("no package {identity}"))?;
        if !package.direct {
            return Err(format!("{} is a dependency of another package", package.name));
        }
        let identity = package.identity.clone();
        let (edit, edits_project): (Box<dyn FnOnce() -> anyhow::Result<()> + Send>, bool) = match &self.project.kind {
            ProjectKind::SwiftPackage => {
                let manifest_path = self.project.root.join("Package.swift");
                (
                    Box::new(move || {
                        let text = std::fs::read_to_string(&manifest_path)?;
                        std::fs::write(&manifest_path, manifest::remove_dependency(&text, &identity)?)?;
                        Ok(())
                    }),
                    false,
                )
            }
            _ => {
                let (xcodeproj, reference) =
                    package.reference.clone().ok_or(format!("{} isn’t referenced by a project", package.name))?;
                (
                    Box::new(move || {
                        let file = XcodeProject::pbxproj_path(&xcodeproj);
                        let text = std::fs::read_to_string(&file)?;
                        if let Some(text) = remove_package(&text, &reference)? {
                            xcodeproj::write_project_file(&file, &text)?;
                        }
                        Ok(())
                    }),
                    true,
                )
            }
        };
        let job = Job {
            activity: format!("Removing {}", package.name),
            edit: Some(edit),
            edits_project,
            commands: packages::commands(&self.project, &Operation::Resolve, self.resolve_scheme(cx).as_deref()),
        };
        self.queue_package_job(job, window, cx);
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Add Package Dependencies

    /// File ▸ Add Package Dependencies…
    pub(super) fn add_package_dependencies(&mut self, _: &AddPackageDependencies, window: &mut Window, cx: &mut Context<Self>) {
        self.show_add_package(None, window, cx);
    }

    /// The Add Package Dependencies sheet, optionally searching `query`.
    pub fn show_add_package(&mut self, query: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.project.kind.is_xcode() && self.project.kind != ProjectKind::SwiftPackage {
            self.notify_error("Packages can be added to Xcode projects and Swift packages.".into(), window, cx);
            return;
        }
        // The projects packages can go in, with their targets, read now.
        let projects: Vec<(PathBuf, ProjectPackages)> = self
            .xcode_projects
            .iter()
            .filter_map(|p| {
                let text = std::fs::read_to_string(XcodeProject::pbxproj_path(p)).ok()?;
                Some((p.clone(), ProjectPackages::parse(&text).ok()?))
            })
            .collect();
        let workspace = cx.weak_entity();
        let kind = self.project.kind.clone();
        let root = self.project.root.clone();
        let scratch = self.cache_dir.join("package-scratch");
        let dialog = cx.new(|cx| AddPackageDialog::new(workspace, kind, root, projects, scratch, window, cx));
        self.packages.dialog = Some(dialog.downgrade());
        self.show_modal(dialog.clone(), window, cx);
        if let Some(query) = query {
            dialog.update(cx, |d, cx| d.search_for(&query, window, cx));
        }
    }

    /// Add Local…: the sheet's button, after its folder panel.
    pub fn prompt_add_local_package(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add Package".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            this.update_in(cx, |this, window, cx| this.add_local_package(path, window, cx)).ok();
        })
        .detach();
    }

    /// The local package at `path` in the sheet's product step.
    pub fn add_local_package(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = match self.packages.dialog.as_ref().and_then(|d| d.upgrade()) {
            Some(dialog) if self.modal.is_some() => dialog,
            _ => {
                self.show_add_package(None, window, cx);
                match self.packages.dialog.as_ref().and_then(|d| d.upgrade()) {
                    Some(dialog) => dialog,
                    None => return,
                }
            }
        };
        dialog.update(cx, |d, cx| d.choose_local(path, cx));
    }

    /// The control channel's `package <field> [<value>]`: `open [query]`,
    /// `local <path>`, or a field of the open sheet (see
    /// `AddPackageDialog::set_field`).
    pub fn add_package_field(&mut self, field: &str, value: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        match field {
            "open" => {
                self.show_add_package((!value.is_empty()).then(|| value.to_string()), window, cx);
                Ok(())
            }
            "local" => {
                self.add_local_package(PathBuf::from(value), window, cx);
                Ok(())
            }
            // Add Package on the products step.
            "add" => {
                let dialog = self.packages.dialog.as_ref().and_then(|d| d.upgrade()).ok_or("Add Package isn’t open")?;
                let request = dialog.read(cx).request().ok_or("choose the package’s products first")?;
                self.finish_add_package(request, window, cx);
                Ok(())
            }
            _ => {
                let dialog = self.packages.dialog.as_ref().and_then(|d| d.upgrade()).ok_or("Add Package isn’t open")?;
                dialog.update(cx, |d, cx| d.set_field(field, value, window, cx))
            }
        }
    }

    /// Adds what the sheet confirmed, then resolves.
    pub fn finish_add_package(&mut self, request: AddPackageRequest, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_modal(window, cx);
        self.packages.dialog = None;
        let AddPackageRequest { source, project, products } = request;
        let location = match &source {
            Source::Remote { url, .. } => url.clone(),
            Source::Local { path } => path.display().to_string(),
        };
        let name = resolved::display_name(&location);
        let job = match &self.project.kind {
            ProjectKind::SwiftPackage => {
                let root = self.project.root.clone();
                let swift = |args: Vec<String>| {
                    let mut cmd = Command::new("/usr/bin/swift");
                    cmd.arg("package").args(args).current_dir(&root);
                    cmd
                };
                let mut add = vec!["add-dependency".to_string()];
                match &source {
                    Source::Remote { url, rule } => {
                        add.push(url.clone());
                        add.extend(rule.swiftpm_args());
                    }
                    Source::Local { path } => {
                        let rel = crate::paths::relative_path(&crate::xcodeproj::files::canonical(&root), &crate::xcodeproj::files::canonical(path));
                        add.extend([rel.display().to_string(), "--type".into(), "path".into()]);
                    }
                }
                // Already a dependency: just add the products.
                let manifest_text = std::fs::read_to_string(root.join("Package.swift")).unwrap_or_default();
                let declared = manifest::declared(&manifest_text).iter().any(|d| d.identity() == resolved::identity(&location));
                let mut commands = vec![];
                if !declared {
                    commands.push(swift(add));
                }
                for (product, target) in &products {
                    commands.push(swift(vec![
                        "add-target-dependency".into(),
                        product.clone(),
                        target.clone(),
                        "--package".into(),
                        name.clone(),
                    ]));
                }
                commands.push(swift(vec!["resolve".into()]));
                Job { activity: format!("Adding {name}"), edit: None, edits_project: false, commands }
            }
            _ => {
                let Some(xcodeproj) = project.or_else(|| self.xcode_projects.first().cloned()) else { return };
                Job {
                    activity: format!("Adding {name}"),
                    edit: Some(Box::new(move || {
                        let file = XcodeProject::pbxproj_path(&xcodeproj);
                        let text = std::fs::read_to_string(&file)?;
                        if let (Some(text), _) = add_package(&xcodeproj, &text, &source, &products)? {
                            xcodeproj::write_project_file(&file, &text)?;
                        }
                        Ok(())
                    })),
                    edits_project: true,
                    commands: packages::commands(&self.project, &Operation::Resolve, self.resolve_scheme(cx).as_deref()),
                }
            }
        };
        self.queue_package_job(job, window, cx);
    }

    // ---------------------------------------------------------------------
    // State for the debug harness

    pub(super) fn packages_summary(&self, cx: &App) -> serde_json::Value {
        let dependencies = &self.packages.dependencies;
        // The section's rows as shown, indented by depth.
        let tree = self.tree.read(cx);
        let mut rows = vec![];
        let mut in_section = false;
        for ix in 0.. {
            let Some(entry) = tree.entry(ix) else { break };
            if entry.item().id.as_ref() == PACKAGES_ID {
                in_section = true;
            }
            if in_section {
                rows.push(format!("{}{}", "  ".repeat(entry.depth()), entry.item().label));
            }
        }
        serde_json::json!({
            "list": dependencies.packages.iter().map(|p| {
                let flags = [(p.local, "local"), (p.direct, "direct"), (p.checkout.is_some(), "checkout")];
                [p.name.as_str(), p.state.as_str()]
                    .into_iter()
                    .chain(flags.into_iter().filter(|(on, _)| *on).map(|(_, flag)| flag))
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            }).collect::<Vec<_>>(),
            "rules": dependencies.packages.iter().filter_map(|p| Some(format!("{} {}", p.name, p.rule.as_ref()?.summary()))).collect::<Vec<_>>(),
            "checkouts": dependencies.packages.iter().filter_map(|p| Some((p.identity.clone(), p.checkout.clone()?))).collect::<HashMap<_, _>>(),
            "tree": rows,
            "activity": self.packages.activity,
            "busy": self.packages.is_busy(),
            "completed": self.packages.completed,
            "issues": self.packages.issues.iter().map(|i| i.message.clone()).collect::<Vec<_>>(),
            "last_log": self.packages.last_log,
            "add_package": self.packages.dialog.as_ref().and_then(|d| d.upgrade()).map(|d| d.read(cx).summary(cx)),
        })
    }

    /// The package `path` belongs to and its path inside it. zcode's index
    /// build keeps its own checkouts, so a checkout's folder name counts.
    pub(super) fn package_path(&self, path: &Path) -> Option<(String, PathBuf)> {
        let packages = &self.packages.dependencies.packages;
        if let Some(found) = packages
            .iter()
            .find_map(|p| Some((p.name.clone(), path.strip_prefix(p.checkout.as_ref()?).ok()?.to_path_buf())))
        {
            return Some(found);
        }
        if !packages::location::is_package_checkout(path) {
            return None;
        }
        let parts: Vec<_> = path.iter().collect();
        let at = parts.iter().position(|p| *p == "checkouts")? + 1;
        let folder = parts.get(at)?;
        let name = packages
            .iter()
            .find(|p| p.checkout.as_ref().and_then(|c| c.file_name()) == Some(folder))
            .map_or_else(|| folder.to_string_lossy().to_string(), |p| p.name.clone());
        Some((name, parts[at + 1..].iter().collect()))
    }

    /// Reveals `path` in the project navigator (⇧⌘J for any file), expanding
    /// its folders, a package's included.
    pub fn reveal_in_navigator(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.show_navigator_tab(NavigatorTab::Project, window, cx);
        let id: SharedString = path.to_string_lossy().to_string().into();
        self.tree.update(cx, |tree, cx| {
            tree.reveal_item(&id, ScrollStrategy::Center, cx);
            if let Some(ix) = tree.index_of(&id) {
                tree.set_selected_index(Some(ix), cx);
            }
        });
    }
}

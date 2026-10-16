//! File ▸ Add Package Dependencies…: Xcode's package sheet. Enter a
//! package's URL (or a GitHub `owner/repo`) in the search field, pick a
//! dependency rule (versions come from the repository's tags), and Add
//! Package fetches that version to read its products; then choose the
//! target each product is added to. Add Local… adds a package folder.

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::Workspace;
use crate::actions::{PickerConfirm, PickerDismiss};
use crate::packages::remote::{self, Manifest, Product, ProductKind, Refs};
use crate::packages::resolved::display_name;
use crate::packages::rule::{Rule, RuleKind, Version};
use crate::project::ProjectKind;
use crate::xcodeproj::packages::{ProjectPackages, Source};

/// What the sheet confirmed.
pub struct AddPackageRequest {
    pub source: Source,
    /// The `.xcodeproj` the package goes in (Xcode projects).
    pub project: Option<PathBuf>,
    /// (product, target): a target ID in Xcode projects, a target name in
    /// Swift packages.
    pub products: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Search,
    /// Fetching the chosen version to read its manifest.
    Verifying,
    Products,
}

struct ProductRow {
    product: Product,
    /// Index into `targets()`, or `None` to leave the product out.
    target: Option<usize>,
}

pub struct AddPackageDialog {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    kind: ProjectKind,
    /// Xcode projects with their packages and targets.
    projects: Vec<(PathBuf, ProjectPackages)>,
    project_ix: usize,
    /// A Swift package's targets (name, type), read in the background.
    package_targets: Vec<(String, String)>,
    scratch: PathBuf,
    search: Entity<InputState>,
    version: Entity<InputState>,
    upper: Entity<InputState>,
    branch: Entity<InputState>,
    commit: Entity<InputState>,
    step: Step,
    url: Option<String>,
    refs: Option<Refs>,
    fetching: bool,
    error: Option<String>,
    rule_kind: RuleKind,
    source: Option<Source>,
    manifest: Option<Manifest>,
    rows: Vec<ProductRow>,
    /// Focus moves to the sheet when the search field goes away.
    refocus: bool,
    _subscriptions: Vec<Subscription>,
    task: Option<Task<()>>,
    _targets_task: Option<Task<()>>,
}

impl EventEmitter<DismissEvent> for AddPackageDialog {}

impl Focusable for AddPackageDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Typing goes to the search field while it's shown.
        match self.step {
            Step::Search => self.search.read(cx).focus_handle(cx),
            _ => self.focus_handle.clone(),
        }
    }
}

impl AddPackageDialog {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        kind: ProjectKind,
        root: PathBuf,
        projects: Vec<(PathBuf, ProjectPackages)>,
        scratch: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let search = input("Search or Enter Package URL", window, cx);
        let version = input("1.0.0", window, cx);
        let upper = input("2.0.0", window, cx);
        let branch = input("main", window, cx);
        let commit = input("Commit hash", window, cx);
        let subscriptions = vec![cx.subscribe_in(&search, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.search(window, cx);
            }
        })];
        // A Swift package's targets come from its manifest.
        let targets_task = (kind == ProjectKind::SwiftPackage).then(|| {
            let root = root.clone();
            cx.spawn(async move |this, cx| {
                let manifest = cx.background_spawn(async move { remote::dump_package(&root) }).await;
                this.update(cx, |this, cx| {
                    match manifest {
                        Ok(manifest) => {
                            this.package_targets = manifest.targets;
                            let default = this.default_target();
                            for row in this.rows.iter_mut().filter(|r| r.product.kind == ProductKind::Library && r.target.is_none()) {
                                row.target = default;
                            }
                        }
                        Err(err) => this.error = Some(format!("Couldn’t read Package.swift: {err:#}")),
                    }
                    cx.notify();
                })
                .ok();
            })
        });
        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            kind,
            projects,
            project_ix: 0,
            package_targets: vec![],
            scratch,
            search,
            version,
            upper,
            branch,
            commit,
            step: Step::Search,
            url: None,
            refs: None,
            fetching: false,
            error: None,
            rule_kind: RuleKind::UpToNextMajor,
            source: None,
            manifest: None,
            rows: vec![],
            refocus: false,
            _subscriptions: subscriptions,
            task: None,
            _targets_task: targets_task,
        }
    }

    /// Targets products can be added to: (ID or name, name).
    fn targets(&self) -> Vec<(String, String)> {
        match self.kind {
            ProjectKind::SwiftPackage => self.package_targets.iter().map(|(name, _)| (name.clone(), name.clone())).collect(),
            _ => self
                .projects
                .get(self.project_ix)
                .map(|(_, p)| p.targets.iter().map(|t| (t.id.clone(), t.name.clone())).collect())
                .unwrap_or_default(),
        }
    }

    /// Where a library goes unless changed: the first target that isn't a
    /// test bundle, as Xcode preselects.
    fn default_target(&self) -> Option<usize> {
        match self.kind {
            ProjectKind::SwiftPackage => self.package_targets.iter().position(|(_, ty)| ty == "regular").or_else(|| {
                self.package_targets.iter().position(|(_, ty)| ty != "test")
            }),
            _ => self.projects.get(self.project_ix).and_then(|(_, p)| {
                p.targets.iter().position(|t| !t.product_type.as_deref().unwrap_or("").contains("test"))
            }),
        }
    }

    fn input_value(input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_string()
    }

    fn set_input(input: &Entity<InputState>, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let value = value.to_string();
        input.update(cx, |input, cx| input.set_value(value, window, cx));
    }

    /// The rule the controls describe.
    fn rule(&self, cx: &App) -> Rule {
        let version = Self::input_value(&self.version, cx);
        match self.rule_kind {
            RuleKind::UpToNextMajor => Rule::UpToNextMajor(version),
            RuleKind::UpToNextMinor => Rule::UpToNextMinor(version),
            RuleKind::Range => Rule::Range(version, Self::input_value(&self.upper, cx)),
            RuleKind::Exact => Rule::Exact(version),
            RuleKind::Branch => Rule::Branch(Self::input_value(&self.branch, cx)),
            RuleKind::Commit => Rule::Commit(Self::input_value(&self.commit, cx)),
        }
    }

    /// What the rule picks among the repository's tags, or why it can't.
    fn resolution(&self, cx: &App) -> Result<String, String> {
        let Some(refs) = &self.refs else { return Err(String::new()) };
        let rule = self.rule(cx);
        if !rule.is_valid() {
            return Err(match rule.kind() {
                RuleKind::Branch => "Enter a branch.".into(),
                RuleKind::Commit => "Enter a commit hash.".into(),
                RuleKind::Range => "Enter a version range.".into(),
                _ => "Enter a version.".into(),
            });
        }
        match &rule {
            Rule::Branch(b) if !refs.branches.is_empty() && !refs.branches.contains(b) => Err(format!("No branch “{b}”.")),
            Rule::Branch(b) => Ok(format!("Branch {b}")),
            Rule::Commit(c) => Ok(format!("Commit {}", &c[..c.len().min(7)])),
            _ => rule.pick(&refs.releases).map(|r| r.tag.clone()).ok_or_else(|| "No version matches this rule.".into()),
        }
    }

    pub fn search_for(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        Self::set_input(&self.search, query, window, cx);
        self.search(window, cx);
    }

    /// Looks up what's in the search field: a package folder is added as a
    /// local package; anything else is a repository to list.
    fn search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = Self::input_value(&self.search, cx);
        if text.is_empty() {
            return;
        }
        let url = remote::normalize_url(&text);
        let path = match url.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
            None => PathBuf::from(&url),
        };
        if path.is_absolute() && path.join("Package.swift").exists() {
            self.choose_local(path, cx);
            return;
        }
        self.step = Step::Search;
        self.url = Some(url.clone());
        self.refs = None;
        self.error = None;
        self.fetching = true;
        cx.notify();
        let lookup = url.clone();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let refs = cx.background_spawn(async move { remote::list_refs(&lookup) }).await;
            this.update_in(cx, |this, window, cx| {
                this.fetching = false;
                match refs {
                    Ok(refs) => this.show_refs(refs, window, cx),
                    Err(err) => this.error = Some(format!("No package found at {url}: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Fills the rule's controls the way Xcode does: Up to Next Major from
    /// the latest release, or the default branch when there are no releases.
    fn show_refs(&mut self, refs: Refs, window: &mut Window, cx: &mut Context<Self>) {
        let latest = crate::packages::rule::latest(&refs.releases).map(|r| r.tag.clone());
        let branch = refs.default_branch.clone().or_else(|| refs.branches.first().cloned()).unwrap_or_else(|| "main".into());
        Self::set_input(&self.branch, &branch, window, cx);
        match &latest {
            Some(tag) => {
                Self::set_input(&self.version, tag, window, cx);
                let upper = Version::parse(tag).map(|v| v.next_major().to_string()).unwrap_or_default();
                Self::set_input(&self.upper, &upper, window, cx);
                self.rule_kind = RuleKind::UpToNextMajor;
            }
            None => self.rule_kind = RuleKind::Branch,
        }
        self.refs = Some(refs);
    }

    fn set_rule_kind(&mut self, kind: RuleKind, cx: &mut Context<Self>) {
        self.rule_kind = kind;
        cx.notify();
    }

    /// Add Package (first step): fetch the version the rule picks and read
    /// its products.
    fn verify(&mut self, cx: &mut Context<Self>) {
        let (Some(url), Some(refs)) = (self.url.clone(), self.refs.as_ref()) else { return };
        if let Err(message) = self.resolution(cx) {
            self.error = Some(message);
            cx.notify();
            return;
        }
        let rule = self.rule(cx);
        let reference = match remote::checkout_ref(&rule, &refs.releases) {
            Ok(reference) => reference,
            Err(err) => {
                self.error = Some(err.to_string());
                cx.notify();
                return;
            }
        };
        let is_commit = rule.kind() == RuleKind::Commit;
        self.source = Some(Source::Remote { url: url.clone(), rule });
        self.start_verifying(cx, move |scratch| remote::fetch_manifest(&url, &reference, is_commit, &scratch));
    }

    /// Add Local…'s folder: read its products.
    pub fn choose_local(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !path.join("Package.swift").exists() {
            self.error = Some(format!("{} isn’t a Swift package: it has no Package.swift.", path.display()));
            cx.notify();
            return;
        }
        self.url = Some(path.display().to_string());
        self.source = Some(Source::Local { path: path.clone() });
        self.start_verifying(cx, move |_| remote::dump_package(&path));
    }

    fn start_verifying(&mut self, cx: &mut Context<Self>, read: impl FnOnce(PathBuf) -> anyhow::Result<Manifest> + Send + 'static) {
        self.step = Step::Verifying;
        self.refocus = true;
        self.error = None;
        cx.notify();
        let scratch = self.scratch.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let manifest = cx.background_spawn(async move { read(scratch) }).await;
            this.update(cx, |this, cx| {
                match manifest {
                    Ok(manifest) => this.show_products(manifest),
                    Err(err) => {
                        this.step = Step::Search;
                        this.error = Some(format!("{err:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn show_products(&mut self, manifest: Manifest) {
        let default = self.default_target();
        self.rows = manifest
            .products
            .iter()
            .map(|product| ProductRow {
                product: product.clone(),
                // Only libraries can be linked; Xcode leaves the rest at None.
                target: (product.kind == ProductKind::Library).then_some(default).flatten(),
            })
            .collect();
        self.manifest = Some(manifest);
        self.step = Step::Products;
    }

    fn package_name(&self) -> String {
        match &self.manifest {
            Some(m) if !m.name.is_empty() => m.name.clone(),
            _ => self.url.as_deref().map(display_name).unwrap_or_default(),
        }
    }

    /// What Add Package (last step) adds.
    pub fn request(&self) -> Option<AddPackageRequest> {
        if self.step != Step::Products {
            return None;
        }
        let targets = self.targets();
        let products = self
            .rows
            .iter()
            .filter_map(|row| Some((row.product.name.clone(), targets.get(row.target?)?.0.clone())))
            .collect();
        Some(AddPackageRequest {
            source: self.source.clone()?,
            project: self.projects.get(self.project_ix).map(|(p, _)| p.clone()).filter(|_| self.kind.is_xcode()),
            products,
        })
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = self.request() else { return };
        self.workspace.update(cx, |ws, cx| ws.finish_add_package(request, window, cx)).ok();
    }

    fn confirm(&mut self, _: &PickerConfirm, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Search if self.refs.is_some() => self.verify(cx),
            Step::Search => self.search(window, cx),
            Step::Verifying => {}
            Step::Products => self.finish(window, cx),
        }
    }

    fn dismiss(&mut self, _: &PickerDismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn add_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| ws.prompt_add_local_package(window, cx)).ok();
    }

    /// Edits the sheet as its controls would, for the control channel:
    /// `search <text>`, `rule major|minor|range|exact|branch|commit`,
    /// `version|upper|branch|commit <value>`, `project <name>`, `next` (Add
    /// Package on the first step), `target <product> <target>|None`, `cancel`
    /// (`add`, Add Package on the last step, is the workspace's).
    pub fn set_field(&mut self, name: &str, value: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        match name {
            "search" => self.search_for(value, window, cx),
            "rule" => self.set_rule_kind(RuleKind::parse(value).ok_or(format!("no rule {value}"))?, cx),
            "version" => Self::set_input(&self.version, value, window, cx),
            "upper" => Self::set_input(&self.upper, value, window, cx),
            "branch" => Self::set_input(&self.branch, value, window, cx),
            "commit" => Self::set_input(&self.commit, value, window, cx),
            "project" => {
                self.project_ix = self
                    .projects
                    .iter()
                    .position(|(p, _)| p.file_stem().is_some_and(|s| s.to_string_lossy() == value))
                    .ok_or(format!("no project {value}"))?;
            }
            "next" => {
                if self.step != Step::Search || self.refs.is_none() {
                    return Err("nothing to add yet".into());
                }
                self.verify(cx);
                if let Some(error) = self.error.clone().filter(|_| self.step == Step::Search) {
                    return Err(error);
                }
            }
            "target" => {
                let (product, target) = value.rsplit_once(' ').ok_or("usage: target <product> <target>|None")?;
                let index = match target {
                    "None" => None,
                    _ => Some(self.targets().iter().position(|(_, n)| n == target).ok_or(format!("no target {target}"))?),
                };
                let row = self.rows.iter_mut().find(|r| r.product.name == product).ok_or(format!("no product {product}"))?;
                row.target = index;
            }
            "cancel" => cx.emit(DismissEvent),
            other => return Err(format!("unknown field {other}")),
        }
        cx.notify();
        Ok(())
    }

    pub fn summary(&self, cx: &App) -> serde_json::Value {
        let targets = self.targets();
        serde_json::json!({
            "step": format!("{:?}", self.step),
            "url": self.url,
            "fetching": self.fetching,
            "error": self.error,
            "versions": self.refs.as_ref().map(|r| r.releases.iter().map(|r| r.tag.clone()).collect::<Vec<_>>()),
            "branches": self.refs.as_ref().map(|r| r.branches.clone()),
            "rule": self.rule_kind.label(),
            "rule_summary": self.refs.as_ref().map(|_| self.rule(cx).summary()),
            "resolves_to": self.resolution(cx).ok(),
            "projects": self.projects.iter().map(|(p, _)| p.file_stem().map(|s| s.to_string_lossy().to_string())).collect::<Vec<_>>(),
            "targets": targets.iter().map(|(_, n)| n.clone()).collect::<Vec<_>>(),
            "package": (self.step == Step::Products).then(|| self.package_name()),
            "products": self.rows.iter().map(|r| format!(
                "{}: {} → {}",
                r.product.name,
                r.product.kind.label(),
                r.target.and_then(|t| targets.get(t)).map(|(_, n)| n.as_str()).unwrap_or("None")
            )).collect::<Vec<_>>(),
        })
    }
}

impl AddPackageDialog {
    fn render_rule(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let dialog = cx.entity().downgrade();
        let current = self.rule_kind;
        let rule_menu = Button::new("ap-rule").label(current.label()).small().outline().w(px(220.)).dropdown_menu(
            move |mut menu, _, _| {
                for kind in RuleKind::ALL {
                    let dialog = dialog.clone();
                    menu = menu.item(PopupMenuItem::new(kind.label()).checked(kind == current).on_click(move |_, _, cx| {
                        dialog.update(cx, |d, cx| d.set_rule_kind(kind, cx)).ok();
                    }));
                }
                menu
            },
        );
        // Versions from the repository's tags, newest first.
        let tags: Vec<String> = self.refs.as_ref().map(|r| r.releases.iter().map(|r| r.tag.clone()).collect()).unwrap_or_default();
        let branches: Vec<String> = self.refs.as_ref().map(|r| r.branches.clone()).unwrap_or_default();
        let picker = |id: &'static str, items: Vec<String>, target: Entity<InputState>, dialog: WeakEntity<Self>| {
            Button::new(id).icon(gpui_kit::assets::IconName::ChevronDown).small().ghost().disabled(items.is_empty()).dropdown_menu(
                move |mut menu, _, _| {
                    menu = menu.scrollable(true).max_h(px(280.));
                    for item in items.clone() {
                        let (target, dialog, value) = (target.clone(), dialog.clone(), item.clone());
                        menu = menu.item(PopupMenuItem::new(item).on_click(move |_, window, cx| {
                            let value = value.clone();
                            target.update(cx, |input, cx| input.set_value(value, window, cx));
                            dialog.update(cx, |_, cx| cx.notify()).ok();
                        }));
                    }
                    menu
                },
            )
        };
        let dialog = cx.entity().downgrade();
        let field = |input: &Entity<InputState>| div().w(px(120.)).child(Input::new(input).small());
        let mut controls = h_flex().gap_1p5().items_center();
        controls = match current {
            RuleKind::UpToNextMajor | RuleKind::UpToNextMinor => {
                let upper = self.rule(cx).upper_bound().map(|v| format!("< {v}")).unwrap_or_default();
                controls
                    .child(field(&self.version))
                    .child(picker("ap-versions", tags, self.version.clone(), dialog))
                    .child(div().text_color(theme.muted_foreground).child(upper))
            }
            RuleKind::Range => controls
                .child(field(&self.version))
                .child(picker("ap-versions", tags, self.version.clone(), dialog))
                .child(div().child("–"))
                .child(field(&self.upper))
                .child(div().text_color(theme.muted_foreground).child("(exclusive)")),
            RuleKind::Exact => controls.child(field(&self.version)).child(picker("ap-versions", tags, self.version.clone(), dialog)),
            RuleKind::Branch => {
                controls.child(field(&self.branch)).child(picker("ap-branches", branches, self.branch.clone(), dialog))
            }
            RuleKind::Commit => controls.child(div().w(px(300.)).child(Input::new(&self.commit).small())),
        };
        let resolution = match self.resolution(cx) {
            Ok(label) => div().text_xs().text_color(theme.muted_foreground).child(format!("Resolves to {label}")),
            Err(message) => div().text_xs().text_color(theme.danger).child(message),
        };
        v_flex()
            .gap_1p5()
            .child(h_flex().gap_2().items_center().child(div().w(px(130.)).flex_none().child("Dependency Rule")).child(rule_menu))
            .child(h_flex().gap_2().child(div().w(px(130.)).flex_none()).child(controls))
            .child(h_flex().gap_2().child(div().w(px(130.)).flex_none()).child(resolution))
    }

    fn render_products(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let targets = self.targets();
        let header = h_flex()
            .px_2()
            .py_1()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.muted_foreground)
            .border_b_1()
            .border_color(theme.border)
            .child(div().flex_1().child("Package Product"))
            .child(div().w(px(90.)).child("Kind"))
            .child(div().w(px(180.)).child("Add to Target"));
        let mut list = v_flex().child(header);
        if self.rows.is_empty() {
            list = list.child(div().p_2().text_color(theme.muted_foreground).child("This package has no products."));
        }
        for (ix, row) in self.rows.iter().enumerate() {
            let label = row.target.and_then(|t| targets.get(t)).map(|(_, n)| n.clone()).unwrap_or_else(|| "None".into());
            let dialog = cx.entity().downgrade();
            let choices = targets.clone();
            let current = row.target;
            let menu = Button::new(("ap-target", ix)).label(label).small().outline().w(px(180.)).dropdown_menu(move |mut menu, _, _| {
                let set = |target: Option<usize>, dialog: WeakEntity<Self>| {
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        dialog
                            .update(cx, |d, cx| {
                                d.rows[ix].target = target;
                                cx.notify();
                            })
                            .ok();
                    }
                };
                menu = menu.item(PopupMenuItem::new("None").checked(current.is_none()).on_click(set(None, dialog.clone())));
                for (t, (_, name)) in choices.iter().enumerate() {
                    menu = menu.item(PopupMenuItem::new(name.clone()).checked(current == Some(t)).on_click(set(Some(t), dialog.clone())));
                }
                menu
            });
            list = list.child(
                h_flex()
                    .px_2()
                    .py_1()
                    .items_center()
                    .child(div().flex_1().truncate().child(row.product.name.clone()))
                    .child(div().w(px(90.)).text_color(theme.muted_foreground).child(row.product.kind.label()))
                    .child(div().w(px(180.)).child(menu)),
            );
        }
        list
    }
}

impl Render for AddPackageDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if std::mem::take(&mut self.refocus) {
            self.focus_handle.focus(window, cx);
        }
        let title = match self.step {
            Step::Products => format!("Choose Package Products for {}:", self.package_name()),
            _ => "Add Package Dependencies".to_string(),
        };
        let mut body = v_flex().gap_2().px_4().pb_3().text_sm();
        match self.step {
            Step::Search => {
                body = body.child(Input::new(&self.search).small().cleanable(true));
                if self.fetching {
                    body = body.child(
                        h_flex()
                            .gap_1p5()
                            .text_color(theme.muted_foreground)
                            .child(Spinner::new().xsmall())
                            .child(format!("Fetching {}…", self.url.clone().unwrap_or_default())),
                    );
                }
                if let (Some(url), Some(refs)) = (self.url.clone(), self.refs.as_ref()) {
                    let count = refs.releases.len();
                    body = body.child(
                        v_flex()
                            .gap_0p5()
                            .py_1()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(display_name(&url)))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(format!(
                                "{url} · {count} version{}",
                                if count == 1 { "" } else { "s" }
                            ))),
                    );
                    body = body.child(self.render_rule(cx));
                    if self.kind.is_xcode() && self.projects.len() > 1 {
                        let dialog = cx.entity().downgrade();
                        let names: Vec<String> =
                            self.projects.iter().map(|(p, _)| p.file_stem().unwrap_or_default().to_string_lossy().to_string()).collect();
                        let current = self.project_ix;
                        let menu = Button::new("ap-project").label(names[current].clone()).small().outline().w(px(220.)).dropdown_menu(
                            move |mut menu, _, _| {
                                for (ix, name) in names.iter().enumerate() {
                                    let dialog = dialog.clone();
                                    menu = menu.item(PopupMenuItem::new(name.clone()).checked(ix == current).on_click(move |_, _, cx| {
                                        dialog
                                            .update(cx, |d, cx| {
                                                d.project_ix = ix;
                                                cx.notify();
                                            })
                                            .ok();
                                    }));
                                }
                                menu
                            },
                        );
                        body = body.child(h_flex().gap_2().items_center().child(div().w(px(130.)).flex_none().child("Add to Project")).child(menu));
                    }
                } else if !self.fetching && self.error.is_none() {
                    body = body.child(div().text_xs().text_color(theme.muted_foreground).child(
                        "Enter a package repository’s URL, or a GitHub owner/repository, and press Return.",
                    ));
                }
            }
            Step::Verifying => {
                body = body.child(
                    h_flex()
                        .gap_1p5()
                        .py_2()
                        .child(Spinner::new().xsmall())
                        .child(format!("Verifying {}…", self.url.as_deref().map(display_name).unwrap_or_default())),
                );
            }
            Step::Products => body = body.child(self.render_products(cx)),
        }
        if let Some(error) = &self.error {
            body = body.child(div().text_xs().text_color(theme.danger).child(error.clone()));
        }

        let can_add = match self.step {
            Step::Search => self.refs.is_some() && self.resolution(cx).is_ok(),
            Step::Verifying => false,
            Step::Products => true,
        };
        v_flex()
            .id("add-package")
            .key_context("AddPackageDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            .w(px(560.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_lg()
            .overflow_hidden()
            .child(div().px_4().pt_3().pb_2().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(body)
            .child(
                h_flex()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .when(self.step == Step::Search, |el| {
                        el.child(
                            Button::new("ap-local")
                                .label("Add Local…")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, window, cx| this.add_local(window, cx))),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("ap-cancel")
                            .label("Cancel")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| this.dismiss(&PickerDismiss, window, cx))),
                    )
                    .child(
                        Button::new("ap-add")
                            .label("Add Package")
                            .small()
                            .primary()
                            .disabled(!can_add)
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(&PickerConfirm, window, cx))),
                    ),
            )
    }
}

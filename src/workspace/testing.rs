//! Tests in the workspace: the tests found in source and the targets they
//! belong to, running a test or class (gutter diamonds, the Test navigator,
//! Test Selected Items), Test Again, re-running failures, test plans, and
//! the Test navigator itself.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::navigator::{empty, section_header};
use super::panels::DebugTab;
use super::test_gutter::diamond_icon;
use super::{Target, Workspace};
use crate::actions::*;
use crate::build::log::{TestResult, TestStatus};
use crate::picker::{Picker, PickerDelegate, PickerItem};
use crate::project::ProjectKind;
use crate::testing::discover::{self, Declarations};
use crate::testing::{self, TestItem, TestRequest, TestSpec, TestTarget};
use crate::xcodeproj::XcodeProject;
use crate::xcodeproj::files::{ProjectFiles, Tracking};

const ROW_HEIGHT: f32 = 24.;
/// Source files larger than this aren't searched for tests.
const MAX_SCAN_SIZE: u64 = 2 * 1024 * 1024;

/// Tests found in the project's source.
#[derive(Default)]
pub struct TestIndex {
    files: Vec<(PathBuf, Declarations)>,
    pub items: Vec<TestItem>,
    /// The test target of `items[i]`, when known.
    pub targets: Vec<Option<TestTarget>>,
    /// Test targets of Xcode project files, from `project.pbxproj`.
    file_targets: HashMap<PathBuf, TestTarget>,
    /// Every test target of the Xcode projects.
    project_targets: Vec<TestTarget>,
    pub loaded: bool,
    pub generation: u64,
}

struct Scan {
    files: Vec<(PathBuf, Declarations)>,
    file_targets: HashMap<PathBuf, TestTarget>,
    project_targets: Vec<TestTarget>,
}

/// Reads the project's source files for tests (open, edited files from
/// `overrides`) and which test targets build them. Runs in the background.
fn scan(root: &Path, files: &[PathBuf], projects: &[PathBuf], overrides: &HashMap<PathBuf, String>) -> Scan {
    let mut found = vec![];
    for rel in files {
        let path = root.join(rel);
        if !matches!(path.extension().and_then(|e| e.to_str()), Some("swift" | "m" | "mm")) {
            continue;
        }
        let path = path.canonicalize().unwrap_or(path);
        let text = match overrides.get(&path) {
            Some(text) => text.clone(),
            None => {
                if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_SCAN_SIZE {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                text
            }
        };
        if !discover::may_declare_tests(&text) {
            continue;
        }
        let decls = discover::declarations(&path, &text);
        if !decls.is_empty() {
            found.push((path, decls));
        }
    }
    let mut file_targets = HashMap::new();
    let mut project_targets = vec![];
    for project in projects {
        let Ok(text) = std::fs::read_to_string(XcodeProject::pbxproj_path(project)) else { continue };
        let Ok(index) = ProjectFiles::parse(project, &text) else { continue };
        let tests: Vec<TestTarget> = index
            .targets
            .iter()
            .filter_map(|t| match t.product_type.as_deref()? {
                "com.apple.product-type.bundle.unit-test" => Some(TestTarget::new(&t.name, false)),
                "com.apple.product-type.bundle.ui-testing" => Some(TestTarget::new(&t.name, true)),
                _ => None,
            })
            .collect();
        for (path, _) in &found {
            if !index.covers(path) || file_targets.contains_key(path) {
                continue;
            }
            let names: Vec<String> = match index.file_info(path).tracking {
                Tracking::Synchronized { targets, .. } => targets,
                Tracking::Referenced { targets, .. } => targets.into_iter().filter(|t| t.member).map(|t| t.name).collect(),
                _ => vec![],
            };
            if let Some(target) = names.iter().find_map(|n| tests.iter().find(|t| t.name == *n)) {
                file_targets.insert(path.clone(), target.clone());
            }
        }
        project_targets.extend(tests);
    }
    Scan { files: found, file_targets, project_targets }
}

impl TestIndex {
    fn resolve(&mut self) {
        self.items = discover::resolve(&self.files);
        self.generation += 1;
    }
}

/// A row of the Test navigator.
#[derive(Clone, Debug)]
enum NavRow {
    Target { name: String, items: Vec<usize>, key: String },
    Item { ix: usize, depth: usize, key: Option<String> },
    OtherHeader,
    Other(usize),
}

/// Test state the workspace keeps: the index, cached statuses and the
/// navigator's tree.
#[derive(Default)]
pub struct TestsState {
    pub index: TestIndex,
    statuses: Option<(u64, u64, Rc<Vec<Option<TestStatus>>>)>,
    pub collapsed: HashSet<String>,
    pub selected: Option<usize>,
    rows: Rc<Vec<NavRow>>,
    rows_key: Option<(u64, u64, usize, Option<String>)>,
    scroll: UniformListScrollHandle,
    task: Option<Task<()>>,
    reparse: HashMap<PathBuf, Task<()>>,
}

impl Workspace {
    // ------------------------------------------------------------------
    // Finding tests

    /// Re-reads every source file for tests, in the background.
    pub(super) fn refresh_tests(&mut self, cx: &mut Context<Self>) {
        let root = self.project.root.clone();
        let files = self.files.clone();
        let projects = self.xcode_projects.clone();
        let overrides: HashMap<PathBuf, String> =
            self.buffers.iter().filter(|b| b.dirty).map(|b| (b.path.clone(), b.editor.read(cx).text().to_string())).collect();
        self.tests.task = Some(cx.spawn(async move |this, cx| {
            let (scan, items) = cx
                .background_spawn(async move {
                    let scan = scan(&root, &files, &projects, &overrides);
                    let items = discover::resolve(&scan.files);
                    (scan, items)
                })
                .await;
            this.update(cx, |this, cx| {
                let index = &mut this.tests.index;
                index.files = scan.files;
                index.file_targets = scan.file_targets;
                index.project_targets = scan.project_targets;
                index.items = items;
                index.loaded = true;
                index.generation += 1;
                this.assign_test_targets(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Re-reads one edited file's tests a moment after typing stops, so the
    /// diamonds follow the text.
    pub(super) fn schedule_test_reparse(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !matches!(path.extension().and_then(|e| e.to_str()), Some("swift" | "m" | "mm")) {
            return;
        }
        let Some(text) = self.buffers.iter().find(|b| b.path == path).map(|b| b.editor.read(cx).text().to_string()) else {
            return;
        };
        let path = path.to_path_buf();
        let key = path.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let decls = cx
                .background_spawn({
                    let path = path.clone();
                    async move { discover::declarations(&path, &text) }
                })
                .await;
            this.update(cx, |this, cx| {
                this.tests.reparse.remove(&path);
                let files = &mut this.tests.index.files;
                let existing = files.iter().position(|(p, _)| *p == path);
                match (existing, decls.is_empty()) {
                    (Some(ix), true) => {
                        files.remove(ix);
                    }
                    (Some(ix), false) if files[ix].1 == decls => return,
                    (Some(ix), false) => files[ix].1 = decls,
                    (None, false) => files.push((path.clone(), decls)),
                    (None, true) => return,
                }
                this.tests.index.resolve();
                this.assign_test_targets(cx);
                cx.notify();
            })
            .ok();
        });
        self.tests.reparse.insert(key, task);
    }

    /// The test target that builds `path`.
    fn test_target_for(&self, path: &Path, cx: &App) -> Option<TestTarget> {
        let index = &self.tests.index;
        match &self.project.kind {
            ProjectKind::SwiftPackage => {
                let rel = path.strip_prefix(&self.project.root).ok()?;
                let listed = self.build.read(cx).schemes.test_targets.iter().find(|(_, dir)| rel.starts_with(dir));
                let name = match listed {
                    Some((name, _)) => name.clone(),
                    // The package layout's default: Tests/<Target>/….
                    None => {
                        let mut parts = rel.components();
                        (parts.next()?.as_os_str() == "Tests").then_some(())?;
                        parts.next()?.as_os_str().to_string_lossy().to_string()
                    }
                };
                Some(TestTarget::new(&name, false))
            }
            _ => index.file_targets.get(path).cloned().or_else(|| {
                // A project with one unit test target builds every test file.
                let unit: Vec<&TestTarget> = index.project_targets.iter().filter(|t| !t.ui).collect();
                (unit.len() == 1).then(|| unit[0].clone())
            }),
        }
    }

    pub(super) fn assign_test_targets(&mut self, cx: &App) {
        let targets: Vec<Option<TestTarget>> = self.tests.index.items.iter().map(|i| self.test_target_for(&i.file, cx)).collect();
        if targets != self.tests.index.targets {
            self.tests.index.targets = targets;
            self.tests.index.generation += 1;
        }
    }

    /// The last result of every test item, rolled up to classes and suites.
    pub(super) fn test_statuses(&mut self, cx: &App) -> Rc<Vec<Option<TestStatus>>> {
        let build = self.build.read(cx);
        let key = (self.tests.index.generation, build.tests_generation);
        if let Some((index, results, statuses)) = &self.tests.statuses
            && (*index, *results) == key
        {
            return statuses.clone();
        }
        let modules: Vec<Option<String>> = self.tests.index.targets.iter().map(|t| t.as_ref().map(|t| t.module.clone())).collect();
        let statuses = Rc::new(testing::statuses(&self.tests.index.items, &modules, &build.tests));
        self.tests.statuses = Some((key.0, key.1, statuses.clone()));
        statuses
    }

    // ------------------------------------------------------------------
    // Running

    /// Saves, shows the build log and runs `request`, as Product ▸ Test does.
    pub fn run_tests(&mut self, request: TestRequest, window: &mut Window, cx: &mut Context<Self>) {
        self.save_all(window, cx);
        self.index_refresh = None;
        if self.build.read(cx).log.is_empty() {
            self.show_debug_area = true;
        }
        self.debug_tab = DebugTab::BuildLog;
        self.build.update(cx, |b, cx| b.test(request, cx));
        cx.notify();
    }

    /// Whether running `items` (all items when `None`) needs Swift Testing
    /// to run serially to tell results apart.
    fn needs_serial(&self, items: Option<&[usize]>) -> bool {
        let index = &self.tests.index;
        let module = |ix: usize| index.targets.get(ix).and_then(|t| t.as_ref()).map(|t| t.module.as_str());
        let all: Vec<(&TestItem, Option<&str>)> = index.items.iter().enumerate().map(|(ix, i)| (i, module(ix))).collect();
        let Some(selection) = items else { return testing::needs_serial(&all) };
        // A selected test's result is ambiguous when a test anywhere in its
        // module has the same name, even if that one doesn't run.
        let selected: Vec<usize> = (0..index.items.len())
            .filter(|&ix| selection.iter().any(|&s| index.items[s].contains(&index.items[ix]) && module(s) == module(ix)))
            .collect();
        let names: HashSet<(Option<&str>, &str)> =
            selected.iter().filter_map(|&ix| Some((module(ix), index.items[ix].name.as_deref()?))).collect();
        let relevant: Vec<(&TestItem, Option<&str>)> = all
            .into_iter()
            .filter(|(i, m)| i.name.as_deref().is_some_and(|n| names.contains(&(*m, n))))
            .collect();
        testing::needs_serial(&relevant)
    }

    /// Product ▸ Test (⌘U): everything the scheme and test plan include.
    pub(super) fn test_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let request = TestRequest { only: vec![], serial: self.needs_serial(None) };
        self.run_tests(request, window, cx);
    }

    /// Runs test items (functions, classes or suites).
    pub fn run_test_items(&mut self, items: Vec<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let index = &self.tests.index;
        let mut only = vec![];
        for &ix in &items {
            let Some(item) = index.items.get(ix) else { continue };
            let target = index.targets.get(ix).cloned().flatten();
            let destination_is_simulator = self.build.read(cx).destination.as_ref().is_some_and(|d| d.platform.is_simulator());
            if target.is_none() && self.project.kind.is_xcode() && !destination_is_simulator {
                let name = item.file.file_name().unwrap_or_default().to_string_lossy().to_string();
                self.notify_error(format!("Couldn’t tell which test target builds {name}."), window, cx);
                return;
            }
            only.push(TestSpec::for_item(item, target));
        }
        if only.is_empty() {
            return;
        }
        let serial = self.needs_serial(Some(&items));
        self.run_tests(TestRequest { only, serial }, window, cx);
    }

    /// Runs a whole test target.
    fn run_test_target(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let index = &self.tests.index;
        let target = index.targets.iter().flatten().find(|t| t.name == name).cloned();
        let only = vec![TestSpec { target, components: vec![], framework: None }];
        let items: Vec<usize> = (0..index.items.len()).filter(|&ix| index.targets[ix].as_ref().is_some_and(|t| t.name == name)).collect();
        let serial = self.needs_serial(Some(&items));
        self.run_tests(TestRequest { only, serial }, window, cx);
    }

    /// The innermost test (or else class or suite) around the cursor.
    fn test_at_cursor(&self, cx: &App) -> Option<usize> {
        let buffer = self.active_buffer()?;
        let line = buffer.editor.read(cx).cursor_position().line;
        self.tests
            .index
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.file == buffer.path && i.start_line <= line && line <= i.end_line)
            .min_by_key(|(_, i)| (i.end_line - i.start_line, !i.is_function()))
            .map(|(ix, _)| ix)
    }

    /// Product ▸ Perform Action ▸ Test Selected Items (⌃⌥⌘U): the test or
    /// class at the cursor, or the Test navigator's selection.
    pub(super) fn test_selected_items(&mut self, _: &TestSelectedItems, window: &mut Window, cx: &mut Context<Self>) {
        let editor_focused = self.active_editor().is_some_and(|e| e.read(cx).focus_handle(cx).contains_focused(window, cx));
        let from_navigator = (self.show_navigator && self.navigator == super::NavigatorTab::Tests && !editor_focused)
            .then_some(self.tests.selected)
            .flatten();
        match from_navigator.or_else(|| self.test_at_cursor(cx)) {
            Some(ix) => self.run_test_items(vec![ix], window, cx),
            None => window.push_notification(Notification::info("No test at the cursor."), cx),
        }
    }

    /// Product ▸ Perform Action ▸ Test Again (⌃⌥⌘G): the last test run's
    /// selection.
    pub(super) fn test_again(&mut self, _: &TestAgain, window: &mut Window, cx: &mut Context<Self>) {
        match self.build.read(cx).last_test.clone() {
            Some(request) => self.run_tests(request, window, cx),
            None => self.test_all(window, cx),
        }
    }

    /// Runs the tests that failed last.
    pub(super) fn rerun_failed_tests(&mut self, _: &RerunFailedTests, window: &mut Window, cx: &mut Context<Self>) {
        let statuses = self.test_statuses(cx);
        let failed: Vec<usize> = self
            .tests
            .index
            .items
            .iter()
            .enumerate()
            .filter(|(ix, i)| i.is_function() && statuses.get(*ix).copied().flatten() == Some(TestStatus::Failed))
            .map(|(ix, _)| ix)
            .collect();
        let mut request = TestRequest { only: vec![], serial: self.needs_serial(Some(&failed)) };
        for &ix in &failed {
            request.only.push(TestSpec::for_item(&self.tests.index.items[ix], self.tests.index.targets[ix].clone()));
        }
        // Failures of tests zcode didn't find in source (XCTest names them
        // with their module).
        let others: Vec<TestSpec> = self
            .unmatched_results(cx)
            .into_iter()
            .filter(|r| r.status == TestStatus::Failed && r.module.is_some() && !r.suites.is_empty())
            .map(|r| TestSpec {
                target: r.module.as_deref().map(|m| TestTarget::new(m, false)),
                components: r.suites.iter().cloned().chain([r.name.clone()]).collect(),
                framework: None,
            })
            .collect();
        request.only.extend(others);
        if request.only.is_empty() {
            window.push_notification(Notification::info("No failed tests."), cx);
            return;
        }
        self.run_tests(request, window, cx);
    }

    /// Results that belong to no test found in source.
    fn unmatched_results(&mut self, cx: &App) -> Vec<TestResult> {
        let results = self.build.read(cx).tests.clone();
        let index = &self.tests.index;
        let modules: Vec<Option<String>> = index.targets.iter().map(|t| t.as_ref().map(|t| t.module.clone())).collect();
        results
            .into_iter()
            .filter(|r| {
                let alone = std::slice::from_ref(r);
                !testing::statuses(&index.items, &modules, alone).iter().zip(&index.items).any(|(s, i)| s.is_some() && i.is_function())
            })
            .collect()
    }

    pub(super) fn choose_test_plan(&mut self, _: &ChooseTestPlan, window: &mut Window, cx: &mut Context<Self>) {
        let delegate = TestPlanDelegate::new(self.build.clone(), cx);
        let picker = cx.new(|cx| Picker::new(delegate, window, cx));
        self.show_modal(picker, window, cx);
    }

    // ------------------------------------------------------------------
    // The Test navigator

    fn build_test_rows(&mut self, cx: &App) {
        let build = self.build.read(cx);
        let key = (self.tests.index.generation, build.tests_generation, self.tests.collapsed.len(), build.active_test_plan().map(|p| p.name.clone()));
        if self.tests.rows_key.as_ref() == Some(&key) {
            return;
        }
        let collapsed_key = self.tests.collapsed.iter().cloned().collect::<Vec<_>>();
        let index = &self.tests.index;
        // Targets in order of appearance; within one, alphabetical by suite
        // path with each class or suite before its contents.
        let mut targets: Vec<(String, Vec<usize>)> = vec![];
        for (ix, _) in index.items.iter().enumerate() {
            let name = index.targets[ix].as_ref().map(|t| t.name.clone()).unwrap_or_else(|| "Tests".into());
            match targets.iter_mut().find(|(n, _)| *n == name) {
                Some((_, items)) => items.push(ix),
                None => targets.push((name, vec![ix])),
            }
        }
        targets.sort_by(|a, b| a.0.cmp(&b.0));
        let sort_key = |item: &TestItem| -> Vec<String> {
            item.suite.iter().cloned().chain([match &item.name {
                Some(name) => format!("\u{1}{name}"),
                None => String::new(),
            }]).collect()
        };
        let mut rows = vec![];
        for (name, mut items) in targets {
            items.sort_by_key(|&ix| sort_key(&index.items[ix]));
            let target_key = format!("{name}\u{0}");
            rows.push(NavRow::Target { name: name.clone(), items: items.clone(), key: target_key.clone() });
            if collapsed_key.contains(&target_key) {
                continue;
            }
            let mut hidden: Option<Vec<String>> = None;
            for ix in items {
                let item = &index.items[ix];
                if let Some(prefix) = &hidden {
                    if item.suite.starts_with(prefix) {
                        continue;
                    }
                    hidden = None;
                }
                let key = (!item.is_function()).then(|| format!("{name}\u{0}{}", item.suite.join("/")));
                if key.as_ref().is_some_and(|k| collapsed_key.contains(k)) {
                    hidden = Some(item.suite.clone());
                }
                rows.push(NavRow::Item { ix, depth: item.suite.len() + item.is_function() as usize, key });
            }
        }
        let unmatched = {
            let results = build.tests.clone();
            let modules: Vec<Option<String>> = index.targets.iter().map(|t| t.as_ref().map(|t| t.module.clone())).collect();
            results
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    !testing::statuses(&index.items, &modules, std::slice::from_ref(*r))
                        .iter()
                        .zip(&index.items)
                        .any(|(s, i)| s.is_some() && i.is_function())
                })
                .map(|(ix, _)| ix)
                .collect::<Vec<_>>()
        };
        if !unmatched.is_empty() {
            rows.push(NavRow::OtherHeader);
            rows.extend(unmatched.into_iter().map(NavRow::Other));
        }
        self.tests.rows = Rc::new(rows);
        self.tests.rows_key = Some(key);
    }

    fn toggle_test_group(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.tests.collapsed.remove(&key) {
            self.tests.collapsed.insert(key);
        }
        self.tests.rows_key = None;
        cx.notify();
    }

    fn select_test(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tests.selected = Some(ix);
        let Some(item) = self.tests.index.items.get(ix) else { return };
        let (file, line) = (item.file.clone(), item.line);
        self.open_file(file, Some(Target::Line { line, column: 0 }), true, window, cx);
    }

    pub(super) fn render_test_navigator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        self.build_test_rows(cx);
        let statuses = self.test_statuses(cx);
        let build = self.build.read(cx);
        let busy = build.phase.is_busy();
        let plans: Vec<String> = build.test_plans.iter().map(|p| p.name.clone()).collect();
        let active_plan = build.active_test_plan().map(|p| p.name.clone());
        let tests = self.tests.index.items.iter().filter(|i| i.is_function()).count();
        let failed = statuses
            .iter()
            .zip(&self.tests.index.items)
            .filter(|(s, i)| i.is_function() && **s == Some(TestStatus::Failed))
            .count();
        let summary = match (tests, failed) {
            (0, _) => "Tests".to_string(),
            (n, 0) => format!("{n} tests"),
            (n, f) => format!("{n} tests, {f} failed"),
        };
        let build_entity = self.build.clone();
        let header = h_flex()
            .justify_between()
            .pr_1()
            .child(section_header(summary, cx))
            .child(
                h_flex()
                    .gap_0p5()
                    .when_some(active_plan.clone().filter(|_| !plans.is_empty()), |el, plan| {
                        el.child(
                            Button::new("test-plan")
                                .label(plan)
                                .icon(IconName::FlaskConical)
                                .ghost()
                                .xsmall()
                                .tooltip("Test Plan")
                                .dropdown_menu(move |menu, _, cx| {
                                    let state = build_entity.read(cx);
                                    let active = state.active_test_plan().map(|p| p.name.clone());
                                    let mut menu = menu.label("Test Plans");
                                    for plan in state.test_plans.clone() {
                                        let build = build_entity.clone();
                                        let name = plan.name.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(if plan.is_default { format!("{} (Default)", plan.name) } else { plan.name.clone() })
                                                .checked(active.as_ref() == Some(&plan.name))
                                                .on_click(move |_, _, cx| build.update(cx, |b, cx| b.set_test_plan(Some(name.clone()), cx))),
                                        );
                                    }
                                    menu
                                }),
                        )
                    })
                    .child(
                        Button::new("run-tests")
                            .icon(IconName::Play)
                            .ghost()
                            .xsmall()
                            .disabled(busy)
                            .tooltip("Test (⌘U)")
                            .on_click(cx.listener(|this, _, window, cx| this.test_all(window, cx))),
                    ),
            );
        let list = if self.tests.rows.is_empty() {
            empty(
                if self.tests.index.loaded {
                    "No tests found. XCTest cases, Swift Testing suites and @Test functions appear here."
                } else {
                    "Finding tests…"
                },
                cx,
            )
            .into_any_element()
        } else {
            uniform_list("test-rows", self.tests.rows.len(), cx.processor(Self::render_test_rows))
                .track_scroll(&self.tests.scroll)
                .size_full()
                .into_any_element()
        };
        v_flex()
            .size_full()
            .child(header)
            .child(div().h(px(1.)).bg(theme.border))
            .child(div().flex_1().min_h_0().child(list))
    }

    fn render_test_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        let statuses = self.test_statuses(cx);
        let rows = self.tests.rows.clone();
        let build = self.build.read(cx);
        let plan = build.active_test_plan().map(|p| p.contents.clone());
        let results = build.tests.clone();
        let base = |id: SharedString, depth: usize| {
            h_flex()
                .id(id)
                .group("test-row")
                .h(px(ROW_HEIGHT))
                .w_full()
                .pl(px(6. + depth as f32 * 14.))
                .pr_1()
                .gap_1p5()
                .text_sm()
                .hover(|el| el.bg(theme.sidebar_accent))
        };
        let chevron = |collapsed: bool| Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).xsmall().text_color(theme.muted_foreground);
        let play = |id: SharedString| {
            Button::new(id)
                .icon(IconName::Play)
                .ghost()
                .xsmall()
                .tooltip("Run")
        };
        let mut out = vec![];
        for row_ix in range {
            let Some(row) = rows.get(row_ix).cloned() else { continue };
            let element = match row {
                NavRow::Target { name, items, key } => {
                    let collapsed = self.tests.collapsed.contains(&key);
                    let failed = items.iter().any(|&ix| statuses.get(ix).copied().flatten() == Some(TestStatus::Failed) && self.tests.index.items[ix].is_function());
                    let run_name = name.clone();
                    base(SharedString::from(format!("test-target-{name}")), 0)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(chevron(collapsed))
                        .child(Icon::new(IconName::Package).small().text_color(if failed { theme.danger } else { theme.muted_foreground }))
                        .child(div().flex_1().truncate().child(name.clone()))
                        .child(
                            play(SharedString::from(format!("run-target-{name}")))
                                .invisible()
                                .group_hover("test-row", |s| s.visible())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.run_test_target(&run_name, window, cx);
                                })),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_test_group(key.clone(), cx)))
                        .into_any_element()
                }
                NavRow::Item { ix, depth, key } => {
                    let item = &self.tests.index.items[ix];
                    let target = self.tests.index.targets[ix].clone();
                    let included = match (&plan, &target) {
                        (Some(plan), Some(target)) if !plan.targets.is_empty() => plan.includes(&target.name, &item.components()),
                        _ => true,
                    };
                    let selected = self.tests.selected == Some(ix);
                    let title = item.title();
                    let menu_title = title.clone();
                    let collapsed = key.as_ref().is_some_and(|k| self.tests.collapsed.contains(k));
                    base(SharedString::from(format!("test-item-{ix}")), depth)
                        .when(selected, |el| el.bg(theme.sidebar_accent))
                        .when(!included, |el| el.text_color(theme.muted_foreground))
                        .map(|el| match key.clone() {
                            Some(key) => el.child(
                                div()
                                    .id(SharedString::from(format!("test-toggle-{ix}")))
                                    .child(chevron(collapsed))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_test_group(key.clone(), cx);
                                    })),
                            ),
                            None => el.child(div().w(px(12.))),
                        })
                        .child(diamond_icon(statuses.get(ix).copied().flatten(), cx))
                        .child(div().flex_1().truncate().when(!item.is_function(), |el| el.font_weight(FontWeight::MEDIUM)).child(title))
                        .child(
                            play(SharedString::from(format!("run-test-{ix}")))
                                .when(!selected, |b| b.invisible().group_hover("test-row", |s| s.visible()))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.run_test_items(vec![ix], window, cx);
                                })),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| this.select_test(ix, window, cx)))
                        .context_menu({
                            let workspace = cx.weak_entity();
                            move |menu, _, _| {
                                let workspace = workspace.clone();
                                menu.item(PopupMenuItem::new(format!("Run “{menu_title}”")).on_click(move |_, window, cx| {
                                    workspace.update(cx, |this, cx| this.run_test_items(vec![ix], window, cx)).ok();
                                }))
                            }
                        })
                        .into_any_element()
                }
                NavRow::OtherHeader => div().h(px(ROW_HEIGHT)).child(section_header("Other Results", cx)).into_any_element(),
                NavRow::Other(result_ix) => {
                    let Some(result) = results.get(result_ix) else { continue };
                    let name = match &result.module {
                        Some(module) => format!("{module}.{}", result.full_name()),
                        None => result.full_name(),
                    };
                    base(SharedString::from(format!("test-other-{result_ix}")), 1)
                        .child(diamond_icon(Some(result.status), cx))
                        .child(div().flex_1().truncate().child(name))
                        .into_any_element()
                }
            };
            out.push(element);
        }
        out
    }

    /// Test state for the debug harness.
    pub(super) fn tests_summary(&mut self, cx: &App) -> serde_json::Value {
        let statuses = self.test_statuses(cx);
        let index = &self.tests.index;
        let label = |ix: usize| {
            let item = &index.items[ix];
            let target = index.targets[ix].as_ref().map(|t| format!("{}/", t.name)).unwrap_or_default();
            format!("{target}{}", item.components().join("/"))
        };
        let status = |ix: usize| statuses.get(ix).copied().flatten().map(|s| format!("{s:?}")).unwrap_or_else(|| "NotRun".into());
        let build = self.build.read(cx);
        serde_json::json!({
            "loaded": index.loaded,
            "items": (0..index.items.len()).map(|ix| format!("{} {}", status(ix), label(ix))).collect::<Vec<_>>(),
            "gutter": self.gutter.painted.borrow().iter().map(|(bounds, d)| serde_json::json!({
                "line": d.line + 1,
                "test": label(d.item),
                "status": status(d.item),
                "x": f32::from(bounds.center().x),
                "y": f32::from(bounds.center().y),
            })).collect::<Vec<_>>(),
            "plans": build.test_plans.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            "plan": build.active_test_plan().map(|p| p.name.clone()),
            "last": build.last_test.as_ref().map(|r| r.label().unwrap_or_else(|| "All".into())),
            "selected": self.tests.selected.map(label),
        })
    }

    /// Runs the test the harness names (`Target/Class/test` or
    /// `Class/test`), as its Test navigator play button does.
    pub fn run_test_named(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let index = &self.tests.index;
        let found = (0..index.items.len()).find(|&ix| {
            let components = index.items[ix].components().join("/");
            let full = index.targets[ix].as_ref().map(|t| format!("{}/{components}", t.name));
            components == name || full.as_deref() == Some(name)
        });
        if let Some(ix) = found {
            self.run_test_items(vec![ix], window, cx);
            return Ok(());
        }
        if index.targets.iter().flatten().any(|t| t.name == name) {
            self.run_test_target(name, window, cx);
            return Ok(());
        }
        Err(format!("no test named {name}"))
    }
}

/// Product ▸ Test Plan: choose the plan ⌘U and Test Again run.
pub struct TestPlanDelegate {
    build: Entity<crate::build::BuildController>,
    plans: Vec<(String, bool)>,
    current: Option<String>,
    items: Vec<PickerItem>,
    matches: Vec<usize>,
}

impl TestPlanDelegate {
    pub fn new(build: Entity<crate::build::BuildController>, cx: &App) -> Self {
        let state = build.read(cx);
        Self {
            plans: state.test_plans.iter().map(|p| (p.name.clone(), p.is_default)).collect(),
            current: state.active_test_plan().map(|p| p.name.clone()),
            build,
            items: vec![],
            matches: vec![],
        }
    }
}

impl PickerDelegate for TestPlanDelegate {
    fn placeholder(&self) -> SharedString {
        "Choose Test Plan".into()
    }

    fn items(&self) -> &[PickerItem] {
        &self.items
    }

    fn empty_message(&self) -> SharedString {
        "The scheme has no test plans".into()
    }

    fn update_matches(&mut self, query: String, _: &mut Window, _: &mut Context<Picker<Self>>) {
        let names: Vec<String> = self.plans.iter().map(|(n, _)| n.clone()).collect();
        let found = crate::fuzzy::fuzzy_match(&query, &names, false, 100);
        self.matches = found.iter().map(|m| m.index).collect();
        self.items = found
            .into_iter()
            .map(|m| {
                let (name, is_default) = &self.plans[m.index];
                PickerItem {
                    label: name.clone().into(),
                    detail: is_default.then(|| "Default".into()),
                    icon: Some(IconName::FlaskConical),
                    positions: m.positions,
                    checked: self.current.as_ref() == Some(name),
                    ..Default::default()
                }
            })
            .collect();
    }

    fn confirm(&mut self, index: usize, _: &mut Window, cx: &mut Context<Picker<Self>>) -> bool {
        let name = self.plans[self.matches[index]].0.clone();
        self.build.update(cx, |build, cx| build.set_test_plan(Some(name), cx));
        true
    }
}

//! Tests: finding them in source (`discover`), test plans (`plans`),
//! choosing which to run, and matching the results the runners print back
//! to the tests they belong to (for the gutter diamonds and the Test
//! navigator).
//!
//! A selection is a list of [`TestSpec`]s, each a whole target, a class or
//! suite, or one test. Each runner spells it its own way:
//! * `xcodebuild test -only-testing:Target/Class/testFoo` (and `Suite/foo()`),
//! * the in-simulator runner's XCTest scope, `Module.Class/testFoo` or
//!   `Module.Suite/foo()` (XCTest hands Swift Testing its share),
//! * `swift test --filter`, a regular expression over `Module.Class/testFoo`
//!   and `Module.Suite/foo()`.

pub mod discover;
pub mod plans;

use std::collections::HashMap;

pub use discover::{Framework, TestItem};

use crate::build::log::{TestResult, TestStatus};

/// The test target (bundle) tests belong to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TestTarget {
    pub name: String,
    pub module: String,
    /// A UI test bundle, which only `xcodebuild test` can run.
    pub ui: bool,
}

impl TestTarget {
    pub fn new(name: &str, ui: bool) -> Self {
        Self { name: name.to_string(), module: module_name(name), ui }
    }
}

/// A target's module name as Xcode derives it (`PRODUCT_MODULE_NAME`
/// defaults to the product name made a C identifier).
pub fn module_name(target: &str) -> String {
    let mut out: String = target.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// One thing to run: a whole target, a class or suite, or a single test.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TestSpec {
    pub target: Option<TestTarget>,
    /// Suites (or the XCTest class), then the test; empty for the whole target.
    pub components: Vec<String>,
    pub framework: Option<Framework>,
}

fn strip_parens(s: &str) -> &str {
    s.strip_suffix("()").unwrap_or(s)
}

impl TestSpec {
    pub fn for_item(item: &TestItem, target: Option<TestTarget>) -> Self {
        Self { target, components: item.components(), framework: Some(item.framework) }
    }

    pub fn module(&self) -> Option<&str> {
        self.target.as_ref().map(|t| t.module.as_str())
    }

    fn framework(&self) -> Framework {
        self.framework.unwrap_or_else(|| match self.components.last() {
            Some(last) if last.ends_with(')') => Framework::SwiftTesting,
            _ => Framework::XCTest,
        })
    }

    /// `Class/testFoo`, `Suite/foo()`: how Xcode names the test.
    pub fn label(&self) -> String {
        match (self.components.is_empty(), &self.target) {
            (true, Some(target)) => target.name.clone(),
            _ => self.components.join("/"),
        }
    }

    /// The `xcodebuild -only-testing:` identifier.
    pub fn xcodebuild_identifier(&self) -> Option<String> {
        let target = self.target.as_ref()?;
        Some(std::iter::once(target.name.clone()).chain(self.components.iter().cloned()).collect::<Vec<_>>().join("/"))
    }

    /// The in-process XCTest scope entry; `None` runs the whole bundle.
    /// XCTest takes `Module.Class/testFoo`, Swift Testing (through XCTest)
    /// `Module.Suite/foo()` and `Module.foo()`.
    pub fn scope_identifier(&self) -> Option<String> {
        if self.components.is_empty() {
            return None;
        }
        let path = self.components.join("/");
        Some(match self.module() {
            Some(module) => format!("{module}.{path}"),
            None => path,
        })
    }

    /// A `swift test --filter` expression. Test IDs look like
    /// `Module.Class/testFoo` (XCTest) and `Module.Suite/foo()` (Swift
    /// Testing, which may append more after the name, so only XCTest names
    /// are anchored at the end).
    pub fn swiftpm_filter(&self) -> String {
        let prefix = match self.module() {
            Some(module) => format!("^{}\\.", regex::escape(module)),
            None => String::from("(^|\\.)"),
        };
        let Some(last) = self.components.last() else { return prefix };
        let path = regex::escape(&self.components.join("/"));
        match self.framework() {
            Framework::XCTest if self.components.len() >= 2 => {
                let class = regex::escape(&self.components[..self.components.len() - 1].join("/"));
                format!("{prefix}{class}/{}$", regex::escape(strip_parens(last)))
            }
            Framework::SwiftTesting if last.ends_with(')') => format!("{prefix}{path}"),
            // A class or suite.
            _ => format!("{prefix}{path}(/|$)"),
        }
    }

    /// Whether `result` reports a test this spec runs.
    pub fn covers(&self, result: &TestResult) -> bool {
        if let (Some(module), Some(result_module)) = (self.module(), result.module.as_deref())
            && module != result_module
        {
            return false;
        }
        let path: Vec<&str> = result.suites.iter().map(String::as_str).chain([result.name.as_str()]).collect();
        if self.components.is_empty() {
            return true;
        }
        if result.suites.is_empty() {
            // Only the test's name is known, which a class or suite can't claim.
            let is_test = self.components.len() >= 2 || self.components.last().is_some_and(|l| l.ends_with(')'));
            return is_test && self.components.last().map(|l| strip_parens(l)) == Some(strip_parens(&result.name));
        }
        self.components.len() <= path.len() && self.components.iter().zip(&path).all(|(a, b)| strip_parens(a) == strip_parens(b))
    }
}

/// What to test: everything the scheme (and test plan) includes, or a
/// selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TestRequest {
    pub only: Vec<TestSpec>,
    /// Run Swift Testing tests one at a time, so the order of its output
    /// tells which suite each result belongs to. Needed when tests in
    /// different suites share a name, because its console output names only
    /// the test.
    pub serial: bool,
}

impl TestRequest {
    pub fn all() -> Self {
        Self::default()
    }

    pub fn is_all(&self) -> bool {
        self.only.is_empty()
    }

    /// "Class/testFoo" for one test, "3 tests" for several.
    pub fn label(&self) -> Option<String> {
        match self.only.as_slice() {
            [] => None,
            [one] => Some(one.label()),
            many => Some(format!("{} tests", many.len())),
        }
    }

    /// `-only-testing:` arguments for xcodebuild.
    pub fn xcodebuild_args(&self) -> Vec<String> {
        self.only.iter().filter_map(TestSpec::xcodebuild_identifier).map(|id| format!("-only-testing:{id}")).collect()
    }

    /// `--filter` arguments for `swift test`.
    pub fn swiftpm_args(&self) -> Vec<String> {
        let mut args: Vec<String> = self.only.iter().flat_map(|s| ["--filter".to_string(), s.swiftpm_filter()]).collect();
        if self.serial {
            args.push("--no-parallel".into());
        }
        args
    }

    /// Whether the selection includes tests from the bundle `module` (or
    /// target `name`), and the scope entries for them (empty: the whole
    /// bundle).
    pub fn scope_for_bundle(&self, name: &str, module: &str) -> Option<Vec<String>> {
        if self.only.is_empty() {
            return Some(vec![]);
        }
        let mine: Vec<&TestSpec> = self
            .only
            .iter()
            .filter(|s| s.target.as_ref().is_none_or(|t| t.name == name || t.module == module))
            .collect();
        if mine.is_empty() {
            return None;
        }
        if mine.iter().any(|s| s.components.is_empty()) {
            return Some(vec![]);
        }
        Some(
            mine.iter()
                .filter_map(|s| {
                    let mut spec = (*s).clone();
                    if spec.target.is_none() {
                        spec.target = Some(TestTarget { name: name.to_string(), module: module.to_string(), ui: false });
                    }
                    spec.scope_identifier()
                })
                .collect(),
        )
    }

    /// Whether only UI test bundles are selected.
    pub fn only_ui_tests(&self) -> bool {
        !self.only.is_empty() && self.only.iter().all(|s| s.target.as_ref().is_some_and(|t| t.ui))
    }
}

/// Whether Swift Testing tests in one module share a name across suites,
/// so their console results can't be told apart without suite order.
pub fn needs_serial(items: &[(&TestItem, Option<&str>)]) -> bool {
    let mut seen: HashMap<(Option<&str>, String), &[String]> = HashMap::new();
    for (item, module) in items {
        if item.framework != Framework::SwiftTesting || !item.is_function() {
            continue;
        }
        for key in std::iter::once(item.name.clone().unwrap_or_default()).chain(item.display_name.clone()) {
            match seen.get(&(*module, key.clone())) {
                Some(suite) if *suite != item.suite.as_slice() => return true,
                _ => {
                    seen.insert((*module, key), &item.suite);
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Results → tests

fn result_key(name: &str) -> String {
    strip_parens(name.trim_matches('"')).to_string()
}

/// How well a result's printed suites fit a test's.
fn suites_match(result: &TestResult, item: &TestItem) -> Option<bool> {
    if result.suites.is_empty() {
        return Some(false);
    }
    if result.suites.len() > item.suite.len() {
        return None;
    }
    let offset = item.suite.len() - result.suites.len();
    let all = result.suites.iter().enumerate().all(|(i, printed)| {
        let ix = offset + i;
        printed == &item.suite[ix] || item.suite_display.get(ix).and_then(|d| d.as_deref()) == Some(printed.as_str())
    });
    all.then_some(true)
}

/// The latest result for each test function in `items`, then rolled up to
/// classes and suites: failed if any test failed, passed once all passed.
/// `modules[i]` is the module of `items[i]`'s target, when known.
pub fn statuses(items: &[TestItem], modules: &[Option<String>], results: &[TestResult]) -> Vec<Option<TestStatus>> {
    let mut by_key: HashMap<String, Vec<&TestResult>> = HashMap::new();
    for result in results {
        by_key.entry(result_key(&result.name)).or_default().push(result);
    }
    let mut out: Vec<Option<TestStatus>> = vec![None; items.len()];
    // (module, framework, suite prefix) → (tests, with results, failed, all skipped)
    let mut rollup: HashMap<(Option<&str>, Framework, &[String]), (usize, usize, bool, bool)> = HashMap::new();
    for (ix, item) in items.iter().enumerate() {
        let Some(name) = &item.name else { continue };
        let module = modules.get(ix).and_then(|m| m.as_deref());
        let candidates = std::iter::once(result_key(name))
            .chain(item.display_name.clone())
            .flat_map(|key| by_key.get(&key).into_iter().flatten().copied())
            .filter(|r| match (module, r.module.as_deref()) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            });
        let mut strong = None;
        let mut weak = None;
        for result in candidates {
            match suites_match(result, item) {
                Some(true) => strong = Some(result),
                Some(false) => weak = Some(result),
                None => {}
            }
        }
        let status = strong.or(weak).map(|r| r.status);
        out[ix] = status;
        for depth in 1..=item.suite.len() {
            let entry = rollup.entry((module, item.framework, &item.suite[..depth])).or_insert((0, 0, false, true));
            entry.0 += 1;
            if let Some(status) = status {
                entry.1 += 1;
                entry.2 |= status == TestStatus::Failed;
                entry.3 &= status == TestStatus::Skipped;
            }
        }
    }
    for (ix, item) in items.iter().enumerate() {
        if item.name.is_some() {
            continue;
        }
        let module = modules.get(ix).and_then(|m| m.as_deref());
        out[ix] = rollup.get(&(module, item.framework, item.suite.as_slice())).and_then(|&(tests, known, failed, skipped)| {
            if failed {
                Some(TestStatus::Failed)
            } else if known == tests && tests > 0 {
                Some(if skipped { TestStatus::Skipped } else { TestStatus::Passed })
            } else {
                None
            }
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::log::parse_test;
    use std::path::{Path, PathBuf};

    fn items(text: &str) -> Vec<TestItem> {
        discover::resolve(&[(PathBuf::from("/p/T.swift"), discover::declarations(Path::new("/p/T.swift"), text))])
    }

    const SOURCE: &str = r#"import XCTest
import Testing
final class LegacyTests: XCTestCase {
    func testOne() {}
    func testTwo() {}
}
@Suite struct A { @Test func same() {} }
@Suite("Bee") struct B { @Test func same() {} }
@Test("Global name") func global() {}
"#;

    fn spec(items: &[TestItem], label: &str, target: &str) -> TestSpec {
        let item = items.iter().find(|i| i.components().join("/") == label).unwrap();
        TestSpec::for_item(item, Some(TestTarget::new(target, false)))
    }

    #[test]
    fn runner_identifiers() {
        let items = items(SOURCE);
        let one = spec(&items, "LegacyTests/testOne", "App Tests");
        assert_eq!(one.xcodebuild_identifier().unwrap(), "App Tests/LegacyTests/testOne");
        assert_eq!(one.scope_identifier().unwrap(), "App_Tests.LegacyTests/testOne");
        assert_eq!(one.swiftpm_filter(), r"^App_Tests\.LegacyTests/testOne$");
        let class = spec(&items, "LegacyTests", "AppTests");
        assert_eq!(class.swiftpm_filter(), r"^AppTests\.LegacyTests(/|$)");
        let same = spec(&items, "B/same()", "AppTests");
        assert_eq!(same.scope_identifier().unwrap(), "AppTests.B/same()");
        assert_eq!(same.swiftpm_filter(), r"^AppTests\.B/same\(\)");
        let global = spec(&items, "global()", "AppTests");
        assert_eq!(global.scope_identifier().unwrap(), "AppTests.global()");
        assert_eq!(global.xcodebuild_identifier().unwrap(), "AppTests/global()");
        let whole = TestSpec { target: Some(TestTarget::new("AppTests", false)), components: vec![], framework: None };
        assert_eq!(whole.scope_identifier(), None);
        assert_eq!(whole.swiftpm_filter(), r"^AppTests\.");

        let request = TestRequest { only: vec![one, same], serial: true };
        assert_eq!(request.scope_for_bundle("App Tests", "App_Tests"), Some(vec!["App_Tests.LegacyTests/testOne".to_string()]));
        assert_eq!(request.scope_for_bundle("Other", "Other"), None);
        assert_eq!(request.swiftpm_args().last().map(String::as_str), Some("--no-parallel"));
        assert_eq!(TestRequest::all().scope_for_bundle("X", "X"), Some(vec![]));
    }

    #[test]
    fn serial_only_when_names_collide() {
        let items = items(SOURCE);
        let with_modules: Vec<(&TestItem, Option<&str>)> = items.iter().map(|i| (i, Some("M"))).collect();
        assert!(needs_serial(&with_modules));
        let unique: Vec<(&TestItem, Option<&str>)> = with_modules.into_iter().filter(|(i, _)| i.suite != ["B"]).collect();
        assert!(!needs_serial(&unique));
    }

    fn result(line: &str, suites: &[&str]) -> TestResult {
        let mut r = parse_test(line).unwrap();
        if r.suites.is_empty() {
            r.suites = suites.iter().map(|s| s.to_string()).collect();
        }
        r
    }

    #[test]
    fn matches_results_to_tests() {
        let items = items(SOURCE);
        let modules = vec![Some("AppTests".to_string()); items.len()];
        let results = vec![
            result("Test Case '-[AppTests.LegacyTests testOne]' passed (0.001 seconds).", &[]),
            result("Test Case '-[AppTests.LegacyTests testTwo]' failed (0.001 seconds).", &[]),
            // Swift Testing's console names the suite only through order.
            result("✔ Test same() passed after 0.001 seconds.", &["A"]),
            result("✘ Test same() failed after 0.001 seconds with 1 issue.", &["Bee"]),
            result("✔ Test \"Global name\" passed after 0.001 seconds.", &[]),
        ];
        let statuses = statuses(&items, &modules, &results);
        let status = |label: &str| statuses[items.iter().position(|i| i.components().join("/") == label).unwrap()];
        assert_eq!(status("LegacyTests/testOne"), Some(TestStatus::Passed));
        assert_eq!(status("LegacyTests/testTwo"), Some(TestStatus::Failed));
        assert_eq!(status("LegacyTests"), Some(TestStatus::Failed));
        assert_eq!(status("A/same()"), Some(TestStatus::Passed));
        assert_eq!(status("A"), Some(TestStatus::Passed));
        assert_eq!(status("B/same()"), Some(TestStatus::Failed));
        assert_eq!(status("global()"), Some(TestStatus::Passed));

        // xcodebuild prints suites; a result from another module is ignored.
        let results = vec![
            result("Test case 'B/same()' passed on 'My Mac' (0.000 seconds)", &[]),
            result("Test Case '-[Other.LegacyTests testOne]' failed (0.001 seconds).", &[]),
        ];
        let statuses = super::statuses(&items, &modules, &results);
        let status = |label: &str| statuses[items.iter().position(|i| i.components().join("/") == label).unwrap()];
        assert_eq!(status("B/same()"), Some(TestStatus::Passed));
        assert_eq!(status("A/same()"), None);
        assert_eq!(status("LegacyTests/testOne"), None);
        assert_eq!(status("LegacyTests"), None, "not every test has a result");
    }

    #[test]
    fn specs_cover_their_results() {
        let items = items(SOURCE);
        let class = spec(&items, "LegacyTests", "AppTests");
        assert!(class.covers(&result("Test Case '-[AppTests.LegacyTests testOne]' passed (0.001 seconds).", &[])));
        assert!(!class.covers(&result("Test Case '-[Other.LegacyTests testOne]' passed (0.001 seconds).", &[])));
        let same = spec(&items, "A/same()", "AppTests");
        assert!(same.covers(&result("✔ Test same() passed after 0.001 seconds.", &["A"])));
        assert!(!same.covers(&result("✔ Test same() passed after 0.001 seconds.", &["B"])));
        assert!(same.covers(&result("✔ Test same() passed after 0.001 seconds.", &[])));
        let suite = spec(&items, "A", "AppTests");
        assert!(!suite.covers(&result("✔ Test \"Global name\" passed after 0.001 seconds.", &[])));
        assert!(suite.covers(&result("✔ Test same() passed after 0.001 seconds.", &["A"])));
    }
}

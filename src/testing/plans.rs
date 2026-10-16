//! Test plans (`.xctestplan`): which ones a scheme lists, and which tests
//! each one includes.
//!
//! Running honors a plan through the runners themselves (`xcodebuild
//! -testPlan`, and the `.xctestrun` that `build-for-testing -testPlan`
//! writes for the in-simulator runner). The contents parsed here only tell
//! the Test navigator which tests the chosen plan leaves out.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::project::ProjectKind;

#[derive(Clone, Debug, PartialEq)]
pub struct TestPlan {
    pub name: String,
    pub path: PathBuf,
    pub is_default: bool,
    pub contents: PlanContents,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanContents {
    pub targets: Vec<PlanTarget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanTarget {
    pub name: String,
    pub enabled: bool,
    /// `selectedTests`: only these run. Identifiers look like `Class`,
    /// `Class/testMethod()` or `Suite/Inner/test()`.
    pub selected: Option<Vec<String>>,
    pub skipped: Vec<String>,
}

/// Compares identifier components, ignoring an XCTest method's `()`.
fn same_component(a: &str, b: &str) -> bool {
    a.strip_suffix("()").unwrap_or(a) == b.strip_suffix("()").unwrap_or(b)
}

/// Whether `pattern` (a plan identifier) names `components` or one of its
/// enclosing classes or suites.
fn covers(pattern: &str, components: &[String]) -> bool {
    let parts: Vec<&str> = pattern.split('/').collect();
    parts.len() <= components.len() && parts.iter().zip(components).all(|(p, c)| same_component(p, c))
}

impl PlanContents {
    pub fn parse(text: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(text) else { return Self::default() };
        let strings = |v: Option<&Value>| -> Vec<String> {
            v.and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                .unwrap_or_default()
        };
        let targets = value
            .get("testTargets")
            .and_then(Value::as_array)
            .map(|targets| {
                targets
                    .iter()
                    .filter_map(|t| {
                        Some(PlanTarget {
                            name: t.get("target")?.get("name")?.as_str()?.to_string(),
                            enabled: t.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                            selected: t.get("selectedTests").map(|s| strings(Some(s))),
                            skipped: strings(t.get("skippedTests")),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { targets }
    }

    /// Whether the plan runs the test (or class/suite) `components` of
    /// `target`. A class counts while any of its tests runs.
    pub fn includes(&self, target: &str, components: &[String]) -> bool {
        let Some(plan_target) = self.targets.iter().find(|t| t.name == target) else { return false };
        if !plan_target.enabled || plan_target.skipped.iter().any(|s| covers(s, components)) {
            return false;
        }
        match &plan_target.selected {
            None => true,
            Some(selected) => selected.iter().any(|s| {
                covers(s, components) || {
                    // A selected test inside this class or suite.
                    let parts: Vec<&str> = s.split('/').collect();
                    parts.len() > components.len() && parts.iter().zip(components).all(|(p, c)| same_component(p, c))
                }
            }),
        }
    }
}

/// The scheme file for `scheme`: shared first, then the user's.
pub fn scheme_file(kind: &ProjectKind, scheme: &str) -> Option<PathBuf> {
    let containers: Vec<PathBuf> = match kind {
        ProjectKind::XcodeWorkspace(path) => {
            std::iter::once(path.clone()).chain(crate::project::xcode_projects(kind)).collect()
        }
        ProjectKind::XcodeProject(path) => vec![path.clone()],
        _ => return None,
    };
    let file = format!("{scheme}.xcscheme");
    for container in &containers {
        let shared = container.join("xcshareddata/xcschemes").join(&file);
        if shared.is_file() {
            return Some(shared);
        }
        if let Ok(users) = std::fs::read_dir(container.join("xcuserdata")) {
            for user in users.flatten() {
                let candidate = user.path().join("xcschemes").join(&file);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// The test plans a scheme lists (`<TestPlans>` in its test action), with
/// their contents.
pub fn scheme_test_plans(kind: &ProjectKind, scheme: &str) -> Vec<TestPlan> {
    let Some(file) = scheme_file(kind, scheme) else { return vec![] };
    let Ok(xml) = std::fs::read_to_string(&file) else { return vec![] };
    // `X.xcodeproj/xcshareddata/xcschemes/S.xcscheme`: "container:" paths
    // are relative to the folder holding `X.xcodeproj`.
    let container_dir = file
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "xcodeproj" || e == "xcworkspace"))
        .and_then(Path::parent)
        .unwrap_or(Path::new("/"))
        .to_path_buf();
    parse_scheme_plans(&xml, &container_dir)
        .into_iter()
        .map(|(path, is_default)| TestPlan {
            name: path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            contents: std::fs::read_to_string(&path).map(|t| PlanContents::parse(&t)).unwrap_or_default(),
            path,
            is_default,
        })
        .collect()
}

/// `(plan path, is default)` for each `<TestPlanReference>`.
pub fn parse_scheme_plans(xml: &str, container_dir: &Path) -> Vec<(PathBuf, bool)> {
    static REFERENCE: OnceLock<Regex> = OnceLock::new();
    static ATTRIBUTE: OnceLock<Regex> = OnceLock::new();
    let reference = REFERENCE.get_or_init(|| Regex::new(r"<TestPlanReference\b([^>]*)>").expect("valid regex"));
    let attribute = ATTRIBUTE.get_or_init(|| Regex::new(r#"(\w+)\s*=\s*"([^"]*)""#).expect("valid regex"));
    reference
        .captures_iter(xml)
        .filter_map(|c| {
            let mut path = None;
            let mut is_default = false;
            for a in attribute.captures_iter(&c[1]) {
                match &a[1] {
                    "reference" => {
                        let value = &a[2];
                        path = Some(match value.split_once(':') {
                            Some(("absolute", p)) => PathBuf::from(p),
                            Some((_, p)) => container_dir.join(p),
                            None => container_dir.join(value),
                        });
                    }
                    "default" => is_default = &a[2] == "YES",
                    _ => {}
                }
            }
            path.map(|p| (p, is_default))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_scheme_plan_references() {
        let xml = r#"
      <TestPlans>
         <TestPlanReference
            reference = "container:HelloZ.xctestplan"
            default = "YES">
         </TestPlanReference>
         <TestPlanReference
            reference = "container:Plans/Smoke.xctestplan">
         </TestPlanReference>
      </TestPlans>"#;
        assert_eq!(
            parse_scheme_plans(xml, Path::new("/p")),
            [(PathBuf::from("/p/HelloZ.xctestplan"), true), (PathBuf::from("/p/Plans/Smoke.xctestplan"), false)]
        );
    }

    #[test]
    fn plan_selection() {
        let plan = PlanContents::parse(
            r#"{
              "testTargets": [
                { "skippedTests": ["LegacyTests/testTwice()", "Slow"], "target": { "name": "AppTests" } },
                { "selectedTests": ["GreeterSuite/greets()"], "target": { "name": "MoreTests" } },
                { "enabled": false, "target": { "name": "UITests" } }
              ]
            }"#,
        );
        let c = |s: &str| s.split('/').map(str::to_string).collect::<Vec<_>>();
        assert!(plan.includes("AppTests", &c("LegacyTests/testOnce")));
        assert!(!plan.includes("AppTests", &c("LegacyTests/testTwice")));
        assert!(!plan.includes("AppTests", &c("Slow/a()")));
        assert!(plan.includes("MoreTests", &c("GreeterSuite")));
        assert!(plan.includes("MoreTests", &c("GreeterSuite/greets()")));
        assert!(!plan.includes("MoreTests", &c("GreeterSuite/named()")));
        assert!(!plan.includes("UITests", &c("A/testB")));
        assert!(!plan.includes("Unlisted", &c("A")));
    }
}

//! Runs a scheme's unit tests inside the iOS Simulator by injecting each test
//! bundle into its host app (`libXCTestBundleInject`) — the mechanism XCTest
//! uses for hosted tests — and reading results from the app's console.
//!
//! Compared with `xcodebuild test`, this skips the external test runner and
//! `testmanagerd` connection: it is faster, and it keeps working when that
//! service is unavailable (as with some simulator runtime / Xcode pairings,
//! where `xcodebuild test` reports "The test runner hung before establishing
//! connection"). UI test bundles still need `xcodebuild test`.
//!
//! Which tests run is XCTest's scope (`XCTestScopeFile`, the file form of
//! `-XCTest Class/test`), passed to the host app as a launch argument. XCTest
//! forwards module-qualified entries (`Module.Suite/test()`) to Swift
//! Testing, so one scope selects tests of both frameworks. A test plan's
//! selection comes from the `.xctestrun` that `build-for-testing -testPlan`
//! writes.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct TestTarget {
    pub name: String,
    pub module: String,
    /// The test plan configuration, when the plan has several.
    pub configuration: Option<String>,
    /// Host app to install, or `None` for a hostless (library) bundle.
    pub host_app: Option<PathBuf>,
    pub host_bundle_id: Option<String>,
    /// Bundle path relative to the host app (`PlugIns/X.xctest`), or an
    /// absolute path for hostless bundles.
    pub bundle: PathBuf,
    pub is_ui_test: bool,
    /// The scheme's or test plan's selection (`Class`, `Class/test()`,
    /// `Suite/test()`).
    pub only: Option<Vec<String>>,
    pub skip: Vec<String>,
    /// The scheme's or test plan's environment and launch arguments.
    pub environment: Vec<(String, String)>,
    pub arguments: Vec<String>,
}

/// Which tests of a bundle to run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scope {
    pub entries: Vec<String>,
    /// Run everything except `entries`.
    pub invert: bool,
}

impl Scope {
    pub fn is_all(&self) -> bool {
        self.entries.is_empty() && !self.invert
    }

    /// The bundle's own selection (from the scheme or test plan). Entries
    /// are qualified with the module so they reach Swift Testing too; a
    /// skip list carries both forms, because XCTest skips only by the
    /// unqualified name.
    pub fn for_target(target: &TestTarget) -> Self {
        let qualified = |id: &String| format!("{}.{id}", target.module);
        match &target.only {
            Some(only) => Self { entries: only.iter().map(qualified).collect(), invert: false },
            None if !target.skip.is_empty() => Self {
                entries: target.skip.iter().flat_map(|id| [id.clone(), qualified(id)]).collect(),
                invert: true,
            },
            None => Self::default(),
        }
    }

    /// An `XCTestScopeFile` property list.
    pub fn plist(&self) -> String {
        let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let entries: String = self.entries.iter().map(|e| format!("<string>{}</string>", escape(e))).collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>XCTestScope</key><array>{entries}</array><key>XCTestInvertScope</key><{}/></dict></plist>\n",
            if self.invert { "true" } else { "false" }
        )
    }
}

/// Environment from the `.xctestrun` worth passing on: what the scheme and
/// test plan set, not the loader paths and placeholders meant for Xcode's
/// own runner.
fn passed_environment(value: Option<&Value>) -> Vec<(String, String)> {
    let Some(map) = value.and_then(Value::as_object) else { return vec![] };
    map.iter()
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
        .filter(|(k, v)| {
            !k.starts_with("DYLD_") && !k.starts_with("__") && k != "XCInjectBundleInto" && !v.contains("__TEST") && !v.contains("__PLATFORMS__") && !v.contains("__SIMRUNTIMEROOT__")
        })
        .collect()
}

/// The newest `<scheme>_*.xctestrun` written by `build-for-testing`.
pub fn find_xctestrun(build_dir: &Path, scheme: &str) -> Option<PathBuf> {
    std::fs::read_dir(build_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "xctestrun")
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(&format!("{scheme}_")))
        })
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

/// Parses an `.xctestrun` (format version 1 or 2).
pub fn parse_xctestrun(path: &Path) -> Result<Vec<TestTarget>> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .output()
        .context("running plutil")?;
    if !output.status.success() {
        bail!("could not read {}", path.display());
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    let test_root = path.parent().unwrap_or(Path::new("/"));
    parse_xctestrun_value(&value, test_root)
}

pub fn parse_xctestrun_value(value: &Value, test_root: &Path) -> Result<Vec<TestTarget>> {
    let entries: Vec<(String, Option<String>, Value)> = match value.get("TestConfigurations").and_then(Value::as_array) {
        // Version 2: TestConfigurations[].TestTargets[].
        Some(configurations) => {
            let named = configurations.len() > 1;
            configurations
                .iter()
                .flat_map(|c| {
                    let configuration = named.then(|| c.get("Name").and_then(Value::as_str).unwrap_or("").to_string());
                    c.get("TestTargets")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(move |t| (configuration.clone(), t))
                })
                .map(|(configuration, t)| {
                    let name = t.get("BlueprintName").and_then(Value::as_str).unwrap_or("Tests").to_string();
                    (name, configuration, t)
                })
                .collect()
        }
        // Version 1: one top-level key per test target.
        None => value
            .as_object()
            .map(|o| {
                o.iter()
                    .filter(|(k, v)| !k.starts_with("__") && v.get("TestBundlePath").is_some())
                    .map(|(k, v)| (k.clone(), None, v.clone()))
                    .collect()
            })
            .unwrap_or_default(),
    };
    let root = test_root.display().to_string();
    let mut targets = vec![];
    for (name, configuration, target) in entries {
        let string = |key: &str| target.get(key).and_then(Value::as_str).map(str::to_string);
        let strings = |key: &str| {
            target.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect::<Vec<_>>())
        };
        let host_app = string("TestHostPath")
            .filter(|p| p.starts_with("__TESTROOT__"))
            .map(|p| PathBuf::from(p.replace("__TESTROOT__", &root)));
        let Some(bundle) = string("TestBundlePath") else { continue };
        let bundle = if let Some(rest) = bundle.strip_prefix("__TESTHOST__/") {
            PathBuf::from(rest)
        } else {
            PathBuf::from(bundle.replace("__TESTROOT__", &root))
        };
        let mut environment = passed_environment(target.get("EnvironmentVariables"));
        for (key, value) in passed_environment(target.get("TestingEnvironmentVariables")) {
            if !environment.iter().any(|(k, _)| *k == key) {
                environment.push((key, value));
            }
        }
        environment.sort();
        targets.push(TestTarget {
            module: string("ProductModuleName").unwrap_or_else(|| crate::testing::module_name(&name)),
            name,
            configuration,
            host_app,
            host_bundle_id: string("TestHostBundleIdentifier"),
            bundle,
            is_ui_test: target.get("IsUITestBundle").and_then(Value::as_bool).unwrap_or(false),
            only: strings("OnlyTestIdentifiers"),
            skip: strings("SkipTestIdentifiers").unwrap_or_default(),
            environment,
            arguments: strings("CommandLineArguments").unwrap_or_default(),
        });
    }
    Ok(targets)
}

fn simulator_developer_dir() -> Result<PathBuf> {
    let output = Command::new("/usr/bin/xcode-select").arg("-p").output()?;
    let developer = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    Ok(developer.join("Platforms/iPhoneSimulator.platform/Developer"))
}

/// The environment `simctl` passes to the launched process (`SIMCTL_CHILD_`).
fn injection_env(platform: &Path, bundle: &Path, host_executable: Option<&Path>) -> Vec<(String, String)> {
    let mut env = vec![
        ("DYLD_FRAMEWORK_PATH", format!(
            "{}:{}",
            platform.join("Library/Frameworks").display(),
            platform.join("Library/PrivateFrameworks").display()
        )),
        ("DYLD_LIBRARY_PATH", platform.join("usr/lib").display().to_string()),
        ("XCTestBundlePath", bundle.display().to_string()),
        // Coverage instrumentation would try to write into the read-only bundle.
        ("LLVM_PROFILE_FILE", "/dev/null".to_string()),
    ];
    if let Some(host) = host_executable {
        env.push((
            "DYLD_INSERT_LIBRARIES",
            platform.join("usr/lib/libXCTestBundleInject.dylib").display().to_string(),
        ));
        env.push(("XCInjectBundleInto", host.display().to_string()));
    }
    env.into_iter()
        .map(|(k, v)| (format!("SIMCTL_CHILD_{k}"), v))
        .collect()
}

/// Installs the host app (if any) and returns the command that runs `target`'s
/// tests with their output on stdout. `scope_file` is an `XCTestScopeFile`
/// limiting which tests run.
pub fn test_command(udid: &str, target: &TestTarget, scope_file: Option<&Path>) -> Result<Command> {
    let platform = simulator_developer_dir()?;
    let mut arguments: Vec<String> = target.arguments.clone();
    if let Some(file) = scope_file {
        arguments.extend(["-XCTestScopeFile".to_string(), file.display().to_string()]);
    }
    let environment = target
        .environment
        .iter()
        .map(|(k, v)| (format!("SIMCTL_CHILD_{k}"), v.clone()));
    match (&target.host_app, &target.host_bundle_id) {
        (Some(app), Some(bundle_id)) => {
            let install = Command::new("/usr/bin/xcrun")
                .args(["simctl", "install", udid])
                .arg(app)
                .output()?;
            if !install.status.success() {
                bail!("simctl install failed: {}", String::from_utf8_lossy(&install.stderr).trim());
            }
            let container = Command::new("/usr/bin/xcrun")
                .args(["simctl", "get_app_container", udid, bundle_id, "app"])
                .output()?;
            let container = PathBuf::from(String::from_utf8_lossy(&container.stdout).trim());
            let executable = container.join(
                app.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
            let mut cmd = Command::new("/usr/bin/xcrun");
            cmd.args(["simctl", "launch", "--console-pty", "--terminate-running-process", udid, bundle_id]);
            cmd.args(&arguments);
            cmd.envs(environment);
            for (key, value) in injection_env(&platform, &container.join(&target.bundle), Some(&executable)) {
                cmd.env(key, value);
            }
            Ok(cmd)
        }
        _ => {
            // Hostless bundle: run it with the simulator's `xctest` agent.
            let mut cmd = Command::new("/usr/bin/xcrun");
            cmd.args(["simctl", "spawn", udid])
                .arg(platform.join("Library/Xcode/Agents/xctest"))
                .args(&arguments)
                .arg(&target.bundle);
            cmd.envs(environment);
            for (key, value) in injection_env(&platform, &target.bundle, None) {
                cmd.env(key, value);
            }
            Ok(cmd)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_v2_xctestrun() {
        let value = json!({
            "TestConfigurations": [{ "TestTargets": [
                {
                    "BlueprintName": "AppTests",
                    "TestHostPath": "__TESTROOT__/Debug-iphonesimulator/App.app",
                    "TestHostBundleIdentifier": "dev.example.App",
                    "TestBundlePath": "__TESTHOST__/PlugIns/AppTests.xctest",
                    "ProductModuleName": "AppTests",
                    "SkipTestIdentifiers": ["LegacyTests/testSlow()", "SlowSuite"],
                    "EnvironmentVariables": { "API_URL": "http://localhost", "DYLD_INSERT_LIBRARIES": "/usr/lib/libRPAC.dylib" },
                    "TestingEnvironmentVariables": { "XCODE_TEST_PLAN_NAME": "Smoke", "DYLD_LIBRARY_PATH": "__TESTROOT__/Debug" },
                    "CommandLineArguments": ["-UITesting"]
                },
                {
                    "BlueprintName": "AppUITests",
                    "TestHostPath": "__TESTROOT__/Debug-iphonesimulator/AppUITests-Runner.app",
                    "TestBundlePath": "__TESTHOST__/PlugIns/AppUITests.xctest",
                    "IsUITestBundle": true
                }
            ]}],
            "__xctestrun_metadata__": { "FormatVersion": 2 }
        });
        let targets = parse_xctestrun_value(&value, Path::new("/dd/Build/Products")).unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(
            targets[0].host_app.as_deref(),
            Some(Path::new("/dd/Build/Products/Debug-iphonesimulator/App.app"))
        );
        assert_eq!(targets[0].bundle, PathBuf::from("PlugIns/AppTests.xctest"));
        assert!(!targets[0].is_ui_test);
        assert!(targets[1].is_ui_test);
        assert_eq!(targets[0].module, "AppTests");
        assert_eq!(targets[1].module, "AppUITests");
        assert_eq!(
            targets[0].environment,
            [("API_URL".to_string(), "http://localhost".to_string()), ("XCODE_TEST_PLAN_NAME".to_string(), "Smoke".to_string())]
        );
        assert_eq!(targets[0].arguments, ["-UITesting"]);
        let scope = Scope::for_target(&targets[0]);
        assert!(scope.invert);
        assert_eq!(scope.entries, ["LegacyTests/testSlow()", "AppTests.LegacyTests/testSlow()", "SlowSuite", "AppTests.SlowSuite"]);
        assert!(scope.plist().contains("<key>XCTestInvertScope</key><true/>"));
        assert!(Scope::for_target(&targets[1]).is_all());
    }

    #[test]
    fn parses_v1_xctestrun() {
        let value = json!({
            "LibTests": { "TestBundlePath": "__TESTROOT__/Debug-iphonesimulator/LibTests.xctest" },
            "__xctestrun_metadata__": { "FormatVersion": 1 }
        });
        let targets = parse_xctestrun_value(&value, Path::new("/p")).unwrap();
        assert_eq!(targets.len(), 1);
        assert!(targets[0].host_app.is_none());
        assert_eq!(targets[0].bundle, PathBuf::from("/p/Debug-iphonesimulator/LibTests.xctest"));
    }
}

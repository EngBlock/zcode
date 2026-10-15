//! Runs a scheme's unit tests inside the iOS Simulator by injecting each test
//! bundle into its host app (`libXCTestBundleInject`) — the mechanism XCTest
//! uses for hosted tests — and reading results from the app's console.
//!
//! Compared with `xcodebuild test`, this skips the external test runner and
//! `testmanagerd` connection: it is faster, and it keeps working when that
//! service is unavailable (as with some simulator runtime / Xcode pairings,
//! where `xcodebuild test` reports "The test runner hung before establishing
//! connection"). UI test bundles still need `xcodebuild test`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct TestTarget {
    pub name: String,
    /// Host app to install, or `None` for a hostless (library) bundle.
    pub host_app: Option<PathBuf>,
    pub host_bundle_id: Option<String>,
    /// Bundle path relative to the host app (`PlugIns/X.xctest`), or an
    /// absolute path for hostless bundles.
    pub bundle: PathBuf,
    pub is_ui_test: bool,
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
    let entries: Vec<(String, Value)> = match value.get("TestConfigurations").and_then(Value::as_array) {
        // Version 2: TestConfigurations[].TestTargets[].
        Some(configurations) => configurations
            .iter()
            .flat_map(|c| c.get("TestTargets").and_then(Value::as_array).cloned().unwrap_or_default())
            .map(|t| {
                let name = t.get("BlueprintName").and_then(Value::as_str).unwrap_or("Tests").to_string();
                (name, t)
            })
            .collect(),
        // Version 1: one top-level key per test target.
        None => value
            .as_object()
            .map(|o| {
                o.iter()
                    .filter(|(k, v)| !k.starts_with("__") && v.get("TestBundlePath").is_some())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default(),
    };
    let root = test_root.display().to_string();
    let mut targets = vec![];
    for (name, target) in entries {
        let string = |key: &str| target.get(key).and_then(Value::as_str).map(str::to_string);
        let host_app = string("TestHostPath")
            .filter(|p| p.starts_with("__TESTROOT__"))
            .map(|p| PathBuf::from(p.replace("__TESTROOT__", &root)));
        let Some(bundle) = string("TestBundlePath") else { continue };
        let bundle = if let Some(rest) = bundle.strip_prefix("__TESTHOST__/") {
            PathBuf::from(rest)
        } else {
            PathBuf::from(bundle.replace("__TESTROOT__", &root))
        };
        targets.push(TestTarget {
            name,
            host_app,
            host_bundle_id: string("TestHostBundleIdentifier"),
            bundle,
            is_ui_test: target.get("IsUITestBundle").and_then(Value::as_bool).unwrap_or(false),
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
/// tests with their output on stdout.
pub fn test_command(udid: &str, target: &TestTarget) -> Result<Command> {
    let platform = simulator_developer_dir()?;
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
                .arg(&target.bundle);
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
                    "TestBundlePath": "__TESTHOST__/PlugIns/AppTests.xctest"
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

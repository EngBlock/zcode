//! Queries against xcodebuild, simctl and SwiftPM. All functions block; call
//! them from background tasks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::project::{ProjectInfo, ProjectKind};

#[derive(Clone, Debug, Default)]
pub struct SchemeList {
    pub schemes: Vec<String>,
    pub configurations: Vec<String>,
    pub targets: Vec<String>,
    /// A Swift package's test targets and their folders (relative to the
    /// package), which tell which target a test file belongs to.
    pub test_targets: Vec<(String, PathBuf)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    MacOS,
    IOSSimulator,
    WatchOSSimulator,
    TvOSSimulator,
    VisionOSSimulator,
}

impl Platform {
    pub fn sdk(&self) -> &'static str {
        match self {
            Self::MacOS => "macosx",
            Self::IOSSimulator => "iphonesimulator",
            Self::WatchOSSimulator => "watchsimulator",
            Self::TvOSSimulator => "appletvsimulator",
            Self::VisionOSSimulator => "xrsimulator",
        }
    }

    pub fn is_simulator(&self) -> bool {
        !matches!(self, Self::MacOS)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Destination {
    /// Simulator UDID, or "mac" for My Mac.
    pub id: String,
    pub name: String,
    pub platform: Platform,
    /// e.g. "iOS 27.0".
    pub runtime: String,
    pub booted: bool,
}

impl Destination {
    pub fn my_mac() -> Self {
        Self {
            id: "mac".into(),
            name: "My Mac".into(),
            platform: Platform::MacOS,
            runtime: "macOS".into(),
            booted: true,
        }
    }

    /// The `-destination` specifier for xcodebuild.
    pub fn xcodebuild_specifier(&self) -> String {
        match self.platform {
            Platform::MacOS => "platform=macOS,arch=arm64".into(),
            _ => format!("id={}", self.id),
        }
    }
}

fn run_json(cmd: &mut Command) -> Result<Value> {
    let output = cmd.output().with_context(|| format!("running {cmd:?}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{}", stderr.lines().last().unwrap_or("command failed"));
    }
    // xcodebuild may print warnings before the JSON payload.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let start = stdout.find(['{', '[']).unwrap_or(0);
    Ok(serde_json::from_str(&stdout[start..])?)
}

pub fn list_schemes(project: &ProjectInfo) -> Result<SchemeList> {
    match &project.kind {
        ProjectKind::XcodeProject(_) | ProjectKind::XcodeWorkspace(_) => {
            let value = run_json(
                Command::new("/usr/bin/xcodebuild")
                    .arg("-list")
                    .arg("-json")
                    .args(project.kind.xcodebuild_args())
                    .current_dir(&project.root),
            )?;
            let section = value
                .get("project")
                .or_else(|| value.get("workspace"))
                .cloned()
                .unwrap_or(Value::Null);
            let strings = |key: &str| -> Vec<String> {
                section
                    .get(key)
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let mut configurations = strings("configurations");
            if configurations.is_empty() {
                configurations = vec!["Debug".into(), "Release".into()];
            }
            Ok(SchemeList {
                schemes: strings("schemes"),
                configurations,
                targets: strings("targets"),
                test_targets: vec![],
            })
        }
        ProjectKind::SwiftPackage => {
            let value = run_json(
                Command::new("/usr/bin/swift")
                    .args(["package", "describe", "--type", "json"])
                    .current_dir(&project.root),
            )?;
            let mut schemes = vec![];
            let mut targets = vec![];
            let mut test_targets = vec![];
            if let Some(products) = value.get("products").and_then(Value::as_array) {
                for product in products {
                    let name = product.get("name").and_then(Value::as_str).unwrap_or("");
                    let executable = product
                        .get("type")
                        .is_some_and(|t| t.get("executable").is_some());
                    if executable {
                        schemes.insert(0, name.to_string());
                    } else {
                        schemes.push(name.to_string());
                    }
                }
            }
            if let Some(ts) = value.get("targets").and_then(Value::as_array) {
                targets = ts
                    .iter()
                    .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
                    .collect();
                test_targets = ts
                    .iter()
                    .filter(|t| t.get("type").and_then(Value::as_str) == Some("test"))
                    .filter_map(|t| {
                        let name = t.get("name")?.as_str()?.to_string();
                        Some((name, PathBuf::from(t.get("path")?.as_str()?)))
                    })
                    .collect();
            }
            Ok(SchemeList {
                schemes,
                configurations: vec!["Debug".into(), "Release".into()],
                targets,
                test_targets,
            })
        }
        ProjectKind::Folder => Ok(SchemeList::default()),
    }
}

/// Executable products in a Swift package.
pub fn swiftpm_executables(root: &Path) -> Vec<String> {
    let Ok(value) = run_json(
        Command::new("/usr/bin/swift")
            .args(["package", "describe", "--type", "json"])
            .current_dir(root),
    ) else {
        return vec![];
    };
    value
        .get("products")
        .and_then(Value::as_array)
        .map(|products| {
            products
                .iter()
                .filter(|p| p.get("type").is_some_and(|t| t.get("executable").is_some()))
                .filter_map(|p| p.get("name").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct SimctlDevices {
    devices: HashMap<String, Vec<SimctlDevice>>,
}

#[derive(Deserialize)]
struct SimctlDevice {
    udid: String,
    name: String,
    state: String,
    #[serde(rename = "isAvailable", default)]
    is_available: bool,
}

/// Available simulators, newest runtime first, booted devices first.
pub fn list_simulators() -> Result<Vec<Destination>> {
    let value = run_json(Command::new("/usr/bin/xcrun").args([
        "simctl", "list", "devices", "available", "--json",
    ]))?;
    let parsed: SimctlDevices = serde_json::from_value(value)?;
    let mut out = vec![];
    for (runtime_id, devices) in parsed.devices {
        // com.apple.CoreSimulator.SimRuntime.iOS-27-0
        let runtime = runtime_id
            .rsplit('.')
            .next()
            .unwrap_or(&runtime_id)
            .to_string();
        let mut parts = runtime.splitn(2, '-');
        let os = parts.next().unwrap_or("");
        let version = parts.next().unwrap_or("").replace('-', ".");
        let platform = match os {
            "iOS" => Platform::IOSSimulator,
            "watchOS" => Platform::WatchOSSimulator,
            "tvOS" => Platform::TvOSSimulator,
            "xrOS" | "visionOS" => Platform::VisionOSSimulator,
            _ => continue,
        };
        let os_label = if os == "xrOS" { "visionOS" } else { os };
        for device in devices.into_iter().filter(|d| d.is_available) {
            out.push(Destination {
                id: device.udid,
                name: device.name,
                platform,
                runtime: format!("{os_label} {version}"),
                booted: device.state == "Booted",
            });
        }
    }
    out.sort_by(|a, b| {
        b.booted
            .cmp(&a.booted)
            .then(version_key(&b.runtime).cmp(&version_key(&a.runtime)))
            .then(a.name.cmp(&b.name))
    });
    Ok(out)
}

/// The host folder a simulator's runtime is rooted at (its `/`).
pub fn simulator_runtime_root(udid: &str) -> Option<PathBuf> {
    let devices = run_json(Command::new("/usr/bin/xcrun").args(["simctl", "list", "devices", "--json"])).ok()?;
    let runtime = devices
        .get("devices")?
        .as_object()?
        .iter()
        .find(|(_, list)| {
            list.as_array()
                .is_some_and(|l| l.iter().any(|d| d.get("udid").and_then(Value::as_str) == Some(udid)))
        })
        .map(|(runtime, _)| runtime.clone())?;
    let runtimes = run_json(Command::new("/usr/bin/xcrun").args(["simctl", "list", "runtimes", "--json"])).ok()?;
    runtimes
        .get("runtimes")?
        .as_array()?
        .iter()
        .find(|r| r.get("identifier").and_then(Value::as_str) == Some(runtime.as_str()))
        .and_then(|r| r.get("runtimeRoot")?.as_str())
        .map(PathBuf::from)
}

/// Main Thread Checker, which Xcode inserts into apps it runs under the
/// debugger: the toolchain's for Mac processes, the runtime's in a simulator.
pub fn main_thread_checker(destination: &Destination) -> Option<PathBuf> {
    let root = match destination.platform {
        Platform::MacOS => crate::toolchain::developer_dir()?,
        _ => simulator_runtime_root(&destination.id)?,
    };
    Some(root.join("usr/lib/libMainThreadChecker.dylib")).filter(|p| p.exists())
}

fn version_key(runtime: &str) -> Vec<u32> {
    runtime
        .rsplit(' ')
        .next()
        .unwrap_or("")
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct BuildSettings {
    pub target: String,
    pub values: HashMap<String, String>,
}

impl BuildSettings {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn is_application(&self) -> bool {
        self.get("PRODUCT_TYPE")
            .is_some_and(|t| t.starts_with("com.apple.product-type.application"))
    }

    pub fn is_test_bundle(&self) -> bool {
        self.get("PRODUCT_TYPE")
            .is_some_and(|t| t.contains("bundle.unit-test") || t.contains("bundle.ui-testing"))
    }

    /// Path to the built product (`.app` bundle, framework, tool...).
    pub fn product_path(&self) -> Option<PathBuf> {
        Some(PathBuf::from(self.get("TARGET_BUILD_DIR")?).join(self.get("FULL_PRODUCT_NAME")?))
    }

    /// Path to the executable inside the product.
    pub fn executable_path(&self) -> Option<PathBuf> {
        Some(PathBuf::from(self.get("TARGET_BUILD_DIR")?).join(self.get("EXECUTABLE_PATH")?))
    }

    pub fn bundle_identifier(&self) -> Option<&str> {
        self.get("PRODUCT_BUNDLE_IDENTIFIER")
    }

    pub fn supported_platforms(&self) -> Vec<String> {
        self.get("SUPPORTED_PLATFORMS")
            .map(|s| s.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default()
    }
}

pub fn show_build_settings(
    project: &ProjectInfo,
    scheme: &str,
    configuration: &str,
    destination: Option<&Destination>,
) -> Result<Vec<BuildSettings>> {
    let mut cmd = Command::new("/usr/bin/xcodebuild");
    cmd.arg("-showBuildSettings")
        .arg("-json")
        .args(project.kind.xcodebuild_args())
        .args(["-scheme", scheme, "-configuration", configuration])
        .current_dir(&project.root);
    if let Some(destination) = destination {
        cmd.args(["-destination", &destination.xcodebuild_specifier()]);
    }
    let value = run_json(&mut cmd)?;
    let entries = value.as_array().cloned().unwrap_or_default();
    Ok(entries
        .into_iter()
        .map(|entry| BuildSettings {
            target: entry
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            values: entry
                .get("buildSettings")
                .and_then(Value::as_object)
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect())
}

/// Picks the settings of the runnable product among a scheme's targets.
pub fn runnable_settings(settings: &[BuildSettings], scheme: &str) -> Option<BuildSettings> {
    settings
        .iter()
        .find(|s| s.is_application() && s.target == scheme)
        .or_else(|| settings.iter().find(|s| s.is_application()))
        .or_else(|| settings.iter().find(|s| s.target == scheme))
        .or_else(|| settings.iter().find(|s| !s.is_test_bundle()))
        .cloned()
}

/// Destinations a scheme can run on, given its build settings.
pub fn filter_destinations(all: &[Destination], supported: &[String]) -> Vec<Destination> {
    if supported.is_empty() {
        return all.to_vec();
    }
    all.iter()
        .filter(|d| supported.iter().any(|p| p == d.platform.sdk()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_specifiers() {
        assert_eq!(
            Destination::my_mac().xcodebuild_specifier(),
            "platform=macOS,arch=arm64"
        );
    }

    #[test]
    fn version_ordering() {
        assert!(version_key("iOS 27.0") > version_key("iOS 18.4"));
    }
}

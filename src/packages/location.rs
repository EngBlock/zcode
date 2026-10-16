//! Where a project's packages are: its `Package.resolved`, and the folder
//! resolved packages are checked out in. Xcode keeps checkouts in the
//! project's DerivedData folder (`SourcePackages/checkouts`), found through
//! the `WorkspacePath` each DerivedData folder records in its `info.plist`;
//! Swift packages keep them in `.build/checkouts`.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::project::ProjectKind;

/// The `Package.resolved` a project's packages are pinned in.
pub fn resolved_file(kind: &ProjectKind, root: &Path) -> Option<PathBuf> {
    match kind {
        ProjectKind::XcodeProject(project) => {
            Some(project.join("project.xcworkspace/xcshareddata/swiftpm/Package.resolved"))
        }
        ProjectKind::XcodeWorkspace(workspace) => Some(workspace.join("xcshareddata/swiftpm/Package.resolved")),
        ProjectKind::SwiftPackage => Some(root.join("Package.resolved")),
        ProjectKind::Folder => None,
    }
}

/// Whether `path` is inside a package checkout (sources zcode shows
/// read-only): `SourcePackages/checkouts` or `.build/checkouts`.
pub fn is_package_checkout(path: &Path) -> bool {
    let parts: Vec<&std::ffi::OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    parts.windows(2).any(|w| (w[0] == "SourcePackages" || w[0] == ".build") && w[1] == "checkouts")
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Xcode's DerivedData folder: the user's custom location if set (read
/// from Xcode's defaults), else `~/Library/Developer/Xcode/DerivedData`.
pub fn derived_data_root() -> PathBuf {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(read_derived_data_root).clone()
}

fn read_derived_data_root() -> PathBuf {
    let custom = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "com.apple.dt.Xcode", "IDECustomDerivedDataLocation"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    match custom {
        Some(path) if path.starts_with('/') => PathBuf::from(path),
        _ => home().join("Library/Developer/Xcode/DerivedData"),
    }
}

/// The DerivedData folder Xcode and `xcodebuild` use for `workspace` (an
/// `.xcodeproj` or `.xcworkspace`). Folders are named `<Name>-<hash>`;
/// the one whose `info.plist` records this workspace path is it.
pub fn derived_data_for(root: &Path, workspace: &Path) -> Option<PathBuf> {
    let name = workspace.file_stem()?.to_string_lossy().replace(' ', "_");
    let prefix = format!("{name}-");
    let needle = workspace.to_string_lossy().to_string();
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix)))
        .filter(|p| {
            // XML or binary, an ASCII path appears as is.
            std::fs::read(p.join("info.plist"))
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
        })
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
        .collect();
    found.sort();
    found.pop().map(|(_, p)| p)
}

/// The folder holding a project's `checkouts` and `workspace-state.json`:
/// `SourcePackages` in its DerivedData folder (`None` before anything
/// resolved there), or a package's `.build`.
pub fn packages_dir(kind: &ProjectKind, root: &Path) -> Option<PathBuf> {
    match kind {
        ProjectKind::XcodeProject(workspace) | ProjectKind::XcodeWorkspace(workspace) => {
            derived_data_for(&derived_data_root(), workspace).map(|d| d.join("SourcePackages"))
        }
        ProjectKind::SwiftPackage => Some(root.join(".build")),
        ProjectKind::Folder => None,
    }
}

/// Checked-out (or local) packages by identity, from SwiftPM's
/// `workspace-state.json` in `dir`, falling back to the `checkouts`
/// folder's contents.
pub fn checkouts(dir: &Path) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    if let Some(state) = std::fs::read_to_string(dir.join("workspace-state.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        let dependencies = state.pointer("/object/dependencies").and_then(Value::as_array).cloned().unwrap_or_default();
        for dependency in dependencies {
            let Some(reference) = dependency.get("packageRef") else { continue };
            let Some(identity) = reference.get("identity").and_then(Value::as_str) else { continue };
            let kind = dependency.pointer("/state/name").and_then(Value::as_str).unwrap_or("");
            let path = match kind {
                "fileSystem" => reference.get("location").and_then(Value::as_str).map(PathBuf::from),
                // Packages in edit mode live in Packages/.
                "edited" => dependency
                    .pointer("/state/path")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .or_else(|| dependency.get("subpath").and_then(Value::as_str).map(|s| dir.join("../Packages").join(s))),
                _ => dependency.get("subpath").and_then(Value::as_str).map(|s| dir.join("checkouts").join(s)),
            };
            if let Some(path) = path.filter(|p| p.exists()) {
                out.insert(identity.to_string(), path);
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(dir.join("checkouts")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.path().is_dir() && !name.starts_with('.') {
                out.entry(name.to_lowercase()).or_insert_with(|| entry.path());
            }
        }
    }
    out
}

/// Makes SwiftPM forget the checkouts of `identities` (every checkout
/// when `None`) in `dir`: their entries in `workspace-state.json` and their
/// folders. Without a pin and a checkout, the next resolve picks the newest
/// version a package's rule allows, which is how `xcodebuild` updates
/// packages (it prefers an existing checkout to a newer version otherwise).
/// The repository caches stay, so only new commits are fetched.
pub fn forget_checkouts(dir: &Path, identities: Option<&[String]>) -> anyhow::Result<()> {
    let state_file = dir.join("workspace-state.json");
    let Ok(text) = std::fs::read_to_string(&state_file) else { return Ok(()) };
    let mut state: Value = serde_json::from_str(&text)?;
    let Some(dependencies) = state.pointer_mut("/object/dependencies").and_then(Value::as_array_mut) else { return Ok(()) };
    let mut forgotten = vec![];
    dependencies.retain(|dependency| {
        let identity = dependency.pointer("/packageRef/identity").and_then(Value::as_str).unwrap_or("");
        let checkout = dependency.pointer("/state/name").and_then(Value::as_str) == Some("sourceControlCheckout");
        let forget = checkout && identities.is_none_or(|ids| ids.iter().any(|i| i.eq_ignore_ascii_case(identity)));
        if forget && let Some(subpath) = dependency.get("subpath").and_then(Value::as_str) {
            forgotten.push(dir.join("checkouts").join(subpath));
        }
        !forget
    });
    std::fs::write(&state_file, super::resolved::to_swiftpm_json(&state))?;
    for checkout in forgotten {
        if checkout.exists() {
            std::fs::remove_dir_all(&checkout)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_checkouts_are_read_only() {
        assert!(is_package_checkout(Path::new(
            "/Users/me/Library/Developer/Xcode/DerivedData/App-abc/SourcePackages/checkouts/swift-collections/Sources/A.swift"
        )));
        assert!(is_package_checkout(Path::new("/p/ZKit/.build/checkouts/Tally/Sources/Tally/Tally.swift")));
        assert!(!is_package_checkout(Path::new("/p/ZKit/Sources/checkouts/A.swift")));
        assert!(!is_package_checkout(Path::new("/p/App/SourcePackages/A.swift")));
    }

    #[test]
    fn resolved_files() {
        let project = ProjectKind::XcodeProject(PathBuf::from("/p/App.xcodeproj"));
        assert_eq!(
            resolved_file(&project, Path::new("/p")).unwrap(),
            Path::new("/p/App.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved")
        );
        let workspace = ProjectKind::XcodeWorkspace(PathBuf::from("/p/App.xcworkspace"));
        assert_eq!(
            resolved_file(&workspace, Path::new("/p")).unwrap(),
            Path::new("/p/App.xcworkspace/xcshareddata/swiftpm/Package.resolved")
        );
        assert_eq!(resolved_file(&ProjectKind::SwiftPackage, Path::new("/p")).unwrap(), Path::new("/p/Package.resolved"));
    }

    #[test]
    fn finds_derived_data_and_checkouts() {
        let dir = std::env::temp_dir().join(format!("zcode-dd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let workspace = Path::new("/Users/me/Projects/My App/My App.xcodeproj");
        for (name, path) in [("My_App-aaaa", "/elsewhere/My App.xcodeproj"), ("My_App-bbbb", workspace.to_str().unwrap())] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            let plist = format!("<plist><dict><key>WorkspacePath</key><string>{path}</string></dict></plist>");
            std::fs::write(dir.join(name).join("info.plist"), plist).unwrap();
        }
        let found = derived_data_for(&dir, workspace).unwrap();
        assert!(found.ends_with("My_App-bbbb"));

        let packages = found.join("SourcePackages");
        std::fs::create_dir_all(packages.join("checkouts/swift-collections")).unwrap();
        std::fs::create_dir_all(packages.join("checkouts/Tally")).unwrap();
        let state = r#"{"object":{"dependencies":[{"basedOn":null,"packageRef":{"identity":"tally","kind":"localSourceControl","location":"/tmp/Tally.git","name":"Tally"},"state":{"checkoutState":{"version":"1.2.0"},"name":"sourceControlCheckout"},"subpath":"Tally"}]},"version":7}"#;
        std::fs::write(packages.join("workspace-state.json"), state).unwrap();
        let found = checkouts(&packages);
        assert_eq!(found["tally"], packages.join("checkouts/Tally"));
        assert_eq!(found["swift-collections"], packages.join("checkouts/swift-collections"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

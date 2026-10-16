//! Swift package dependencies: what a project declares (package references
//! in `project.pbxproj`, `.package` entries in `Package.swift`), what they
//! resolved to (`Package.resolved`) and where they're checked out, for the
//! Package Dependencies section of the project navigator; and the commands
//! behind File ▸ Packages (Resolve Package Versions, Update to Latest
//! Package Versions, Reset Package Caches) and Update Package.
//!
//! Xcode projects and workspaces resolve with `xcodebuild
//! -resolvePackageDependencies` into the DerivedData folder their builds
//! use; Swift packages with `swift package resolve/update/reset`.

pub mod location;
pub mod manifest;
pub mod remote;
pub mod resolved;
pub mod rule;

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;

use crate::project::{ProjectInfo, ProjectKind};
use crate::xcodeproj::packages::ProjectPackages;
use rule::Rule;

/// One package in the graph.
#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    pub identity: String,
    pub name: String,
    /// Repository URL, or the folder of a local package.
    pub location: String,
    pub local: bool,
    /// The resolved version, branch or commit (empty until resolved).
    pub state: String,
    pub rule: Option<Rule>,
    /// Where its sources are: the checkout, or a local package's folder.
    pub checkout: Option<PathBuf>,
    /// Declared by the project itself rather than by another package.
    pub direct: bool,
    /// The `.xcodeproj` that declares it and its package reference's ID.
    pub reference: Option<(PathBuf, String)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dependencies {
    pub packages: Vec<Package>,
    pub resolved_file: Option<PathBuf>,
    /// The folder with `checkouts` (`SourcePackages` or `.build`).
    pub packages_dir: Option<PathBuf>,
}

impl Dependencies {
    pub fn package(&self, identity: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.identity.eq_ignore_ascii_case(identity))
    }
}

/// Local packages an `.xcworkspace` lists directly (folders with a
/// `Package.swift`).
pub fn workspace_local_packages(workspace: &Path) -> Vec<PathBuf> {
    let base = workspace.parent().unwrap_or(workspace);
    let Ok(data) = std::fs::read_to_string(workspace.join("contents.xcworkspacedata")) else { return vec![] };
    data.split("location = \"")
        .skip(1)
        .filter_map(|part| part.split('"').next())
        .filter_map(|location| {
            let (kind, path) = location.split_once(':').unwrap_or(("group", location));
            let path = match kind {
                "absolute" => PathBuf::from(path),
                "self" => return None,
                _ => base.join(path),
            };
            path.join("Package.swift").exists().then(|| crate::xcodeproj::files::normalize(&path))
        })
        .collect()
}

/// Reads a project's package graph. Blocking.
pub fn load(project: &ProjectInfo, xcode_projects: &[PathBuf], cache_dir: &Path) -> Dependencies {
    let resolved_file = location::resolved_file(&project.kind, &project.root);
    let pins = resolved_file
        .as_ref()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| resolved::parse(&t).ok())
        .unwrap_or_default();
    let packages_dir = location::packages_dir(&project.kind, &project.root)
        .filter(|d| d.exists())
        .or_else(|| {
            // Before anything resolved into DerivedData, zcode's own index
            // build may have.
            let index = cache_dir.join("IndexDerivedData/SourcePackages");
            (project.kind.is_xcode() && index.exists()).then_some(index)
        });
    let checkouts = packages_dir.as_deref().map(location::checkouts).unwrap_or_default();

    // What the project declares.
    let mut declared: Vec<Package> = vec![];
    let add_local = |declared: &mut Vec<Package>, path: PathBuf, reference: Option<(PathBuf, String)>| {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        declared.push(Package {
            identity: name.to_lowercase(),
            name,
            location: path.display().to_string(),
            local: true,
            state: String::new(),
            rule: None,
            checkout: path.exists().then(|| path.clone()),
            direct: true,
            reference,
        });
    };
    match &project.kind {
        ProjectKind::XcodeProject(_) | ProjectKind::XcodeWorkspace(_) => {
            for xcodeproj in xcode_projects {
                let text = std::fs::read_to_string(crate::xcodeproj::XcodeProject::pbxproj_path(xcodeproj)).unwrap_or_default();
                let Ok(packages) = ProjectPackages::parse(&text) else { continue };
                for reference in packages.references {
                    let declared_by = Some((xcodeproj.clone(), reference.id.clone()));
                    if let Some(path) = reference.local_path(xcodeproj) {
                        add_local(&mut declared, path, declared_by);
                        continue;
                    }
                    declared.push(Package {
                        identity: resolved::identity(&reference.location),
                        name: reference.name(),
                        location: reference.location.clone(),
                        local: false,
                        state: String::new(),
                        rule: reference.rule.clone(),
                        checkout: None,
                        direct: true,
                        reference: declared_by,
                    });
                }
            }
            if let ProjectKind::XcodeWorkspace(workspace) = &project.kind {
                for path in workspace_local_packages(workspace) {
                    add_local(&mut declared, path, None);
                }
            }
        }
        ProjectKind::SwiftPackage => {
            let text = std::fs::read_to_string(project.root.join("Package.swift")).unwrap_or_default();
            for dependency in manifest::declared(&text) {
                if let Some(path) = &dependency.path {
                    add_local(&mut declared, crate::xcodeproj::files::normalize(&project.root.join(path)), None);
                    continue;
                }
                let url = dependency.url.clone().unwrap_or_default();
                declared.push(Package {
                    identity: dependency.identity(),
                    name: resolved::display_name(&url),
                    location: url,
                    local: false,
                    state: String::new(),
                    rule: dependency.rule.clone(),
                    checkout: None,
                    direct: true,
                    reference: None,
                });
            }
        }
        ProjectKind::Folder => {}
    }

    let mut packages: Vec<Package> = vec![];
    for pin in &pins {
        let direct = declared.iter().find(|d| d.identity == pin.identity);
        packages.push(Package {
            identity: pin.identity.clone(),
            name: direct.map(|d| d.name.clone()).unwrap_or_else(|| resolved::display_name(&pin.location)),
            location: direct.map(|d| d.location.clone()).unwrap_or_else(|| pin.location.clone()),
            local: false,
            state: pin.state_label(),
            rule: direct.and_then(|d| d.rule.clone()),
            checkout: checkouts.get(&pin.identity).cloned(),
            direct: direct.is_some(),
            reference: direct.and_then(|d| d.reference.clone()),
        });
    }
    for package in declared {
        if !packages.iter().any(|p| p.identity == package.identity) {
            let checkout = package.checkout.clone().or_else(|| checkouts.get(&package.identity).cloned());
            packages.push(Package { checkout, ..package });
        }
    }
    packages.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Dependencies { packages, resolved_file, packages_dir }
}

/// File ▸ Packages commands and Update Package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Resolve,
    UpdateAll,
    Update(String),
    Reset,
}

impl Operation {
    /// What the activity view says meanwhile.
    pub fn activity(&self) -> String {
        match self {
            Self::Resolve => "Resolving Package Graph".into(),
            Self::UpdateAll => "Updating Packages".into(),
            Self::Update(name) => format!("Updating {name}"),
            Self::Reset => "Resetting Package Caches".into(),
        }
    }
}

/// The file changes an operation makes before resolving. For Xcode
/// projects, updating drops pins and checkouts (so `xcodebuild` resolves
/// those packages to the newest versions their rules allow) and resetting
/// drops the whole packages folder. Swift packages leave this to `swift
/// package`. Blocking.
pub fn prepare(project: &ProjectInfo, operation: &Operation, dependencies: &Dependencies, cache_dir: &Path) -> Result<()> {
    if !project.kind.is_xcode() {
        return Ok(());
    }
    let resolved_file = location::resolved_file(&project.kind, &project.root);
    // zcode's index build keeps its own checkouts.
    let index = cache_dir.join("IndexDerivedData/SourcePackages");
    let dirs: Vec<&PathBuf> = dependencies.packages_dir.iter().chain([&index]).filter(|d| d.exists()).collect();
    match operation {
        Operation::Resolve => {}
        Operation::UpdateAll => {
            if let Some(file) = resolved_file.filter(|f| f.exists()) {
                std::fs::remove_file(file)?;
            }
            for dir in dirs {
                location::forget_checkouts(dir, None)?;
            }
        }
        Operation::Update(identity) => {
            let identities = std::slice::from_ref(identity);
            if let Some(file) = resolved_file.filter(|f| f.exists()) {
                let text = std::fs::read_to_string(&file)?;
                if let Some(updated) = resolved::without_pins(&text, identities)? {
                    std::fs::write(&file, updated)?;
                }
            }
            for dir in dirs {
                location::forget_checkouts(dir, Some(identities))?;
            }
        }
        Operation::Reset => {
            for dir in dirs {
                std::fs::remove_dir_all(dir)?;
            }
        }
    }
    Ok(())
}

/// The commands that carry out `operation`, in order.
pub fn commands(project: &ProjectInfo, operation: &Operation) -> Vec<Command> {
    let swift = |args: &[&str]| {
        let mut cmd = Command::new("/usr/bin/swift");
        cmd.arg("package").args(args).current_dir(&project.root);
        cmd
    };
    match &project.kind {
        ProjectKind::SwiftPackage => match operation {
            Operation::Resolve => vec![swift(&["resolve"])],
            Operation::UpdateAll => vec![swift(&["update"])],
            Operation::Update(identity) => vec![swift(&["update", identity])],
            Operation::Reset => vec![swift(&["reset"]), swift(&["resolve"])],
        },
        kind if kind.is_xcode() => {
            let mut cmd = Command::new("/usr/bin/xcrun");
            cmd.arg("xcodebuild").arg("-resolvePackageDependencies").args(kind.xcodebuild_args()).current_dir(&project.root);
            vec![cmd]
        }
        _ => vec![],
    }
}

/// What the activity view shows for a line of resolver output.
pub fn progress(line: &str) -> Option<String> {
    let line = line.trim();
    let name_after = |prefix: &str| -> Option<String> {
        let rest = line.strip_prefix(prefix)?.trim();
        let location = rest.split_whitespace().next()?;
        Some(resolved::display_name(location))
    };
    if line.starts_with("Resolve Package Graph") || line.starts_with("Computing version for") {
        return Some("Resolving Package Graph".into());
    }
    if let Some(name) = name_after("Fetching from").or_else(|| name_after("Fetching")) {
        return Some(format!("Fetching {name}"));
    }
    if let Some(name) = name_after("Updating") {
        return Some(format!("Updating {name}"));
    }
    if let Some(name) = name_after("Creating working copy for").or_else(|| name_after("Cloning")) {
        return Some(format!("Checking out {name}"));
    }
    None
}

/// Error messages in resolver output: each `error:` line, with the
/// indented lines that continue it.
pub fn errors(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut continuing = false;
    for line in lines {
        if let Some(at) = line.find("error: ") {
            let message = line[at + "error: ".len()..].trim().to_string();
            if !message.is_empty() {
                out.push(message);
            }
            continuing = true;
        } else if continuing && line.starts_with(char::is_whitespace) && !line.trim().is_empty() {
            if let Some(last) = out.last_mut() {
                if last.ends_with(':') {
                    last.push(' ');
                } else {
                    last.push_str(": ");
                }
                last.push_str(line.trim());
            }
        } else {
            continuing = false;
        }
    }
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_progress() {
        assert_eq!(progress("Fetching from https://github.com/apple/swift-collections.git").as_deref(), Some("Fetching swift-collections"));
        assert_eq!(progress("Fetching /tmp/zcode-tally/Tally.git").as_deref(), Some("Fetching Tally"));
        assert_eq!(progress("Creating working copy for /tmp/zcode-tally/Tally.git").as_deref(), Some("Checking out Tally"));
        assert_eq!(progress("Resolve Package Graph").as_deref(), Some("Resolving Package Graph"));
        assert_eq!(progress("Resolved source packages:"), None);
    }

    #[test]
    fn collects_errors() {
        let lines: Vec<String> = [
            "Resolve Package Graph",
            "xcodebuild: error: Could not resolve package dependencies:",
            "  Failed to clone repository https://example.invalid/nothing.git:",
            "    fatal: unable to access",
            "",
            "error: Dependencies could not be resolved because no versions of 'tally' match the requirement 9.0.0..<10.0.0",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            errors(&lines),
            [
                "Could not resolve package dependencies: Failed to clone repository https://example.invalid/nothing.git: fatal: unable to access",
                "Dependencies could not be resolved because no versions of 'tally' match the requirement 9.0.0..<10.0.0",
            ]
        );
    }

    #[test]
    fn swiftpm_commands() {
        let project = ProjectInfo { root: PathBuf::from("/p"), name: "P".into(), kind: ProjectKind::SwiftPackage };
        let args = |op: Operation| -> Vec<Vec<String>> {
            commands(&project, &op)
                .iter()
                .map(|c| c.get_args().map(|a| a.to_string_lossy().to_string()).collect())
                .collect()
        };
        assert_eq!(args(Operation::Update("tally".into())), [["package", "update", "tally"]]);
        assert_eq!(args(Operation::Reset), [["package", "reset"], ["package", "resolve"]]);
        let xcode = ProjectInfo { root: PathBuf::from("/p"), name: "A".into(), kind: ProjectKind::XcodeProject(PathBuf::from("/p/A.xcodeproj")) };
        let cmd = &commands(&xcode, &Operation::UpdateAll)[0];
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(args, ["xcodebuild", "-resolvePackageDependencies", "-project", "/p/A.xcodeproj"]);
    }

    #[test]
    fn loads_a_package_graph() {
        let dir = std::env::temp_dir().join(format!("zcode-graph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".build/checkouts/Tally")).unwrap();
        std::fs::create_dir_all(dir.join("LocalKit")).unwrap();
        std::fs::write(
            dir.join("Package.swift"),
            "let p = Package(name: \"P\", dependencies: [\n    .package(url: \"file:///tmp/Tally.git\", from: \"1.0.0\"),\n    .package(path: \"LocalKit\"),\n])\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("Package.resolved"),
            r#"{"pins":[{"identity":"tally","kind":"localSourceControl","location":"/tmp/Tally.git","state":{"revision":"abc","version":"1.2.0"}},{"identity":"swift-numerics","kind":"remoteSourceControl","location":"https://github.com/apple/swift-numerics","state":{"revision":"def","version":"1.0.2"}}],"version":3}"#,
        )
        .unwrap();
        let project = ProjectInfo { root: dir.clone(), name: "P".into(), kind: ProjectKind::SwiftPackage };
        let graph = load(&project, &[], &dir);
        let summary: Vec<(String, String, bool, bool)> =
            graph.packages.iter().map(|p| (p.name.clone(), p.state.clone(), p.direct, p.checkout.is_some())).collect();
        assert_eq!(
            summary,
            [
                ("LocalKit".to_string(), String::new(), true, true),
                ("swift-numerics".to_string(), "1.0.2".to_string(), false, false),
                ("Tally".to_string(), "1.2.0".to_string(), true, true),
            ]
        );
        assert_eq!(graph.package("tally").unwrap().rule, Some(Rule::UpToNextMajor("1.0.0".into())));
        assert!(graph.package("localkit").unwrap().local);
        std::fs::remove_dir_all(&dir).ok();
    }
}

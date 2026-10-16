//! Project detection (Xcode workspace/project, Swift package, plain folder)
//! and file scanning.

use std::path::{Path, PathBuf};

use gpui_kit::component::tree::TreeItem;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectKind {
    XcodeWorkspace(PathBuf),
    XcodeProject(PathBuf),
    SwiftPackage,
    Folder,
}

impl ProjectKind {
    pub fn is_xcode(&self) -> bool {
        matches!(self, Self::XcodeWorkspace(_) | Self::XcodeProject(_))
    }

    /// `-workspace X` / `-project X` arguments for xcodebuild.
    pub fn xcodebuild_args(&self) -> Vec<String> {
        match self {
            Self::XcodeWorkspace(path) => vec!["-workspace".into(), path.display().to_string()],
            Self::XcodeProject(path) => vec!["-project".into(), path.display().to_string()],
            _ => vec![],
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::XcodeWorkspace(_) => "Xcode Workspace",
            Self::XcodeProject(_) => "Xcode Project",
            Self::SwiftPackage => "Swift Package",
            Self::Folder => "Folder",
        }
    }
}

/// The `.xcodeproj` bundles a project opens: itself, or a workspace's
/// projects in order.
pub fn xcode_projects(kind: &ProjectKind) -> Vec<PathBuf> {
    match kind {
        ProjectKind::XcodeProject(path) => vec![path.clone()],
        ProjectKind::XcodeWorkspace(path) => {
            let mut projects = crate::xcodeproj::workspace_projects(path);
            // A project's own embedded workspace refers to "self:".
            if projects.is_empty()
                && let Some(parent) = path.parent().filter(|p| p.extension().is_some_and(|e| e == "xcodeproj"))
            {
                projects.push(parent.to_path_buf());
            }
            projects
        }
        _ => vec![],
    }
}

#[derive(Clone, Debug)]
pub struct ProjectInfo {
    pub root: PathBuf,
    pub name: String,
    pub kind: ProjectKind,
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e == ext)
}

/// Works out what the user opened. Accepts a folder, an `.xcodeproj`,
/// `.xcworkspace`, `Package.swift`, or any file inside a project.
pub fn detect(path: &Path) -> ProjectInfo {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let stem = |p: &Path| {
        p.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    };

    if has_ext(&path, "xcworkspace") {
        return ProjectInfo {
            root: path.parent().unwrap_or(&path).to_path_buf(),
            name: stem(&path),
            kind: ProjectKind::XcodeWorkspace(path.clone()),
        };
    }
    if has_ext(&path, "xcodeproj") {
        return ProjectInfo {
            root: path.parent().unwrap_or(&path).to_path_buf(),
            name: stem(&path),
            kind: ProjectKind::XcodeProject(path.clone()),
        };
    }
    let dir = if path.is_dir() {
        path.clone()
    } else {
        path.parent().unwrap_or(&path).to_path_buf()
    };
    let dir_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Project".into());

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.sort();

    if let Some(ws) = entries.iter().find(|p| has_ext(p, "xcworkspace")) {
        return ProjectInfo {
            root: dir.clone(),
            name: stem(ws),
            kind: ProjectKind::XcodeWorkspace(ws.clone()),
        };
    }
    if let Some(proj) = entries.iter().find(|p| has_ext(p, "xcodeproj")) {
        return ProjectInfo {
            root: dir.clone(),
            name: stem(proj),
            kind: ProjectKind::XcodeProject(proj.clone()),
        };
    }
    if dir.join("Package.swift").exists() {
        return ProjectInfo {
            root: dir.clone(),
            name: dir_name,
            kind: ProjectKind::SwiftPackage,
        };
    }
    ProjectInfo {
        root: dir,
        name: dir_name,
        kind: ProjectKind::Folder,
    }
}

const SKIP_DIRS: &[&str] = &[
    ".build",
    ".swiftpm",
    "DerivedData",
    "xcuserdata",
    "node_modules",
    ".git",
    ".zcode",
];

const MAX_FILES: usize = 100_000;

/// Scans the project for files, honoring `.gitignore`. Blocking.
pub fn scan_files(root: &Path) -> Vec<PathBuf> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .require_git(false)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            if !is_dir {
                return true;
            }
            !(SKIP_DIRS.contains(&name.as_ref())
                || name.ends_with(".xcodeproj")
                || name.ends_with(".xcworkspace"))
        })
        .build();

    let mut files = Vec::new();
    for entry in walker.flatten() {
        if entry.file_type().is_some_and(|t| t.is_file())
            && let Ok(rel) = entry.path().strip_prefix(root)
        {
            files.push(rel.to_path_buf());
            if files.len() >= MAX_FILES {
                break;
            }
        }
    }
    files.sort_by(|a, b| {
        a.to_string_lossy()
            .to_lowercase()
            .cmp(&b.to_string_lossy().to_lowercase())
    });
    files
}

/// Builds the navigator tree. Folders sort before files, like Finder.
pub fn build_tree(root: &Path, files: &[PathBuf]) -> Vec<TreeItem> {
    #[derive(Default)]
    struct Dir {
        dirs: std::collections::BTreeMap<String, Dir>,
        files: Vec<String>,
    }
    let mut top = Dir::default();
    for file in files {
        let mut node = &mut top;
        let components: Vec<_> = file
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        let Some((name, parents)) = components.split_last() else { continue };
        for parent in parents {
            node = node.dirs.entry(parent.clone()).or_default();
        }
        node.files.push(name.clone());
    }

    let expand_top = files.len() < 400;
    fn to_items(dir: Dir, path: &Path, depth: usize, expand_top: bool) -> Vec<TreeItem> {
        let mut items = Vec::new();
        let mut dirs: Vec<_> = dir.dirs.into_iter().collect();
        dirs.sort_by_key(|(name, _)| name.to_lowercase());
        for (name, child) in dirs {
            let child_path = path.join(&name);
            let children = to_items(child, &child_path, depth + 1, expand_top);
            items.push(
                TreeItem::new(child_path.to_string_lossy().to_string(), name)
                    .expanded(expand_top && depth == 0)
                    .children(children),
            );
        }
        let mut files = dir.files;
        files.sort_by_key(|name| name.to_lowercase());
        for name in files {
            let child_path = path.join(&name);
            items.push(TreeItem::new(child_path.to_string_lossy().to_string(), name));
        }
        items
    }
    to_items(top, root, 0, expand_top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_groups_folders_first() {
        let files = vec![
            PathBuf::from("b.swift"),
            PathBuf::from("Sources/App/main.swift"),
            PathBuf::from("Sources/App/View.swift"),
            PathBuf::from("a.md"),
        ];
        let items = build_tree(Path::new("/p"), &files);
        assert_eq!(items[0].label.as_ref(), "Sources");
        assert_eq!(items[1].label.as_ref(), "a.md");
        assert_eq!(items[2].label.as_ref(), "b.swift");
    }
}
